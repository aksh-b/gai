//! The prompt optimizer over HTTP: started as a background job, followed by
//! polling, and applied (or not) as the agent's or prompt template's next
//! version.

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use gai::agent::PromptSpec;
use gai::eval::{OptimizeOptions, OptimizeReport, Target};
use gai::store::Job;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AppState;
use crate::api::Saved;
use crate::error::{ApiError, ApiResult};
use crate::orgs::Tenant;

/// The job kind optimizations are stored under.
const KIND: &str = "optimize";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/evals/{name}/optimize", post(start))
        .route("/v1/optimizations", get(list))
        .route("/v1/optimizations/{id}", get(get_one))
        .route("/v1/optimizations/{id}/apply", post(apply))
}

#[derive(Default, Deserialize, JsonSchema)]
#[serde(default)]
pub struct StartOptimize {
    /// The agent (defaults to the eval set's) and an optional model to run
    /// it on.
    #[serde(flatten)]
    pub target: Target,
    #[serde(flatten)]
    pub options: OptimizeOptions,
}

/// An optimization: what was asked, and what it found so far.
#[derive(Serialize, JsonSchema)]
pub struct Optimization {
    pub id: String,
    /// `running`, `done` or `failed`.
    pub status: String,
    pub eval_set: String,
    pub agent: Option<String>,
    pub report: Option<OptimizeReport>,
    pub error: Option<String>,
    /// The version the best instructions were saved as, once applied.
    pub applied: Option<Saved>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Default, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Apply {
    /// The version note; one with the scores is written when empty.
    pub note: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct Started {
    pub id: String,
}

#[derive(Deserialize)]
struct ListQuery {
    limit: Option<u32>,
}

fn view(j: Job) -> Optimization {
    let report = j
        .result
        .as_ref()
        .and_then(|r| serde_json::from_value(r["report"].clone()).ok());
    let applied = j
        .result
        .as_ref()
        .and_then(|r| serde_json::from_value(r["applied"].clone()).ok());
    Optimization {
        id: j.id,
        status: j.status,
        eval_set: j.input["eval_set"].as_str().unwrap_or_default().to_owned(),
        agent: j.input["agent"].as_str().map(str::to_owned),
        report,
        error: j.error,
        applied,
        created_at: j.created_at,
        updated_at: j.updated_at,
    }
}

/// Start an optimization in the background; follow it with
/// `GET /v1/optimizations/{id}`.
async fn start(
    Tenant(s): Tenant,
    Path(name): Path<String>,
    Json(b): Json<StartOptimize>,
) -> ApiResult<(StatusCode, Json<Started>)> {
    let ev = s.gai.evals().await?;
    // Fail fast on a bad set or agent, before going to the background.
    let (set, _) = ev.load(&name).await?;
    let agent = b
        .target
        .agent
        .clone()
        .or(set.agent.clone())
        .ok_or_else(|| {
            ApiError::bad_request(format!("eval set {name} names no agent; choose one"))
        })?;
    s.rt.resolve_agent(&agent).await?;
    let store = s.rt.store().clone();
    let job = store
        .create_job(
            KIND,
            &json!({"eval_set": name, "agent": agent, "options": {
                "writer_model": b.options.writer_model, "rounds": b.options.rounds,
                "candidates": b.options.candidates, "target_model": b.target.model,
            }}),
        )
        .await?;
    let id = job.id.clone();
    let target = Target {
        agent: Some(agent),
        model: b.target.model,
    };
    // One writer, so the job's updates land in order and the last wins.
    type Update = (&'static str, Option<OptimizeReport>, Option<String>);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Update>();
    let writer_id = id.clone();
    tokio::spawn(async move {
        let mut last: Option<OptimizeReport> = None;
        while let Some((status, report, error)) = rx.recv().await {
            if report.is_some() {
                last = report;
            }
            let result = last.as_ref().map(|r| json!({"report": r}));
            if let Err(e) = store
                .update_job(&writer_id, status, result.as_ref(), error.as_deref())
                .await
            {
                tracing::warn!(error = %e, "could not record an optimization");
            }
        }
    });
    let progress_tx = tx.clone();
    let progress: gai::eval::OptimizeProgress = std::sync::Arc::new(move |r| {
        if !r.done {
            let _ = progress_tx.send(("running", Some(r.clone()), None));
        }
    });
    tokio::spawn(async move {
        let _ = match ev
            .optimize(&name, &target, &b.options, Some(progress))
            .await
        {
            Ok(r) => tx.send(("done", Some(r), None)),
            Err(e) => {
                tracing::warn!(eval_set = %name, error = %e, "optimization failed");
                tx.send(("failed", None, Some(e.to_string())))
            }
        };
    });
    Ok((StatusCode::ACCEPTED, Json(Started { id: job.id })))
}

/// Newest first.
async fn list(Tenant(s): Tenant, Query(q): Query<ListQuery>) -> ApiResult<Json<Vec<Optimization>>> {
    let jobs =
        s.rt.store()
            .list_jobs(KIND, q.limit.unwrap_or(20).min(200))
            .await?;
    Ok(Json(jobs.into_iter().map(view).collect()))
}

async fn job(s: &AppState, id: &str) -> ApiResult<Job> {
    s.rt.store()
        .get_job(id)
        .await?
        .filter(|j| j.kind == KIND)
        .ok_or_else(|| ApiError::not_found(format!("optimization {id}")))
}

async fn get_one(Tenant(s): Tenant, Path(id): Path<String>) -> ApiResult<Json<Optimization>> {
    Ok(Json(view(job(&s, &id).await?)))
}

/// Save the best instructions found: as the prompt template's next version
/// when the agent uses one, else as the agent's.
async fn apply(
    Tenant(s): Tenant,
    Path(id): Path<String>,
    Json(b): Json<Apply>,
) -> ApiResult<(StatusCode, Json<Saved>)> {
    let j = job(&s, &id).await?;
    let o = view(j.clone());
    let report = o
        .report
        .filter(|r| r.done)
        .ok_or_else(|| ApiError::new(StatusCode::CONFLICT, "the optimization hasn't finished"))?;
    if !report.improved {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "nothing scored higher than the instructions as they are",
        ));
    }
    if o.applied.is_some() {
        return Err(ApiError::new(StatusCode::CONFLICT, "already applied"));
    }
    let note = b.note.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
        format!(
            "prompt optimizer on {}: score {:.2} → {:.2}",
            report.eval_set, report.baseline.score, report.best.score
        )
    });
    let saved = match &report.prompt {
        Some(reference) => {
            let name = reference.split('@').next().unwrap_or(reference);
            let (mut p, _): (PromptSpec, u32) = s.rt.resolve_prompt(name).await?;
            p.text = report.best.instructions.clone();
            let version = s.rt.save_prompt(&p, Some(&note)).await?;
            Saved {
                name: format!("prompt {}", p.name),
                version,
            }
        }
        None => {
            let (mut agent, _) = s.rt.resolve_agent(&report.agent).await?;
            agent.system = Some(report.best.instructions.clone());
            let version = s.rt.save_agent(&agent, Some(&note)).await?;
            Saved {
                name: agent.name,
                version,
            }
        }
    };
    let mut result = j.result.unwrap_or_default();
    result["applied"] = json!(saved);
    s.rt.store()
        .update_job(&id, &j.status, Some(&result), j.error.as_deref())
        .await?;
    Ok((StatusCode::CREATED, Json(saved)))
}

/// Optimizations a restart cut short are marked failed.
pub fn recover(gai: std::sync::Arc<gai::Gai>) {
    tokio::spawn(async move {
        let Ok(rt) = gai.runtime().await else { return };
        let store = rt.store();
        for j in store.list_jobs(KIND, 200).await.unwrap_or_default() {
            if j.status == "running" {
                let _ = store
                    .update_job(
                        &j.id,
                        "failed",
                        j.result.as_ref(),
                        Some("the server restarted"),
                    )
                    .await;
            }
        }
    });
}
