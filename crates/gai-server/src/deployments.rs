//! Model servers this gai runs in containers, and the machine's GPUs.

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use gai::deployments::{DeploymentView, Deployments, GpuStatus};
use serde::Deserialize;
use serde_json::Value;

use crate::AppState;
use crate::api::Done;
use crate::error::{ApiError, ApiResult};
use crate::orgs::Tenant;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/deployments", get(list).post(deploy))
        .route("/v1/deployments/gpus", get(gpus))
        .route("/v1/deployments/{name}", get(get_one).delete(delete))
        .route("/v1/deployments/{name}/start", post(start))
        .route("/v1/deployments/{name}/stop", post(stop))
        .route("/v1/deployments/{name}/logs", get(logs))
}

async fn manager(s: &AppState) -> ApiResult<&Deployments> {
    s.gai.deployments().await?.ok_or_else(|| {
        ApiError::new(
            StatusCode::FORBIDDEN,
            "model servers are managed in the default organization",
        )
    })
}

async fn list(Tenant(s): Tenant) -> ApiResult<Json<Vec<DeploymentView>>> {
    Ok(Json(manager(&s).await?.list().await?))
}

/// Save a deployment (JSON, or `{"yaml": "..."}`) and start it.
async fn deploy(
    Tenant(s): Tenant,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<DeploymentView>)> {
    let value = match body.get("yaml").and_then(Value::as_str) {
        Some(y) => {
            let v: serde_yaml::Value = serde_yaml::from_str(y)
                .map_err(|e| ApiError::bad_request(format!("deployment: {e}")))?;
            serde_json::to_value(v).map_err(|e| ApiError::bad_request(e.to_string()))?
        }
        None => body,
    };
    let spec = Deployments::parse(value)?;
    Ok((
        StatusCode::CREATED,
        Json(manager(&s).await?.deploy(spec).await?),
    ))
}

async fn get_one(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<Json<DeploymentView>> {
    Ok(Json(manager(&s).await?.get(&name).await?))
}

async fn start(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<Json<DeploymentView>> {
    Ok(Json(manager(&s).await?.start(&name).await?))
}

async fn stop(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<Json<DeploymentView>> {
    Ok(Json(manager(&s).await?.stop(&name).await?))
}

async fn delete(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<Json<Done>> {
    if !manager(&s).await?.delete(&name).await? {
        return Err(ApiError::not_found(format!("deployment {name}")));
    }
    Ok(Json(Done { ok: true }))
}

#[derive(Deserialize)]
struct LogsQuery {
    lines: Option<usize>,
}

async fn logs(
    Tenant(s): Tenant,
    Path(name): Path<String>,
    Query(q): Query<LogsQuery>,
) -> ApiResult<String> {
    Ok(manager(&s)
        .await?
        .logs(&name, q.lines.unwrap_or(200))
        .await?)
}

async fn gpus(Tenant(s): Tenant) -> ApiResult<Json<GpuStatus>> {
    Ok(Json(manager(&s).await?.gpus().await))
}
