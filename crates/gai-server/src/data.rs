//! Database connections: saving them (admins), their schemas, and running
//! read-only queries.

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use gai::data::dashboards::{DashboardResults, DashboardView, Dashboards};
use gai::data::{Data, QueryResult, Schema};
use gai::store::DataConnection;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::AppState;
use crate::api::Done;
use crate::auth::Principal;
use crate::error::{ApiError, ApiResult};
use crate::orgs::Tenant;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/connections", get(list).post(save))
        .route("/v1/connections/{name}", get(get_one).delete(delete))
        .route("/v1/connections/{name}/schema", get(schema))
        .route("/v1/connections/{name}/query", post(query))
        .route("/v1/dashboards", get(list_dashboards).post(save_dashboard))
        .route(
            "/v1/dashboards/{name}",
            get(get_dashboard).delete(delete_dashboard),
        )
        .route("/v1/dashboards/{name}/refresh", post(refresh_dashboard))
}

async fn list_dashboards(Tenant(s): Tenant) -> ApiResult<Json<Vec<DashboardView>>> {
    Ok(Json(s.gai.dashboards().await?.list().await?))
}

/// The body is a dashboard spec as JSON, or `{"yaml": "..."}`. It is
/// refreshed right away.
async fn save_dashboard(
    Tenant(s): Tenant,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<DashboardView>)> {
    let value = match body.get("yaml").and_then(Value::as_str) {
        Some(y) => serde_yaml::from_str::<Value>(y)
            .map_err(|e| ApiError::bad_request(format!("dashboard: {e}")))?,
        None => body,
    };
    let spec = Dashboards::parse(value)?;
    let d = s.gai.dashboards().await?;
    d.save(&spec).await?;
    d.refresh(&spec.name).await?;
    Ok((StatusCode::CREATED, Json(d.get(&spec.name).await?)))
}

async fn get_dashboard(
    Tenant(s): Tenant,
    Path(name): Path<String>,
) -> ApiResult<Json<DashboardView>> {
    Ok(Json(s.gai.dashboards().await?.get(&name).await?))
}

async fn delete_dashboard(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<Json<Done>> {
    if !s.gai.dashboards().await?.delete(&name).await? {
        return Err(ApiError::not_found(format!("dashboard {name}")));
    }
    Ok(Json(Done { ok: true }))
}

async fn refresh_dashboard(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
) -> ApiResult<Json<DashboardResults>> {
    p.check_budget()?;
    Ok(Json(s.gai.dashboards().await?.refresh(&name).await?))
}

/// Refresh dashboards whose `refresh_minutes` have passed, checking once a
/// minute.
pub fn start_scheduler(gai: std::sync::Arc<gai::Gai>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            tick.tick().await;
            let Ok(d) = gai.dashboards().await else {
                continue;
            };
            match d.due().await {
                Ok(due) => {
                    for name in due {
                        if let Err(e) = d.refresh(&name).await {
                            tracing::warn!(dashboard = %name, error = %e, "could not refresh a dashboard");
                        }
                    }
                }
                Err(e) => tracing::warn!(error = %e, "could not list dashboards"),
            }
        }
    });
}

async fn list(Tenant(s): Tenant) -> ApiResult<Json<Vec<DataConnection>>> {
    Ok(Json(s.gai.data().await?.list().await?))
}

/// The body is a connection spec as JSON, or `{"yaml": "..."}`. The schema
/// is read right away, so a bad connection fails here.
async fn save(
    Tenant(s): Tenant,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<DataConnection>)> {
    let value = match body.get("yaml").and_then(Value::as_str) {
        Some(y) => serde_yaml::from_str::<Value>(y)
            .map_err(|e| ApiError::bad_request(format!("connection: {e}")))?,
        None => body,
    };
    let spec = Data::parse(value)?;
    let data = s.gai.data().await?;
    data.save(&spec).await?;
    if let Err(e) = data.schema(&spec.name, true).await {
        return Err(ApiError::bad_request(format!(
            "saved, but the database didn't answer: {e}"
        )));
    }
    Ok((StatusCode::CREATED, Json(data.get(&spec.name).await?)))
}

async fn get_one(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<Json<DataConnection>> {
    Ok(Json(s.gai.data().await?.get(&name).await?))
}

async fn delete(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<Json<Done>> {
    if !s.gai.data().await?.delete(&name).await? {
        return Err(ApiError::not_found(format!("connection {name}")));
    }
    Ok(Json(Done { ok: true }))
}

#[derive(Deserialize)]
struct SchemaQuery {
    /// Read it from the database again.
    #[serde(default)]
    refresh: bool,
}

async fn schema(
    Tenant(s): Tenant,
    Path(name): Path<String>,
    Query(q): Query<SchemaQuery>,
) -> ApiResult<Json<Schema>> {
    Ok(Json(s.gai.data().await?.schema(&name, q.refresh).await?))
}

#[derive(Deserialize, JsonSchema)]
pub struct RunQuery {
    /// One SELECT.
    pub sql: String,
    pub limit: Option<usize>,
    /// Only check it and return the plan.
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Serialize, JsonSchema)]
pub struct QueryAnswer {
    #[serde(flatten)]
    pub result: Option<QueryResult>,
    /// For a dry run.
    pub plan: Option<String>,
}

async fn query(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
    Json(b): Json<RunQuery>,
) -> ApiResult<Json<QueryAnswer>> {
    p.check_budget()?;
    let data = s.gai.data().await?;
    if b.dry_run {
        return Ok(Json(QueryAnswer {
            result: None,
            plan: Some(data.dry_run(&name, &b.sql).await?),
        }));
    }
    Ok(Json(QueryAnswer {
        result: Some(data.query(&name, &b.sql, b.limit).await?),
        plan: None,
    }))
}
