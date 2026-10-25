//! The audit log (recorded by middleware), retention sweeps and deleting a
//! user's data.

use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, Request};
use axum::http::{HeaderValue, Method, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use gai::store::{AuditEntry, AuditFilter, Deleted, NewAudit};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::AppState;
use crate::auth::Principal;
use crate::error::{ApiError, ApiResult};
use crate::orgs::Tenant;

/// Bodies read for the audit detail; bigger ones are recorded without it.
const DETAIL_BODY_LIMIT: usize = 256 << 10;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/audit", get(list))
        .route("/v1/changes", get(changes))
        .route("/v1/users/{id}", delete(forget))
        .route("/v1/retention/sweep", post(sweep))
}

/// Whether a request changes something worth recording. Running agents,
/// chatting, uploads and searches are not audited (runs have their own
/// history); changes to configuration, keys and approvals are.
fn audited(method: &Method, path: &str) -> bool {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) || !path.starts_with("/v1/")
    {
        return false;
    }
    const SKIP_EXACT: [&str; 6] = [
        "/v1/runs",
        "/v1/blobs",
        "/v1/audio/speech",
        "/v1/audio/transcriptions",
        "/v1/images/generations",
        "/v1/workflows/run",
    ];
    const SKIP_SUFFIX: [&str; 8] = [
        "/search",
        "/render",
        "/preview",
        "/validate",
        "/test",
        "/feedback",
        "/cancel",
        "/inspect",
    ];
    if SKIP_EXACT.contains(&path)
        || SKIP_SUFFIX.iter().any(|s| path.ends_with(s))
        || path.starts_with("/v1/webhooks/")
        || (path.starts_with("/v1/evals/") && path.ends_with("/runs"))
        || path.starts_with("/v1/workflows/") && path.ends_with("/runs")
    {
        return false;
    }
    true
}

/// `agents/researcher` from `/v1/agents/researcher`, plus the body's name
/// when the path doesn't carry one (`POST /v1/agents {name}`).
fn target(path: &str, body: &Value) -> String {
    let t = path.trim_start_matches("/v1/").trim_end_matches('/');
    match body.get("name").and_then(Value::as_str) {
        Some(n) if !t.contains('/') => format!("{t}/{n}"),
        _ => t.to_owned(),
    }
}

/// Facts worth keeping from a request body, never secrets: approval
/// decisions and scopes, but not keys, tokens or whole specs.
fn detail(path: &str, body: &Value) -> Value {
    let mut d = serde_json::Map::new();
    if path.contains("/approvals/") {
        for k in ["approved", "reason"] {
            if let Some(v) = body.get(k) {
                d.insert(k.into(), v.clone());
            }
        }
    }
    for k in [
        "scopes",
        "budget_usd",
        "default_model",
        "version",
        "label",
        "all",
    ] {
        if let Some(v) = body.get(k) {
            d.insert(k.into(), v.clone());
        }
    }
    Value::Object(d)
}

/// Middleware: record changes in the audit log after they are answered.
pub async fn record(Tenant(s): Tenant, req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    if !audited(&method, &path) {
        return next.run(req).await;
    }
    let principal = req.extensions().get::<Principal>().cloned();
    let json_body = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"))
        && req
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok()?.parse::<usize>().ok())
            .is_some_and(|n| n <= DETAIL_BODY_LIMIT);
    let (req, body) = if json_body {
        let (parts, b) = req.into_parts();
        let bytes: Bytes = axum::body::to_bytes(b, DETAIL_BODY_LIMIT)
            .await
            .unwrap_or_default();
        let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (Request::from_parts(parts, Body::from(bytes)), v)
    } else {
        (req, Value::Null)
    };
    let resp = next.run(req).await;
    let (actor, actor_id) = principal
        .map(|p| (p.name, p.id))
        .unwrap_or_else(|| ("anonymous".into(), "anonymous".into()));
    let entry = NewAudit {
        actor,
        actor_id,
        action: format!("{method} {path}"),
        target: target(&path, &body),
        status: resp.status().as_u16(),
        detail: detail(&path, &body),
    };
    if let Err(e) = s.rt.store().add_audit(entry).await {
        tracing::warn!(error = %e, "could not write the audit log");
    }
    resp
}

#[derive(Deserialize)]
struct ChangesQuery {
    /// e.g. `agents/helper`.
    target: String,
    #[serde(default)]
    limit: u32,
}

/// Who changed one thing, and when: the audit log for a target, without
/// details, for everyone who can see it.
#[derive(Serialize, JsonSchema)]
pub struct Change {
    pub at: chrono::DateTime<chrono::Utc>,
    pub actor: String,
    pub action: String,
    pub status: u16,
}

