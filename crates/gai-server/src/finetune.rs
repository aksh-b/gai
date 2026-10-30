//! Fine-tuning on rated runs: training sets, jobs, and the poller that
//! adds finished models to the catalog.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use gai::finetune::{DatasetFilter, DatasetStats, FineTuneJob, FineTunes, build_dataset};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::AppState;
use crate::api::Done;
use crate::error::{ApiError, ApiResult};
use crate::orgs::Tenant;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/finetunes", get(list).post(start))
        .route("/v1/finetunes/dataset", post(dataset))
        .route("/v1/finetunes/dataset.jsonl", post(download))
        .route("/v1/finetunes/{name}", get(get_one).delete(delete))
        .route("/v1/finetunes/{name}/cancel", post(cancel))
        .route("/v1/finetunes/{name}/check", post(check))
}

async fn list(Tenant(s): Tenant) -> ApiResult<Json<Vec<FineTuneJob>>> {
    Ok(Json(s.gai.fine_tunes().await?.list().await?))
}

/// Build the training set and start training (JSON, or `{"yaml": "..."}`).
async fn start(
    Tenant(s): Tenant,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<FineTuneJob>)> {
    let value = match body.get("yaml").and_then(Value::as_str) {
        Some(y) => {
            let v: serde_yaml::Value = serde_yaml::from_str(y)
                .map_err(|e| ApiError::bad_request(format!("fine-tune: {e}")))?;
            serde_json::to_value(v).map_err(|e| ApiError::bad_request(e.to_string()))?
        }
        None => body,
    };
    let spec = FineTunes::parse(value)?;
    let job = s.gai.fine_tunes().await?.start(spec).await?;
    Ok((StatusCode::CREATED, Json(job)))
}

#[derive(Serialize, JsonSchema)]
pub struct DatasetPreview {
    pub stats: DatasetStats,
    /// The first few examples.
    pub sample: Vec<Value>,
}

/// What a training set would hold, with a few examples.
async fn dataset(
    Tenant(s): Tenant,
    Json(f): Json<DatasetFilter>,
) -> ApiResult<Json<DatasetPreview>> {
    let (data, stats) = build_dataset(s.rt.store(), &f).await?;
    let sample = data
        .lines()
        .take(3)
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    Ok(Json(DatasetPreview { stats, sample }))
}

/// The training set itself, as JSON Lines.
async fn download(Tenant(s): Tenant, Json(f): Json<DatasetFilter>) -> ApiResult<impl IntoResponse> {
    let (data, _) = build_dataset(s.rt.store(), &f).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/jsonl"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"gai-training.jsonl\"",
            ),
        ],
        data,
    ))
}

async fn get_one(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<Json<FineTuneJob>> {
    Ok(Json(s.gai.fine_tunes().await?.get(&name).await?))
}

async fn delete(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<Json<Done>> {
    if !s.gai.fine_tunes().await?.delete(&name).await? {
        return Err(ApiError::not_found(format!("fine-tune {name}")));
    }
    Ok(Json(Done { ok: true }))
}

async fn cancel(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<Json<FineTuneJob>> {
    Ok(Json(s.gai.fine_tunes().await?.cancel(&name).await?))
}

/// Check with the provider now instead of waiting for the next minute.
async fn check(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<Json<FineTuneJob>> {
    s.gai.poll_fine_tunes().await?;
    Ok(Json(s.gai.fine_tunes().await?.get(&name).await?))
}

/// Check running fine-tunes once a minute.
pub fn start_poller(gai: Arc<gai::Gai>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            match gai.poll_fine_tunes().await {
                Ok(changed) => {
                    for j in changed {
                        tracing::info!(job = %j.spec.name, status = ?j.status, model = ?j.model, "fine-tune changed");
                    }
                }
                Err(e) => tracing::warn!(error = %e, "could not check fine-tunes"),
            }
        }
    });
}
