//! API keys, scopes, rate limits and per-key budgets.

use std::num::NonZeroU32;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use gai::AuthMode;
use gai::store::{ApiKey, Scope, SharedStore, keys};
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use schemars::JsonSchema;
use serde::Serialize;

use crate::AppState;
use crate::error::ApiError;

/// Who is calling.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct Principal {
    /// The key id, or `anonymous` when authentication is open.
    pub id: String,
    pub name: String,
    pub scopes: Vec<Scope>,
    #[serde(skip)]
    pub key: Option<ApiKey>,
}

impl Principal {
    fn anonymous() -> Self {
        Self {
            id: "anonymous".into(),
            name: "anonymous".into(),
            scopes: vec![Scope::Admin],
            key: None,
        }
    }

    fn from_key(k: ApiKey) -> Self {
        Self {
            id: k.id.clone(),
            name: k.name.clone(),
            scopes: k.scopes.clone(),
            key: Some(k),
        }
    }

    /// Refuse new spending once a key has used its budget.
    pub fn check_budget(&self) -> Result<(), ApiError> {
        if let Some(k) = &self.key
            && let Some(budget) = k.budget_usd
            && k.spent_usd >= budget
        {
            return Err(ApiError::new(
                StatusCode::PAYMENT_REQUIRED,
                format!(
                    "API key {} has used its ${budget:.2} budget (${:.4} spent)",
                    k.name, k.spent_usd
                ),
            ));
        }
        Ok(())
    }

    pub fn key_id(&self) -> Option<&str> {
        self.key.as_ref().map(|k| k.id.as_str())
    }
}

/// Add what a call cost to the key that made it.
pub async fn charge(store: &SharedStore, key_id: Option<&str>, usd: f64) {
    if let Some(id) = key_id
        && usd > 0.0
        && let Err(e) = store.add_key_spend(id, usd).await
    {
        tracing::warn!(key = id, error = %e, "could not record key spend");
    }
}

#[derive(Clone)]
pub struct AuthPolicy {
    pub mode: AuthMode,
    /// The server is only reachable from this machine (loopback bind, or a
    /// container port published on loopback).
    pub local: bool,
    pub limiter: Option<Arc<DefaultKeyedRateLimiter<String>>>,
}

impl AuthPolicy {
    pub fn new(mode: AuthMode, local: bool, per_minute: u32) -> Self {
        let limiter =
            NonZeroU32::new(per_minute).map(|n| Arc::new(RateLimiter::keyed(Quota::per_minute(n))));
        Self {
            mode,
            local,
            limiter,
        }
    }

    /// Whether requests without a key are let in right now.
    pub async fn open(&self, store: &SharedStore) -> bool {
        match self.mode {
            AuthMode::Off => true,
            AuthMode::Required => false,
            AuthMode::Auto => self.local && !store.has_keys().await.unwrap_or(true),
        }
    }
}

/// The scope a request needs, from its method and path.
pub fn required_scope(method: &Method, path: &str) -> Scope {
    // Saving or deleting a workflow is admin; running or validating one is not.
    let workflow_admin = path.starts_with("/v1/workflows")
        && !path.ends_with("/runs")
        && path != "/v1/workflows/run"
        && path != "/v1/workflows/validate";
    let admin_write = (path.starts_with("/v1/agents")
        || (path.starts_with("/v1/personas") && path != "/v1/personas/render")
        || (path.starts_with("/v1/triggers") && path != "/v1/triggers/preview")
        || path.starts_with("/v1/mcp")
        || path.starts_with("/v1/skills")
        || path.starts_with("/v1/settings")
        || path.starts_with("/v1/providers")
        || path.starts_with("/v1/templates")
        || workflow_admin)
        && method != Method::GET;
    if path.starts_with("/v1/keys") || admin_write {
        Scope::Admin
    } else if method == Method::GET || method == Method::HEAD {
        Scope::Read
    } else {
        Scope::Run
    }
}

fn token(req: &Request) -> Option<String> {
    header_token(req.headers())
}

/// The API key a request carries (`Authorization: Bearer` or `x-api-key`).
pub(crate) fn header_token(h: &axum::http::HeaderMap) -> Option<String> {
    h.get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
        })
        .or_else(|| h.get("x-api-key").and_then(|v| v.to_str().ok()))
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
}