async fn changes(Tenant(s): Tenant, Query(q): Query<ChangesQuery>) -> ApiResult<Json<Vec<Change>>> {
    let rows =
        s.rt.store()
            .list_audit(AuditFilter {
                actor: None,
                target: Some(q.target.clone()),
                since: None,
                until: None,
                limit: if q.limit == 0 { 50 } else { q.limit.min(500) },
            })
            .await?;
    Ok(Json(
        rows.into_iter()
            // `target` matches by prefix; keep this one thing only.
            .filter(|r| r.target == q.target)
            .map(|r| Change {
                at: r.at,
                actor: r.actor,
                action: r.action,
                status: r.status,
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
struct AuditQuery {
    actor: Option<String>,
    target: Option<String>,
    since: Option<chrono::DateTime<chrono::Utc>>,
    until: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    limit: u32,
    /// `csv` for a download.
    format: Option<String>,
}

async fn list(Tenant(s): Tenant, Query(q): Query<AuditQuery>) -> ApiResult<Response> {
    let rows =
        s.rt.store()
            .list_audit(AuditFilter {
                actor: q.actor,
                target: q.target,
                since: q.since,
                until: q.until,
                limit: q.limit.min(10_000),
            })
            .await?;
    if q.format.as_deref() == Some("csv") {
        let mut out = String::from("id,at,actor,actor_id,action,target,status,detail\n");
        let cell = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        for r in &rows {
            out.push_str(&format!(
                "{},{},{},{},{},{},{},{}\n",
                r.id,
                r.at.to_rfc3339(),
                cell(&r.actor),
                cell(&r.actor_id),
                cell(&r.action),
                cell(&r.target),
                r.status,
                cell(&r.detail.to_string())
            ));
        }
        let mut resp = out.into_response();
        let h = resp.headers_mut();
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/csv; charset=utf-8"),
        );
        h.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_static("attachment; filename=\"gai-audit.csv\""),
        );
        return Ok(resp);
    }
    Ok(Json::<Vec<AuditEntry>>(rows).into_response())
}

async fn forget(Tenant(s): Tenant, Path(id): Path<String>) -> ApiResult<Json<Deleted>> {
    if id.trim().is_empty() {
        return Err(ApiError::bad_request("give a user id"));
    }
    Ok(Json(s.gai.forget_user(&id).await?))
}

#[derive(Serialize, JsonSchema)]
pub struct Swept {
    /// What was deleted, per agent with a retention period.
    pub agents: Vec<SweptAgent>,
}

#[derive(Serialize, JsonSchema)]
pub struct SweptAgent {
    pub agent: String,
    #[serde(flatten)]
    pub deleted: Deleted,
}

async fn sweep(Tenant(s): Tenant) -> ApiResult<Json<Swept>> {
    let agents = s
        .gai
        .sweep_retention()
        .await?
        .into_iter()
        .map(|(agent, deleted)| SweptAgent { agent, deleted })
        .collect();
    Ok(Json(Swept { agents }))
}

/// Apply retention once an hour while the server runs.
pub fn start_retention(gai: std::sync::Arc<gai::Gai>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(3600));
        loop {
            tick.tick().await;
            if let Err(e) = gai.sweep_retention().await {
                tracing::warn!(error = %e, "retention sweep failed");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn what_is_audited() {
        assert!(audited(&Method::POST, "/v1/agents"));
        assert!(audited(&Method::DELETE, "/v1/keys/key_1"));
        assert!(audited(&Method::PUT, "/v1/settings"));
        assert!(audited(&Method::POST, "/v1/runs/r1/approvals/c1"));
        assert!(audited(&Method::POST, "/v1/bundles/import"));
        assert!(!audited(&Method::GET, "/v1/agents"));
        assert!(!audited(&Method::POST, "/v1/runs"));
        assert!(!audited(&Method::POST, "/v1/collections/hr/search"));
        assert!(!audited(&Method::POST, "/v1/evals/basics/runs"));
        assert!(!audited(&Method::POST, "/openai/v1/chat/completions"));
        assert_eq!(
            target("/v1/agents", &json!({"name": "researcher"})),
            "agents/researcher"
        );
        assert_eq!(
            target("/v1/agents/researcher/labels/prod", &json!({})),
            "agents/researcher/labels/prod"
        );
        let d = detail(
            "/v1/keys",
            &json!({"name": "ci", "scopes": ["run"], "secret": "x"}),
        );
        assert_eq!(d, json!({"scopes": ["run"]}));
    }
}
