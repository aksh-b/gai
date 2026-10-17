//! Workflows: storing them and running them.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use gai::orchestra::{GraphNode, WorkflowRequest, WorkflowSpec};
use gai::store::{WorkflowSummary, WorkflowVersion};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

use crate::AppState;
use crate::api::{Mode, RunIds, Saved, sse};
use crate::auth::Principal;
use crate::error::{ApiError, ApiResult};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/workflows", get(list).post(save))
        .route("/v1/workflows/validate", post(validate))
        .route("/v1/workflows/run", post(run_inline))
        .route("/v1/workflows/{name}", get(get_one).delete(delete))
        .route("/v1/workflows/{name}/versions", get(versions))
        .route("/v1/workflows/{name}/runs", post(run_named))
}

#[derive(Serialize, JsonSchema)]
pub struct WorkflowEntry {
    pub name: String,
    pub description: Option<String>,
    /// `stored` or `file`.
    pub source: String,
    pub latest_version: Option<u32>,
    /// Labels pointing at stored versions (e.g. `prod`).
    pub labels: std::collections::BTreeMap<String, u32>,
    pub path: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct WorkflowView {
    pub name: String,
    pub version: Option<u32>,
    pub spec: WorkflowSpec,
    /// Each node with the nodes it waits for.
    pub graph: Vec<GraphNode>,
}

#[derive(Serialize, JsonSchema)]
pub struct Validation {
    pub ok: bool,
    pub error: Option<String>,
    pub graph: Vec<GraphNode>,
}

#[derive(Deserialize, JsonSchema)]
pub struct RunWorkflow {
    #[serde(default)]
    pub inputs: Map<String, Value>,
    #[serde(default)]
    pub mode: Mode,
    /// A stored version to run instead of the latest.
    pub version: Option<u32>,
    pub user_id: Option<String>,
    #[serde(default)]
    pub auto_approve: bool,
    pub callback_url: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct RunInlineWorkflow {
    pub workflow: WorkflowSpec,
    #[serde(flatten)]
    pub run: RunWorkflow,
}

async fn list(State(s): State<AppState>) -> ApiResult<Json<Vec<WorkflowEntry>>> {
    let mut out: Vec<WorkflowEntry> =
        s.rt.store()
            .list_workflows()
            .await?
            .into_iter()
            .map(|w: WorkflowSummary| WorkflowEntry {
                name: w.name,
                description: w.description,
                source: "stored".into(),
                latest_version: Some(w.latest_version),
                labels: w.labels,
                path: None,
            })
            .collect();
    for (name, path) in s.wf.files() {
        if out.iter().any(|e| e.name == name) {
            continue;
        }
        let description = gai::orchestra::WorkflowSpec::load(&path)
            .ok()
            .and_then(|w| w.description);
        out.push(WorkflowEntry {
            name,
            description,
            source: "file".into(),
            latest_version: None,
            labels: Default::default(),
            path: Some(path.display().to_string()),
        });
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
struct SaveQuery {
    note: Option<String>,
}

async fn save(
    State(s): State<AppState>,
    Query(q): Query<SaveQuery>,
    Json(spec): Json<WorkflowSpec>,
) -> ApiResult<(StatusCode, Json<Saved>)> {
    let version = s.wf.save(&spec, q.note.as_deref()).await?;
    Ok((
        StatusCode::CREATED,
        Json(Saved {
            name: spec.name,
            version,
        }),
    ))
}

async fn validate(Json(spec): Json<WorkflowSpec>) -> Json<Validation> {
    match spec.graph() {
        Ok(graph) => Json(Validation {
            ok: true,
            error: None,
            graph,
        }),
        Err(e) => Json(Validation {
            ok: false,
            error: Some(e),
            graph: Vec::new(),
        }),
    }
}

#[derive(Deserialize)]
struct VersionQuery {
    version: Option<u32>,
}

async fn get_one(
    State(s): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<VersionQuery>,
) -> ApiResult<Json<WorkflowView>> {
    let reference = match q.version {
        Some(v) => format!("{name}@v{v}"),
        None => name.clone(),
    };
    let (spec, version) = s.wf.resolve(&reference).await?;
    let graph = spec.graph().map_err(ApiError::bad_request)?;
    Ok(Json(WorkflowView {
        name,
        version,
        spec,
        graph,
    }))
}

async fn versions(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<Vec<WorkflowVersion>>> {
    Ok(Json(s.rt.store().list_workflow_versions(&name).await?))
}

async fn delete(State(s): State<AppState>, Path(name): Path<String>) -> ApiResult<StatusCode> {
    if s.rt.store().delete_workflow(&name).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("stored workflow {name}")))
    }
}

async fn run_named(
    State(s): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
    Json(b): Json<RunWorkflow>,
) -> ApiResult<Response> {
    let reference = match b.version {
        Some(v) => format!("{name}@v{v}"),
        None => name,
    };
    let (spec, version) = s.wf.resolve(&reference).await?;
    run(s, p, spec, version, b).await
}

async fn run_inline(
    State(s): State<AppState>,
    Extension(p): Extension<Principal>,
    Json(b): Json<RunInlineWorkflow>,
) -> ApiResult<Response> {
    run(s, p, b.workflow, None, b.run).await
}

async fn run(
    s: AppState,
    p: Principal,
    spec: WorkflowSpec,
    version: Option<u32>,
    b: RunWorkflow,
) -> ApiResult<Response> {
    p.check_budget()?;
    let mut req = WorkflowRequest::new(spec, b.inputs);
    req.version = version;
    req.user_id = b.user_id;
    req.auto_approve = b.auto_approve;
    req.callback_url = b.callback_url;
    if let Some(id) = p.key_id() {
        req.metadata.insert("key_id".into(), json!(id));
    }
    match b.mode {
        Mode::Background => {
            let (run_id, session_id) = s.wf.enqueue(req).await?;
            Ok((StatusCode::ACCEPTED, Json(RunIds { run_id, session_id })).into_response())
        }
        Mode::Wait => Ok(Json(s.wf.run(req).await?).into_response()),
        Mode::Stream => {
            let mut h = s.wf.start(req).await?;
            let (tx, rx) = mpsc::channel(256);
            tokio::spawn(async move {
                while let Some(e) = h.next_event().await {
                    let _ = tx.send(e).await;
                }
                h.wait().await;
            });
            Ok(sse(rx).into_response())
        }
    }
}
