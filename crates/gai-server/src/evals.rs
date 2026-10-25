//! Feedback on replies, and evaluations: eval sets, running them, results
//! and comparisons.

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use gai::eval::{Comparison, EvalOptions, EvalReport, EvalSet, Target, Verdict, compare, gate};
use gai::store::{
    EvalRun, EvalSetSummary, EvalSetVersion, Feedback, FeedbackFilter, FeedbackSummary,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::AppState;
use crate::api::{Done, Saved};
use crate::auth::Principal;
use crate::error::{ApiError, ApiResult};
use crate::orgs::Tenant;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/runs/{id}/feedback",
            post(rate).delete(unrate).get(run_feedback),
        )
        .route("/v1/feedback", get(list_feedback))
        .route("/v1/feedback/summary", get(feedback_summary))
        .route("/v1/evals", get(list_sets).post(save_set))
        .route("/v1/evals/{name}", get(get_set).delete(delete_set))
        .route("/v1/evals/{name}/versions", get(set_versions))
        .route("/v1/evals/{name}/runs", post(start_run).get(set_runs))
        .route("/v1/eval-runs", get(list_runs))
        .route("/v1/eval-runs/compare", get(compare_runs))
        .route("/v1/eval-runs/{id}", get(get_run).delete(delete_run))
}

// ---- feedback ----

#[derive(Deserialize, JsonSchema)]
pub struct Rate {
    /// 1 for good, -1 for bad.
    pub rating: i8,
    pub comment: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Who is rating: the API key's name, or `anonymous`.
fn rater(p: &Principal) -> String {
    p.name.clone()
}

async fn rate(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Path(id): Path<String>,
    Json(body): Json<Rate>,
) -> ApiResult<Json<Feedback>> {
    let run =
        s.rt.store()
            .get_run(&id)
            .await?
            .ok_or_else(|| ApiError::not_found(format!("run {id}")))?;
    let tags: Vec<String> = body
        .tags
        .iter()
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
        .collect();
    let comment = body
        .comment
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty());
    Ok(Json(
        s.rt.store()
            .put_feedback(&run, &rater(&p), body.rating, comment, &tags)
            .await?,
    ))
}

