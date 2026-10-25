//! Sources synced into knowledge collections.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use gai::memory::sources::{SourceSpec, Sources, SyncStats};
use gai::store::SourceRecord;
use serde::Deserialize;
use serde_json::Value;

use crate::AppState;
use crate::api::Done;
use crate::error::{ApiError, ApiResult};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/sources", get(list).post(save))
        .route("/v1/sources/{name}", get(get_one).delete(delete))
        .route("/v1/sources/{name}/sync", post(sync))
}

async fn list(State(s): State<AppState>) -> ApiResult<Json<Vec<SourceRecord>>> {
    Ok(Json(s.rt.store().list_sources().await?))
}

/// The body is a source spec as JSON, or `{"yaml": "..."}`.
async fn save(
    State(s): State<AppState>,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<SourceRecord>)> {
    let value = match body.get("yaml").and_then(Value::as_str) {
        Some(y) => {
            let v: serde_yaml::Value = serde_yaml::from_str(y)
                .map_err(|e| ApiError::bad_request(format!("source: {e}")))?;
            serde_json::to_value(v).map_err(|e| ApiError::bad_request(e.to_string()))?
        }
        None => body,
    };
    let spec: SourceSpec = Sources::parse(value)?;
    let src = s.gai.sources().await?;
    src.save(&spec).await?;
    Ok((StatusCode::CREATED, Json(src.get(&spec.name).await?)))
}

async fn get_one(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<SourceRecord>> {
    Ok(Json(s.gai.sources().await?.get(&name).await?))
}

async fn delete(State(s): State<AppState>, Path(name): Path<String>) -> ApiResult<Json<Done>> {
    if !s.rt.store().delete_source(&name).await? {
        return Err(ApiError::not_found(format!("source {name}")));
    }
    Ok(Json(Done { ok: true }))
}

#[derive(Deserialize)]
struct SyncQuery {
    /// Answer when the sync is done, with its counts.
    #[serde(default)]
    wait: bool,
}

/// Sync now: in the background (202), or with `?wait=true` (200 and the
/// counts).
async fn sync(
    State(s): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<SyncQuery>,
) -> ApiResult<(StatusCode, Json<Option<SyncStats>>)> {
    let src = s.gai.sources().await?;
    let rec = src.get(&name).await?;
    if rec.status == "syncing" {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("{name} is already syncing"),
        ));
    }
    if q.wait {
        return Ok((StatusCode::OK, Json(Some(src.sync(&name).await?))));
    }
    tokio::spawn(async move {
        if let Err(e) = src.sync(&name).await {
            tracing::warn!(source = %name, error = %e, "source sync failed");
        }
    });
    Ok((StatusCode::ACCEPTED, Json(None)))
}

/// Sync sources whose `every_minutes` has passed, checking once a minute.
pub fn start_scheduler(gai: Arc<gai::Gai>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            let Ok(src) = gai.sources().await else {
                continue;
            };
            match src.due().await {
                Ok(due) => {
                    for name in due {
                        let src = src.clone();
                        tokio::spawn(async move {
                            if let Err(e) = src.sync(&name).await {
                                tracing::warn!(source = %name, error = %e, "scheduled source sync failed");
                            }
                        });
                    }
                }
                Err(e) => tracing::warn!(error = %e, "could not list sources"),
            }
        }
    });
}