pub async fn authenticate(State(s): State<AppState>, mut req: Request, next: Next) -> Response {
    let path = req.uri().path();
    // The OAuth callback is reached by a browser redirect, without a key;
    // it only completes a sign-in an admin started (matched by `state`).
    // Webhooks carry their trigger's signature instead of a key.
    // Setup status and claiming check for themselves who may use them.
    let public = path == "/v1/health"
        || path == "/v1/setup/status"
        || path == "/v1/setup/claim"
        || path == "/v1/mcp/oauth/callback"
        || path.starts_with("/v1/webhooks/");
    // Agent cards are public: they are how other agents find gai.
    let a2a = (path == "/a2a" || path.starts_with("/a2a/")) && !path.contains("/.well-known/");
    let guarded = (path.starts_with("/v1/")
        || path.starts_with("/openai/")
        || path == "/mcp"
        || path.starts_with("/mcp/")
        || a2a)
        && !public;
    if !guarded || req.method() == Method::OPTIONS {
        return next.run(req).await;
    }
    let store = s.rt.store();
    let principal = match token(&req) {
        Some(t) => match store.find_key(&keys::hash(&t)).await {
            Ok(Some(k)) => Principal::from_key(k),
            Ok(None) => {
                return ApiError::new(StatusCode::UNAUTHORIZED, "invalid or revoked API key")
                    .into_response();
            }
            Err(e) => return ApiError::from(e).into_response(),
        },
        None if s.auth.open(store).await => Principal::anonymous(),
        None => {
            return ApiError::new(
                StatusCode::UNAUTHORIZED,
                "an API key is required: send `Authorization: Bearer <key>` (create one with `gai keys create`)",
            )
            .into_response();
        }
    };
    let needed = required_scope(req.method(), path);
    if !Scope::allows(&principal.scopes, needed) {
        return ApiError::new(
            StatusCode::FORBIDDEN,
            format!("this key lacks the {needed:?} scope").to_lowercase(),
        )
        .into_response();
    }
    if let Some(limiter) = &s.auth.limiter
        && let Err(not_until) = limiter.check_key(&principal.id)
    {
        let wait = not_until.wait_time_from(governor::clock::Clock::now(
            &governor::clock::DefaultClock::default(),
        ));
        let mut resp =
            ApiError::new(StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded").into_response();
        if let Ok(v) = (wait.as_secs() + 1).to_string().parse() {
            resp.headers_mut().insert(header::RETRY_AFTER, v);
        }
        return resp;
    }
    if let Some(id) = principal.key_id().map(str::to_owned) {
        let store = store.clone();
        tokio::spawn(async move {
            let _ = store.touch_key(&id).await;
        });
    }
    req.extensions_mut().insert(principal);
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_by_route() {
        assert_eq!(required_scope(&Method::GET, "/v1/runs"), Scope::Read);
        assert_eq!(required_scope(&Method::POST, "/v1/runs"), Scope::Run);
        assert_eq!(required_scope(&Method::GET, "/v1/keys"), Scope::Admin);
        assert_eq!(required_scope(&Method::POST, "/v1/agents"), Scope::Admin);
        assert_eq!(required_scope(&Method::GET, "/v1/agents"), Scope::Read);
        assert_eq!(required_scope(&Method::POST, "/v1/personas"), Scope::Admin);
        assert_eq!(
            required_scope(&Method::PUT, "/v1/personas/defaults"),
            Scope::Admin
        );
        assert_eq!(required_scope(&Method::GET, "/v1/personas"), Scope::Read);
        assert_eq!(
            required_scope(&Method::POST, "/v1/personas/render"),
            Scope::Run
        );
        assert_eq!(required_scope(&Method::POST, "/v1/triggers"), Scope::Admin);
        assert_eq!(required_scope(&Method::GET, "/v1/triggers"), Scope::Read);
        assert_eq!(required_scope(&Method::POST, "/mcp"), Scope::Run);
        assert_eq!(
            required_scope(&Method::PUT, "/v1/providers/openai/key"),
            Scope::Admin
        );
        assert_eq!(required_scope(&Method::PUT, "/v1/settings"), Scope::Admin);
        assert_eq!(required_scope(&Method::GET, "/v1/settings"), Scope::Read);
        assert_eq!(required_scope(&Method::GET, "/mcp"), Scope::Read);
        assert_eq!(required_scope(&Method::POST, "/a2a/helper"), Scope::Run);
        assert_eq!(
            required_scope(&Method::POST, "/openai/v1/chat/completions"),
            Scope::Run
        );
    }
}