async fn unrate(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<Json<Done>> {
    s.rt.store().delete_feedback(&id, &rater(&p)).await?;
    Ok(Json(Done { ok: true }))
}

async fn run_feedback(Tenant(s): Tenant, Path(id): Path<String>) -> ApiResult<Json<Vec<Feedback>>> {
    Ok(Json(
        s.rt.store()
            .list_feedback(FeedbackFilter {
                run_id: Some(id),
                ..Default::default()
            })
            .await?,
    ))
}

#[derive(Deserialize)]
struct FeedbackQuery {
    agent: Option<String>,
    rating: Option<i8>,
    #[serde(default)]
    limit: u32,
}

async fn list_feedback(
    Tenant(s): Tenant,
    Query(q): Query<FeedbackQuery>,
) -> ApiResult<Json<Vec<Feedback>>> {
    Ok(Json(
        s.rt.store()
            .list_feedback(FeedbackFilter {
                agent: q.agent,
                run_id: None,
                rating: q.rating,
                limit: q.limit,
            })
            .await?,
    ))
}

#[derive(Deserialize)]
struct AgentQuery {
    agent: Option<String>,
}

async fn feedback_summary(
    Tenant(s): Tenant,
    Query(q): Query<AgentQuery>,
) -> ApiResult<Json<Vec<FeedbackSummary>>> {
    Ok(Json(
        s.rt.store().feedback_summary(q.agent.as_deref()).await?,
    ))
}

// ---- eval sets ----

async fn list_sets(Tenant(s): Tenant) -> ApiResult<Json<Vec<EvalSetSummary>>> {
    Ok(Json(s.rt.store().list_eval_sets().await?))
}

#[derive(Deserialize)]
struct NoteQuery {
    note: Option<String>,
}

/// The body is an eval set as JSON, or `{"yaml": "..."}`.
async fn save_set(
    Tenant(s): Tenant,
    Query(q): Query<NoteQuery>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Saved>> {
    let set = match body.get("yaml").and_then(Value::as_str) {
        Some(y) => EvalSet::from_yaml(y)?,
        None => {
            let mut set: EvalSet = serde_json::from_value(body)
                .map_err(|e| ApiError::bad_request(format!("eval set: {e}")))?;
            set.normalize()?;
            set
        }
    };
    let version = s.gai.evals().await?.save(&set, q.note.as_deref()).await?;
    Ok(Json(Saved {
        name: set.name,
        version,
    }))
}

#[derive(Serialize, JsonSchema)]
pub struct EvalSetView {
    pub name: String,
    pub version: u32,
    pub set: EvalSet,
}

async fn get_set(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<Json<EvalSetView>> {
    let (set, version) = s.gai.evals().await?.load(&name).await?;
    Ok(Json(EvalSetView {
        name: set.name.clone(),
        version,
        set,
    }))
}

async fn set_versions(
    Tenant(s): Tenant,
    Path(name): Path<String>,
) -> ApiResult<Json<Vec<EvalSetVersion>>> {
    Ok(Json(s.rt.store().list_eval_set_versions(&name).await?))
}

async fn delete_set(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<Json<Done>> {
    if !s.rt.store().delete_eval_set(&name).await? {
        return Err(ApiError::not_found(format!("eval set {name}")));
    }
    Ok(Json(Done { ok: true }))
}

// ---- evaluations ----

#[derive(Default, Deserialize, JsonSchema)]
#[serde(default)]
pub struct StartEval {
    /// The agent (defaults to the set's) and an optional model override.
    #[serde(flatten)]
    pub target: Target,
    /// Several models to compare: one evaluation per model.
    pub models: Vec<String>,
    pub judge_model: Option<String>,
    pub concurrency: Option<usize>,
    pub cases: Vec<String>,
    pub tags: Vec<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct StartedEvals {
    /// One per model, in the order given.
    pub eval_run_ids: Vec<String>,
}

/// Start evaluations in the background; follow them with
/// `GET /v1/eval-runs/{id}`.
async fn start_run(
    Tenant(s): Tenant,
    Path(name): Path<String>,
    Json(body): Json<StartEval>,
) -> ApiResult<(StatusCode, Json<StartedEvals>)> {
    let ev = s.gai.evals().await?;
    // Fail fast on a bad set or agent, before going to the background.
    let (set, _) = ev.load(&name).await?;
    let agent = body
        .target
        .agent
        .clone()
        .or(set.agent.clone())
        .ok_or_else(|| {
            ApiError::bad_request(format!("eval set {name} names no agent; choose one"))
        })?;
    s.rt.resolve_agent(&agent).await?;
    let opts = EvalOptions {
        concurrency: body.concurrency.unwrap_or(4).clamp(1, 16),
        judge_model: body.judge_model.clone(),
        cases: body.cases.clone(),
        tags: body.tags.clone(),
    };
    let models: Vec<Option<String>> = if body.models.is_empty() {
        vec![body.target.model.clone()]
    } else {
        body.models.iter().cloned().map(Some).collect()
    };
    let mut ids = Vec::new();
    for model in models {
        let target = Target {
            agent: Some(agent.clone()),
            model,
        };
        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        let tx = std::sync::Mutex::new(Some(tx));
        let (ev, name, opts) = (ev.clone(), name.clone(), opts.clone());
        let started: gai::eval::Progress = std::sync::Arc::new(move |p| {
            if let gai::eval::EvalProgress::Started { eval_run_id, .. } = p
                && let Some(tx) = tx.lock().unwrap().take()
            {
                let _ = tx.send(eval_run_id);
            }
        });
        tokio::spawn(async move {
            if let Err(e) = ev.run(&name, &target, &opts, Some(started)).await {
                tracing::warn!(eval_set = %name, error = %e, "evaluation failed");
            }
        });
        match rx.await {
            Ok(id) => ids.push(id),
            Err(_) => {
                return Err(ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "the evaluation did not start",
                ));
            }
        }
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(StartedEvals { eval_run_ids: ids }),
    ))
}

#[derive(Deserialize)]
struct RunsQuery {
    set: Option<String>,
    #[serde(default)]
    limit: u32,
}

async fn list_runs(Tenant(s): Tenant, Query(q): Query<RunsQuery>) -> ApiResult<Json<Vec<EvalRun>>> {
    Ok(Json(
        s.rt.store()
            .list_eval_runs(q.set.as_deref(), q.limit)
            .await?,
    ))
}

async fn set_runs(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<Json<Vec<EvalRun>>> {
    Ok(Json(s.rt.store().list_eval_runs(Some(&name), 50).await?))
}

async fn get_run(Tenant(s): Tenant, Path(id): Path<String>) -> ApiResult<Json<EvalReport>> {
    Ok(Json(s.gai.evals().await?.report(&id).await?))
}

async fn delete_run(Tenant(s): Tenant, Path(id): Path<String>) -> ApiResult<Json<Done>> {
    if !s.rt.store().delete_eval_run(&id).await? {
        return Err(ApiError::not_found(format!("evaluation {id}")));
    }
    Ok(Json(Done { ok: true }))
}

#[derive(Deserialize)]
struct CompareQuery {
    baseline: String,
    candidate: String,
    /// Largest score drop that still passes (default 0.05).
    max_drop: Option<f64>,
}

#[derive(Serialize, JsonSchema)]
pub struct CompareView {
    #[serde(flatten)]
    pub comparison: Comparison,
    pub verdict: Verdict,
}

async fn compare_runs(
    Tenant(s): Tenant,
    Query(q): Query<CompareQuery>,
) -> ApiResult<Json<CompareView>> {
    let ev = s.gai.evals().await?;
    let (b, c) = (
        ev.report(&q.baseline).await?,
        ev.report(&q.candidate).await?,
    );
    Ok(Json(CompareView {
        comparison: compare(&b, &c),
        verdict: gate(&c, Some(&b), q.max_drop.unwrap_or(0.05)),
    }))
}
