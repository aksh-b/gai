//! Signing in as a user: OpenID Connect (authorization code with PKCE),
//! or an email header from a proxy that did the sign-in. A sign-in is a
//! random token kept in an HttpOnly cookie; the store keeps its hash.
//!
//! The ID token comes straight from the provider's token endpoint over
//! TLS, so its signature isn't checked (OIDC Core 3.1.3.7); its issuer,
//! audience, expiry and nonce are.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use gai::store::{DEFAULT_ORG, Role, User, keys};
use gai::{OidcSettings, SsoSettings};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::AppState;
use crate::api::Done;
use crate::auth::{SESSION_COOKIE, cookie};
use crate::error::{ApiError, ApiResult};

/// Sign-in tokens start with this (API keys with `gai_`).
pub const LOGIN_PREFIX: &str = "gais_";

/// Sign-ins in progress, by `state`: they have ten minutes.
static PENDING: LazyLock<Mutex<HashMap<String, Pending>>> = LazyLock::new(Mutex::default);

struct Pending {
    verifier: String,
    nonce: String,
    return_to: String,
    redirect_uri: String,
    at: Instant,
}

#[derive(Clone, Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
}

/// Discovery documents by issuer, for an hour.
static DISCOVERED: LazyLock<Mutex<HashMap<String, (Instant, Discovery)>>> =
    LazyLock::new(Mutex::default);

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/auth/config", get(config))
        .route("/v1/auth/login", get(login))
        .route("/v1/auth/callback", get(callback))
        .route("/v1/auth/logout", post(logout))
}

fn sso(s: &AppState) -> &SsoSettings {
    &s.gai.config().server.sso
}

fn random_hex(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    getrandom::fill(&mut bytes).expect("the OS random source is available");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Serialize, JsonSchema)]
pub struct AuthConfig {
    /// Signing in with OIDC is set up.
    pub oidc: bool,
    /// The sign-in button's label.
    pub label: String,
    /// A proxy signs people in.
    pub proxy: bool,
}

async fn config(State(s): State<AppState>) -> Json<AuthConfig> {
    let c = sso(&s);
    Json(AuthConfig {
        oidc: c.oidc.is_some(),
        label: c
            .oidc
            .as_ref()
            .and_then(|o| o.label.clone())
            .unwrap_or_else(|| "Sign in with SSO".into()),
        proxy: c.trusted_header.is_some(),
    })
}

async fn discover(http: &reqwest::Client, o: &OidcSettings) -> ApiResult<Discovery> {
    let issuer = o.issuer.trim_end_matches('/').to_owned();
    if let Some((at, d)) = DISCOVERED.lock().expect("discovery lock").get(&issuer)
        && at.elapsed() < Duration::from_secs(3600)
    {
        return Ok(d.clone());
    }
    let url = format!("{issuer}/.well-known/openid-configuration");
    let d: Discovery = http
        .get(&url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                format!("OIDC discovery ({url}): {e}"),
            )
        })?
        .json()
        .await
        .map_err(|e| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                format!("OIDC discovery ({url}): {e}"),
            )
        })?;
    DISCOVERED
        .lock()
        .expect("discovery lock")
        .insert(issuer, (Instant::now(), d.clone()));
    Ok(d)
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .expect("http client")
}

/// This server's address as the browser sees it.
fn origin(h: &HeaderMap) -> String {
    let host = h
        .get("x-forwarded-host")
        .or_else(|| h.get(header::HOST))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    let proto = h
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    format!("{proto}://{host}")
}

/// Only paths on this server, so sign-in can't send people elsewhere.
fn safe_return(to: Option<&str>) -> String {
    match to {
        Some(t) if t.starts_with('/') && !t.starts_with("//") && !t.contains('\\') => t.to_owned(),
        _ => "/".into(),
    }
}

#[derive(Deserialize)]
struct LoginQuery {
    return_to: Option<String>,
}

