//! Triggers (schedules, webhooks, file watchers) and the webhook receiver.

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use gai::store::{TriggerEvent, TriggerRecord};
use gai::triggers::{Fired, TriggerSpec, Triggers, verify_signature};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::AppState;
use crate::error::{ApiError, ApiResult};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/triggers", get(list).post(create))
        .route("/v1/triggers/preview", post(preview))
        .route("/v1/triggers/{id}", get(get_one).put(update).delete(delete))
        .route("/v1/triggers/{id}/enable", post(enable))
        .route("/v1/triggers/{id}/disable", post(disable))
        .route("/v1/triggers/{id}/fire", post(fire))
        .route("/v1/triggers/{id}/secret", post(secret))
        .route("/v1/triggers/{id}/events", get(events))
        .route("/v1/webhooks/{id}", post(webhook))
}

#[derive(Serialize, JsonSchema)]
pub struct TriggerView {
    #[serde(flatten)]
    pub record: TriggerRecord,
    /// The schedule in words, for cron triggers.
    pub schedule_text: Option<String>,
    /// Where to send webhooks, for webhook triggers.
    pub webhook_path: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct CreatedTrigger {
    pub trigger: TriggerView,
    /// For webhooks: the signing secret. Also available later from
    /// `POST /v1/triggers/{id}/secret`.
    pub secret: Option<String>,
}

/// What an unsaved trigger would do.
#[derive(Serialize, JsonSchema)]
pub struct TriggerPreview {
    pub ok: bool,
    pub error: Option<String>,
    /// The schedule in words, for cron triggers.
    pub schedule_text: Option<String>,
    /// The next few times a schedule fires (UTC).
    pub next: Vec<chrono::DateTime<chrono::Utc>>,
}

#[derive(Serialize, JsonSchema)]
pub struct TriggerSecret {
    pub secret: String,
}

#[derive(Deserialize)]
struct EventsQuery {
    limit: Option<u32>,
}

fn view(r: TriggerRecord) -> TriggerView {
    let spec = Triggers::spec_of(&r).ok();
    TriggerView {
        schedule_text: spec.as_ref().and_then(TriggerSpec::describe),
        webhook_path: (r.kind == "webhook").then(|| format!("/v1/webhooks/{}", r.id)),
        record: r,
    }
}

async fn engine(s: &AppState) -> ApiResult<&Triggers> {
    Ok(s.gai.triggers().await?)
}

async fn list(State(s): State<AppState>) -> ApiResult<Json<Vec<TriggerView>>> {
    Ok(Json(
        s.rt.store()
            .list_triggers()
            .await?
            .into_iter()
            .map(view)
            .collect(),
    ))
}

async fn create(
    State(s): State<AppState>,
    Json(spec): Json<TriggerSpec>,
) -> ApiResult<(StatusCode, Json<CreatedTrigger>)> {
    let rec = engine(&s).await?.create(spec).await?;
    let secret = rec.secret.clone();
    Ok((
        StatusCode::CREATED,
        Json(CreatedTrigger {
            trigger: view(rec),
            secret,
        }),
    ))
}

/// Check a trigger without saving it: the spec, its target, and for
/// schedules the next times it fires.
async fn preview(State(s): State<AppState>, Json(v): Json<Value>) -> Json<TriggerPreview> {
    let fail = |e: String| {
        Json(TriggerPreview {
            ok: false,
            error: Some(e),
            schedule_text: None,
            next: Vec::new(),
        })
    };
    let spec: TriggerSpec = match serde_json::from_value(v) {
        Ok(s) => s,
        Err(e) => return fail(e.to_string()),
    };
    let mut next = Vec::new();
    let mut at = chrono::Utc::now();
    for _ in 0..5 {
        match spec.next_after(at) {
            Ok(Some(t)) => {
                next.push(t);
                at = t;
            }
            Ok(None) => break,
            Err(e) => return fail(e),
        }
    }
    let error = match engine(&s).await {
        Ok(t) => t.check(&spec).await.err().map(|e| e.to_string()),
        Err(e) => Some(e.message().to_owned()),
    };
    Json(TriggerPreview {
        ok: error.is_none(),
        error,
        schedule_text: spec.describe(),
        next,
    })
}

async fn get_one(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<TriggerView>> {
    Ok(Json(view(engine(&s).await?.get(&id).await?)))
}

async fn update(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(spec): Json<TriggerSpec>,
) -> ApiResult<Json<TriggerView>> {
    Ok(Json(view(engine(&s).await?.update(&id, spec).await?)))
}

async fn delete(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    engine(&s).await?.delete(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn enable(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<TriggerView>> {
    Ok(Json(view(engine(&s).await?.set_enabled(&id, true).await?)))
}

async fn disable(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<TriggerView>> {
    Ok(Json(view(engine(&s).await?.set_enabled(&id, false).await?)))
}

/// Fire now, by hand. The body, if any, is the payload templates see.
async fn fire(
    State(s): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<(StatusCode, Json<Fired>)> {
    let t = engine(&s).await?;
    let rec = t.get(&id).await?;
    let payload = payload_of(&body);
    let fired = t.fire(&rec, "manual", payload, false).await?;
    Ok((StatusCode::ACCEPTED, Json(fired)))
}

async fn secret(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<TriggerSecret>> {
    let rec = engine(&s).await?.get(&id).await?;
    rec.secret
        .map(|secret| Json(TriggerSecret { secret }))
        .ok_or_else(|| ApiError::bad_request(format!("{} is not a webhook trigger", rec.name)))
}

async fn events(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<EventsQuery>,
) -> ApiResult<Json<Vec<TriggerEvent>>> {
    let rec = engine(&s).await?.get(&id).await?;
    Ok(Json(
        s.rt.store()
            .list_trigger_events(&rec.id, q.limit.unwrap_or(50).min(500))
            .await?,
    ))
}

fn payload_of(body: &[u8]) -> Option<Value> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return None;
    }
    Some(
        serde_json::from_slice(body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(body).into_owned())),
    )
}

/// Receive a webhook. No API key: the request must be signed with the
/// trigger's secret (`X-Gai-Signature` or GitHub's `X-Hub-Signature-256`:
/// `sha256=` and the hex HMAC-SHA256 of the body) or carry it in
/// `X-Gai-Token`.
async fn webhook(
    State(s): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<(StatusCode, Json<Fired>)> {
    let t = engine(&s).await?;
    // Unknown, disabled and non-webhook triggers all look the same.
    let not_found = || ApiError::not_found(format!("webhook {id}"));
    let rec = s.rt.store().get_trigger(&id).await?.ok_or_else(not_found)?;
    if rec.id != id || rec.kind != "webhook" || !rec.enabled {
        return Err(not_found());
    }
    let header = |n: &str| headers.get(n).and_then(|v| v.to_str().ok());
    let signature = header("x-gai-signature").or_else(|| header("x-hub-signature-256"));
    let secret = rec.secret.as_deref().unwrap_or_default();
    if secret.is_empty() || !verify_signature(secret, &body, signature, header("x-gai-token")) {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "bad or missing signature (X-Gai-Signature: sha256=<hmac of the body>) or token (X-Gai-Token)",
        ));
    }
    let fired = t.fire(&rec, "webhook", payload_of(&body), false).await?;
    Ok((StatusCode::ACCEPTED, Json(fired)))
}
