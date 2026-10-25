//! Versions of agents and workflows (diff, rollback, workflow labels) and
//! replaying runs.

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use gai::agent::ReplayRequest;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

use crate::AppState;
use crate::api::{Done, LabelBody, Mode, RunIds, Saved, sse};
use crate::auth::Principal;
use crate::error::{ApiError, ApiResult};
use crate::orgs::Tenant;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/agents/{name}/diff", get(agent_diff))
        .route("/v1/agents/{name}/rollback", post(agent_rollback))
        .route("/v1/workflows/{name}/diff", get(workflow_diff))
        .route("/v1/workflows/{name}/rollback", post(workflow_rollback))
        .route(
            "/v1/workflows/{name}/labels/{label}",
            post(set_workflow_label).delete(remove_workflow_label),
        )
        .route("/v1/runs/{id}/replay", post(replay))
}

/// Two versions of a spec and a unified diff between them (as YAML).
#[derive(Serialize, JsonSchema)]
pub struct VersionDiff {
    pub name: String,
    pub from: u32,
    pub to: u32,
    pub from_yaml: String,
    pub to_yaml: String,
    /// Unified diff from `from_yaml` to `to_yaml`; empty when they match.
    pub diff: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct DiffQuery {
    /// Older version; defaults to the one before `to`.
    pub from: Option<u32>,
    /// Newer version; defaults to the latest.
    pub to: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
pub struct Rollback {
    /// The version to bring back. It is saved again as the newest version,
    /// so history is kept.
    pub version: u32,
    pub note: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ReplayRun {
    #[serde(flatten)]
    pub replay: ReplayRequest,
    /// `wait` (default) for the outcome, `stream` for events, or
    /// `background` to get the new run's id at once.
    #[serde(default)]
    pub mode: Mode,
}

/// A spec as YAML, for reading and diffing.
pub fn spec_yaml(spec: &Value) -> String {
    serde_yaml::to_string(spec).unwrap_or_else(|_| spec.to_string())
}

/// Unified diff between two texts, labelled with their names.
pub fn unified_diff(a: &str, b: &str, a_name: &str, b_name: &str) -> String {
    if a == b {
        return String::new();
    }
    similar::TextDiff::from_lines(a, b)
        .unified_diff()
        .context_radius(3)
        .header(a_name, b_name)
        .to_string()
}

/// Which two versions to compare, given the latest.
pub(crate) fn pick(q: &DiffQuery, latest: u32) -> ApiResult<(u32, u32)> {
    let to = q.to.unwrap_or(latest);
    let from = q.from.unwrap_or(to.saturating_sub(1).max(1));
    if from == 0 || to == 0 {
        return Err(ApiError::bad_request("versions start at 1"));
    }
    Ok((from, to))
}

pub(crate) fn diff_of(name: String, from: (u32, &Value), to: (u32, &Value)) -> VersionDiff {
    let (a, b) = (spec_yaml(from.1), spec_yaml(to.1));
    VersionDiff {
        diff: unified_diff(
            &a,
            &b,
            &format!("{name}@v{}", from.0),
            &format!("{name}@v{}", to.0),
        ),
        name,
        from: from.0,
        to: to.0,
        from_yaml: a,
        to_yaml: b,
    }
}

async fn agent_diff(
    Tenant(s): Tenant,
    Path(name): Path<String>,
    Query(q): Query<DiffQuery>,
) -> ApiResult<Json<VersionDiff>> {
    let store = s.rt.store();
    let latest = store
        .get_agent(&name, None)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("agent {name}")))?;
    let (from, to) = pick(&q, latest.version)?;
    let get = |v: u32| {
        let name = name.clone();
        async move {
            store
                .get_agent(&name, Some(v))
                .await?
                .ok_or_else(|| ApiError::not_found(format!("agent {name} v{v}")))
        }
    };
    let (a, b) = (get(from).await?, get(to).await?);
    Ok(Json(diff_of(name, (from, &a.spec), (to, &b.spec))))
}

async fn agent_rollback(
    Tenant(s): Tenant,
    Path(name): Path<String>,
    Json(b): Json<Rollback>,
) -> ApiResult<(StatusCode, Json<Saved>)> {
    let store = s.rt.store();
    let old = store
        .get_agent(&name, Some(b.version))
        .await?
        .ok_or_else(|| ApiError::not_found(format!("agent {name} v{}", b.version)))?;
    let note = b
        .note
        .unwrap_or_else(|| format!("rollback to v{}", b.version));
    let version = store.put_agent(&name, &old.spec, Some(&note)).await?;
    Ok((StatusCode::CREATED, Json(Saved { name, version })))
}

async fn workflow_diff(
    Tenant(s): Tenant,
    Path(name): Path<String>,
    Query(q): Query<DiffQuery>,
) -> ApiResult<Json<VersionDiff>> {
    let store = s.rt.store();
    let latest = store
        .get_workflow(&name, None)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("workflow {name}")))?;
    let (from, to) = pick(&q, latest.version)?;
    let get = |v: u32| {
        let name = name.clone();
        async move {
            store
                .get_workflow(&name, Some(v))
                .await?
                .ok_or_else(|| ApiError::not_found(format!("workflow {name} v{v}")))
        }
    };
    let (a, b) = (get(from).await?, get(to).await?);
    Ok(Json(diff_of(name, (from, &a.spec), (to, &b.spec))))
}