/// Start signing in: off to the identity provider.
async fn login(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<LoginQuery>,
) -> ApiResult<Redirect> {
    let o = sso(&s)
        .oidc
        .clone()
        .ok_or_else(|| ApiError::not_found("OIDC sign-in isn't set up ([server.sso.oidc])"))?;
    let d = discover(&http(), &o).await?;
    let state = random_hex(16);
    let nonce = random_hex(16);
    let verifier = random_hex(32);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let redirect_uri = o
        .redirect_url
        .clone()
        .unwrap_or_else(|| format!("{}/v1/auth/callback", origin(&headers)));
    let scopes = o
        .scopes
        .clone()
        .unwrap_or_else(|| "openid email profile".into());
    let mut url = url::Url::parse(&d.authorization_endpoint).map_err(|e| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            format!("authorization_endpoint: {e}"),
        )
    })?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &o.client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", &scopes)
        .append_pair("state", &state)
        .append_pair("nonce", &nonce)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    let mut pending = PENDING.lock().expect("pending lock");
    pending.retain(|_, p| p.at.elapsed() < Duration::from_secs(600));
    pending.insert(
        state,
        Pending {
            verifier,
            nonce,
            return_to: safe_return(q.return_to.as_deref()),
            redirect_uri,
            at: Instant::now(),
        },
    );
    Ok(Redirect::to(url.as_str()))
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    aud: Value,
    exp: i64,
    nonce: Option<String>,
    email: Option<String>,
    email_verified: Option<Value>,
    name: Option<String>,
}

fn unauthorized(msg: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::UNAUTHORIZED, msg)
}

/// The provider sends the browser back here with a code.
async fn callback(
    State(s): State<AppState>,
    Query(q): Query<CallbackQuery>,
) -> ApiResult<Response> {
    if let Some(e) = q.error {
        return Err(unauthorized(format!(
            "sign-in failed: {e} {}",
            q.error_description.unwrap_or_default()
        )));
    }
    let (Some(code), Some(state)) = (q.code, q.state) else {
        return Err(ApiError::bad_request("missing code or state"));
    };
    let p = PENDING
        .lock()
        .expect("pending lock")
        .remove(&state)
        .filter(|p| p.at.elapsed() < Duration::from_secs(600))
        .ok_or_else(|| unauthorized("this sign-in expired or was already used; try again"))?;
    let o = sso(&s)
        .oidc
        .clone()
        .ok_or_else(|| ApiError::not_found("OIDC sign-in isn't set up"))?;
    let http = http();
    let d = discover(&http, &o).await?;
    let mut form = vec![
        ("grant_type", "authorization_code".to_owned()),
        ("code", code),
        ("redirect_uri", p.redirect_uri.clone()),
        ("client_id", o.client_id.clone()),
        ("code_verifier", p.verifier.clone()),
    ];
    if let Some(var) = &o.client_secret_env {
        let secret = std::env::var(var)
            .map_err(|_| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, format!("set {var}")))?;
        form.push(("client_secret", secret));
    }
    let r = http
        .post(&d.token_endpoint)
        .form(&form)
        .send()
        .await
        .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, format!("token endpoint: {e}")))?;
    let status = r.status();
    let body: Value = r.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(unauthorized(format!(
            "the provider refused the sign-in: {}",
            body["error_description"]
                .as_str()
                .or(body["error"].as_str())
                .unwrap_or("")
        )));
    }
    let id_token = body["id_token"]
        .as_str()
        .ok_or_else(|| unauthorized("the provider sent no ID token"))?;
    let claims = parse_id_token(id_token)?;
    check_claims(&claims, &d, &o, &p.nonce)?;
    if claims
        .email_verified
        .as_ref()
        .is_some_and(|v| v == false || v == "false")
    {
        return Err(unauthorized("your email isn't verified with the provider"));
    }
    let email = claims
        .email
        .clone()
        .ok_or_else(|| unauthorized("the provider didn't share your email (scope `email`)"))?;
    allowed(&s, &email)?;
    let user = s
        .orgs
        .control()
        .sign_in_user(
            &format!("{}|{}", claims.iss, claims.sub),
            &email,
            claims.name.as_deref(),
        )
        .await?;
    let user = after_sign_in(&s, user).await?;
    let token = format!("{LOGIN_PREFIX}{}", random_hex(24));
    let hours = sso(&s).session_hours.unwrap_or(168).max(1);
    s.orgs
        .control()
        .create_login(
            &keys::hash(&token),
            &user.id,
            chrono::Utc::now() + chrono::Duration::hours(hours as i64),
        )
        .await?;
    let secure = if p.redirect_uri.starts_with("https://") {
        "; Secure"
    } else {
        ""
    };
    let set = format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{secure}",
        hours * 3600
    );
    let mut resp = Redirect::to(&p.return_to).into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&set)
            .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
    );
    Ok(resp)
}

