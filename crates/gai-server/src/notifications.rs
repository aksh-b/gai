//! Notification rules and web push subscriptions.

use axum::extract::Path;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use gai::notifications::{Event, NotificationRule, Notifications, NotifyEvent, PushDevice, Sent};
use gai::tools::webpush::PushSubscription;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::AppState;
use crate::api::Done;
use crate::auth::Principal;
use crate::error::{ApiError, ApiResult};
use crate::orgs::Tenant;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/notifications/rules", get(list).post(save))
        .route(
            "/v1/notifications/rules/{name}",
            get(get_one).delete(delete),
        )
        .route("/v1/notifications/rules/{name}/test", post(test))
        .route("/v1/notifications/push", get(push_info).post(subscribe))
        .route("/v1/notifications/push/unsubscribe", post(unsubscribe))
}

async fn list(Tenant(s): Tenant) -> ApiResult<Json<Vec<NotificationRule>>> {
    Ok(Json(s.gai.notifications().await?.rules().await?))
}

/// The body is a rule as JSON, or `{"yaml": "..."}`.
async fn save(
    Tenant(s): Tenant,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<NotificationRule>)> {
    let value = match body.get("yaml").and_then(Value::as_str) {
        Some(y) => {
            let v: serde_yaml::Value =
                serde_yaml::from_str(y).map_err(|e| ApiError::bad_request(format!("rule: {e}")))?;
            serde_json::to_value(v).map_err(|e| ApiError::bad_request(e.to_string()))?
        }
        None => body,
    };
    let rule = Notifications::parse(value)?;
    s.gai.notifications().await?.save_rule(&rule).await?;
    Ok((StatusCode::CREATED, Json(rule)))
}

async fn get_one(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<Json<NotificationRule>> {
    Ok(Json(s.gai.notifications().await?.rule(&name).await?))
}

async fn delete(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<Json<Done>> {
    if !s.gai.notifications().await?.delete_rule(&name).await? {
        return Err(ApiError::not_found(format!("notification rule {name}")));
    }
    Ok(Json(Done { ok: true }))
}

/// Send a sample of the rule's first event through it now.
async fn test(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<Json<Sent>> {
    let n = s.gai.notifications().await?;
    let rule = n.rule(&name).await?;
    let kind = rule.on.first().copied().unwrap_or(NotifyEvent::RunFinished);
    let e = Event {
        kind,
        source: rule.agents.first().cloned(),
        subject: format!("Test from gai: {name}"),
        text: format!(
            "This is how the rule {name} will reach you when {}.",
            match kind {
                NotifyEvent::Approval => "a call waits for approval",
                NotifyEvent::RunFailed => "a run fails",
                NotifyEvent::RunFinished => "a run finishes",
                NotifyEvent::Alert => "a dashboard alert fires",
                NotifyEvent::Suggestion => "an assistant suggests something",
            }
        ),
        link: Some("#/notifications".into()),
        meta: json!({"test": true}),
    };
    Ok(Json(n.send(&rule, &e).await))
}

#[derive(Serialize, JsonSchema)]
pub struct PushInfo {
    /// For `PushManager.subscribe({applicationServerKey})`.
    pub public_key: String,
    /// Browsers subscribed in this organization.
    pub devices: Vec<PushDevice>,
}

async fn push_info(Tenant(s): Tenant) -> ApiResult<Json<PushInfo>> {
    let n = s.gai.notifications().await?;
    Ok(Json(PushInfo {
        public_key: n.vapid().await?.public_key(),
        devices: n.devices().await?,
    }))
}

async fn subscribe(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Json(sub): Json<PushSubscription>,
) -> ApiResult<(StatusCode, Json<PushDevice>)> {
    let device = s
        .gai
        .notifications()
        .await?
        .subscribe(sub, Some(p.user_id.clone().unwrap_or(p.id)))
        .await?;
    Ok((StatusCode::CREATED, Json(device)))
}

#[derive(Deserialize, JsonSchema)]
pub struct Unsubscribe {
    /// The device id, or the subscription's endpoint.
    pub id: String,
}

async fn unsubscribe(Tenant(s): Tenant, Json(b): Json<Unsubscribe>) -> ApiResult<Json<Done>> {
    let gone = s.gai.notifications().await?.unsubscribe(&b.id).await?;
    Ok(Json(Done { ok: gone }))
}