async fn workflow_rollback(
    Tenant(s): Tenant,
    Path(name): Path<String>,
    Json(b): Json<Rollback>,
) -> ApiResult<(StatusCode, Json<Saved>)> {
    let store = s.rt.store();
    let old = store
        .get_workflow(&name, Some(b.version))
        .await?
        .ok_or_else(|| ApiError::not_found(format!("workflow {name} v{}", b.version)))?;
    let note = b
        .note
        .unwrap_or_else(|| format!("rollback to v{}", b.version));
    let version = store.put_workflow(&name, &old.spec, Some(&note)).await?;
    Ok((StatusCode::CREATED, Json(Saved { name, version })))
}

async fn set_workflow_label(
    Tenant(s): Tenant,
    Path((name, label)): Path<(String, String)>,
    Json(b): Json<LabelBody>,
) -> ApiResult<Json<Done>> {
    s.rt.store()
        .set_workflow_label(&name, &label, Some(b.version))
        .await?;
    Ok(Json(Done { ok: true }))
}

async fn remove_workflow_label(
    Tenant(s): Tenant,
    Path((name, label)): Path<(String, String)>,
) -> ApiResult<Json<Done>> {
    s.rt.store().set_workflow_label(&name, &label, None).await?;
    Ok(Json(Done { ok: true }))
}

/// Fork a run: replay its steps before `from_step` from the recording and
/// run the rest live, with any overrides.
async fn replay(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Path(id): Path<String>,
    Json(b): Json<ReplayRun>,
) -> ApiResult<Response> {
    p.check_budget()?;
    let mut meta = Map::new();
    if let Some(key) = p.key_id() {
        meta.insert("key_id".into(), json!(key));
    }
    let mut handle = s.rt.replay(&id, b.replay, meta).await?;
    match b.mode {
        Mode::Background => {
            let ids = RunIds {
                run_id: handle.run_id.clone(),
                session_id: handle.session_id.clone(),
            };
            tokio::spawn(handle.wait());
            Ok((StatusCode::ACCEPTED, Json(ids)).into_response())
        }
        Mode::Stream => {
            let (tx, rx) = mpsc::channel(256);
            tokio::spawn(async move {
                while let Some(e) = handle.next_event().await {
                    let _ = tx.send(e).await;
                }
                handle.wait().await;
            });
            Ok(sse(rx).into_response())
        }
        Mode::Wait => Ok(Json(handle.wait().await).into_response()),
    }
}