fn parse_id_token(t: &str) -> ApiResult<Claims> {
    let payload = t
        .split('.')
        .nth(1)
        .ok_or_else(|| unauthorized("the ID token isn't a JWT"))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .map_err(|_| unauthorized("the ID token isn't a JWT"))?;
    serde_json::from_slice(&bytes).map_err(|e| unauthorized(format!("the ID token: {e}")))
}

fn check_claims(c: &Claims, d: &Discovery, o: &OidcSettings, nonce: &str) -> ApiResult<()> {
    if c.iss.trim_end_matches('/') != d.issuer.trim_end_matches('/') {
        return Err(unauthorized("the ID token is from another issuer"));
    }
    let aud_ok = match &c.aud {
        Value::String(a) => *a == o.client_id,
        Value::Array(a) => a.iter().any(|v| v.as_str() == Some(o.client_id.as_str())),
        _ => false,
    };
    if !aud_ok {
        return Err(unauthorized("the ID token is for another client"));
    }
    if c.exp < chrono::Utc::now().timestamp() - 60 {
        return Err(unauthorized("the ID token has expired"));
    }
    if c.nonce.as_deref() != Some(nonce) {
        return Err(unauthorized("the ID token doesn't match this sign-in"));
    }
    Ok(())
}

fn allowed(s: &AppState, email: &str) -> ApiResult<()> {
    let domains = &sso(s).allowed_domains;
    let domain = email.rsplit_once('@').map(|(_, d)| d.to_ascii_lowercase());
    if !domains.is_empty()
        && !domain.is_some_and(|d| domains.iter().any(|a| a.eq_ignore_ascii_case(&d)))
    {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            format!("{email} can't sign in to this server"),
        ));
    }
    Ok(())
}

/// Server admins from `[server.sso] admins`, and `default_role` for
/// people nobody invited.
async fn after_sign_in(s: &AppState, user: User) -> ApiResult<User> {
    if user.disabled {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "this account is disabled",
        ));
    }
    let c = sso(s);
    let control = s.orgs.control();
    let mine = control.user_memberships(&user.id).await?;
    let in_default = mine
        .iter()
        .find(|m| m.org_id == DEFAULT_ORG)
        .map(|m| m.role);
    if c.admins.iter().any(|a| a.eq_ignore_ascii_case(&user.email)) {
        if in_default != Some(Role::Owner) {
            control
                .set_membership(DEFAULT_ORG, &user.id, Role::Owner)
                .await?;
        }
    } else if mine.is_empty()
        && let Some(role) = c.default_role
    {
        control.set_membership(DEFAULT_ORG, &user.id, role).await?;
    }
    Ok(user)
}

/// The user a trusted proxy signed in, when `[server.sso.trusted_header]`
/// is set and the request carries it.
// Middleware answers with whole responses (a Retry-After, say).
#[allow(clippy::result_large_err)]
pub(crate) async fn trusted_user(s: &AppState, h: &HeaderMap) -> Result<Option<User>, Response> {
    let Some(t) = &sso(s).trusted_header else {
        return Ok(None);
    };
    let Some(email) = h
        .get(t.email.as_str())
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|e| e.contains('@'))
    else {
        return Ok(None);
    };
    allowed(s, email).map_err(IntoResponse::into_response)?;
    let name = t
        .name
        .as_deref()
        .and_then(|n| h.get(n))
        .and_then(|v| v.to_str().ok());
    let control = s.orgs.control();
    let user = match control.find_user_by_email(email).await {
        Ok(Some(u)) => u,
        Ok(None) => control
            .sign_in_user(&format!("proxy|{}", email.to_lowercase()), email, name)
            .await
            .map_err(|e| ApiError::from(e).into_response())?,
        Err(e) => return Err(ApiError::from(e).into_response()),
    };
    after_sign_in(s, user)
        .await
        .map(Some)
        .map_err(IntoResponse::into_response)
}

/// Sign out this browser.
async fn logout(State(s): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    if let Some(t) = cookie(&headers, SESSION_COOKIE) {
        s.orgs.control().delete_login(&keys::hash(&t)).await?;
    }
    let mut resp = Json(Done { ok: true }).into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static("gai_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"),
    );
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_paths_stay_on_this_server() {
        assert_eq!(safe_return(Some("/runs")), "/runs");
        assert_eq!(safe_return(Some("//evil.example")), "/");
        assert_eq!(safe_return(Some("https://evil.example")), "/");
        assert_eq!(safe_return(Some("/\\evil")), "/");
        assert_eq!(safe_return(None), "/");
    }
}
