//! `.gaibundle` export and import over HTTP.

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use gai::bundle::{ExportOptions, ImportOptions, ImportReport};

use crate::AppState;
use crate::error::ApiResult;

/// Largest bundle accepted.
const MAX_BUNDLE: usize = 200 << 20;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/bundles/export", post(export))
        .route("/v1/bundles/inspect", post(inspect))
        .route("/v1/bundles/import", post(import))
        .layer(DefaultBodyLimit::max(MAX_BUNDLE))
}

async fn export(State(s): State<AppState>, Json(opts): Json<ExportOptions>) -> ApiResult<Response> {
    let bytes = gai::bundle::export(&s.gai, &opts).await?;
    let stem: String = opts
        .name
        .as_deref()
        .unwrap_or("gai")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let mut resp = bytes.into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{stem}.gaibundle\"")) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(resp)
}

async fn inspect(State(s): State<AppState>, body: Bytes) -> ApiResult<Json<ImportReport>> {
    Ok(Json(gai::bundle::inspect(&s.gai, &body).await?))
}

async fn import(
    State(s): State<AppState>,
    Query(opts): Query<ImportOptions>,
    body: Bytes,
) -> ApiResult<Json<ImportReport>> {
    Ok(Json(gai::bundle::import(&s.gai, &body, &opts).await?))
}
