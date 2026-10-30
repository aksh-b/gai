//! Who is calling (an API key, a signed-in user, or nobody when auth is
//! open), which organization they work in, what they may do there, rate
//! limits and budgets.

use std::num::NonZeroU32;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use gai::AuthMode;
use gai::store::{ApiKey, DEFAULT_ORG, Org, Role, Scope, SharedStore, User, keys};
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use schemars::JsonSchema;
use serde::Serialize;

use crate::AppState;
use crate::error::ApiError;

/// The cookie holding a dashboard sign-in.
pub const SESSION_COOKIE: &str = "gai_session";
/// Picks the organization a signed-in user (or an open server) works in.
pub const ORG_HEADER: &str = "x-gai-org";

/// Who is calling.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct Principal {
    /// The key id, the user id, or `anonymous` when authentication is open.
    pub id: String,
    /// The key's name, or the user's email.
    pub name: String,
    pub scopes: Vec<Scope>,
    /// The organization this request works in.
    pub org_id: String,
    pub org_slug: String,
    /// A signed-in user's role there.
    pub role: Option<Role>,
    pub user_id: Option<String>,
    /// Set for a team's key.
    pub team_id: Option<String>,
    /// The teams the caller acts for: a user's teams in the organization,
    /// or a team key's team.
    pub teams: Vec<String>,
    #[serde(skip)]
    pub key: Option<ApiKey>,
    /// What the caller may find in knowledge with an `acl`; None for
    /// admins, who find everything.
    #[serde(skip)]
    pub access: Option<gai::memory::Access>,
}

impl Principal {
    fn anonymous(org: &Org) -> Self {
        Self {
            id: "anonymous".into(),
            name: "anonymous".into(),
            scopes: vec![Scope::Admin],
            org_id: org.id.clone(),
            org_slug: org.slug.clone(),
            role: None,
            user_id: None,
            team_id: None,
            teams: Vec::new(),
            key: None,
            access: None,
        }
    }

    fn from_key(k: ApiKey, org: &Org) -> Self {
        Self {
            id: k.id.clone(),
            name: k.name.clone(),
            scopes: k.scopes.clone(),
            org_id: org.id.clone(),
            org_slug: org.slug.clone(),
            role: None,
            user_id: k.user_id.clone(),
            team_id: k.team_id.clone(),
            teams: k.team_id.iter().cloned().collect(),
            key: Some(k),
            access: None,
        }
    }

    fn from_user(u: &User, org: &Org, role: Option<Role>, teams: Vec<String>) -> Self {
        Self {
            id: u.id.clone(),
            name: u.email.clone(),
            scopes: role.map(|r| vec![r.scope()]).unwrap_or_default(),
            org_id: org.id.clone(),
            org_slug: org.slug.clone(),
            role,
            user_id: Some(u.id.clone()),
            team_id: None,
            teams,
            key: None,
            access: None,
        }
    }

    /// Put who started a run on it: the key it is charged to, and what it
    /// may find in knowledge.
    pub fn stamp(&self, meta: &mut serde_json::Map<String, serde_json::Value>) {
        if let Some(id) = self.key_id() {
            meta.insert("key_id".into(), serde_json::json!(id));
        }
        if let Some(a) = &self.access {
            meta.insert("access".into(), serde_json::json!(a));
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

    /// An organization admin (or owner, or an admin key), not limited to
    /// a team.
    pub fn is_admin(&self) -> bool {
        Scope::allows(&self.scopes, Scope::Admin) && self.team_id.is_none()
    }

    pub fn is_owner(&self) -> bool {
        self.role == Some(Role::Owner) || (self.role.is_none() && self.is_admin())
    }

    /// Whether the caller may change a resource owned by `team` (none:
    /// the whole organization's).
    pub fn can_change(&self, team: Option<&str>) -> bool {
        match team {
            _ if self.is_admin() => true,
            None => true,
            Some(t) => self.teams.iter().any(|m| m == t),
        }
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

    /// Whether requests without a key are let in right now. `store` is the
    /// main database, where every key lives.
    pub async fn open(&self, store: &SharedStore) -> bool {
        match self.mode {
            AuthMode::Off => true,
            AuthMode::Required => false,
            AuthMode::Auto => self.local && !store.has_keys().await.unwrap_or(true),
        }
    }
}

/// The scope a request needs, from its method and path. Each scope
/// includes the ones below it: admin, build, run, read.
pub fn required_scope(method: &Method, path: &str) -> Scope {
    // A voice session runs the agent, though it opens with a GET.
    let write = (*method != Method::GET && *method != Method::HEAD) || path == "/v1/voice";
    // Saving or deleting a workflow is build; running or validating one is not.
    let workflow_build = path.starts_with("/v1/workflows")
        && !path.ends_with("/runs")
        && path != "/v1/workflows/run"
        && path != "/v1/workflows/validate";
    let build = (path.starts_with("/v1/agents")
        || (path.starts_with("/v1/personas") && path != "/v1/personas/render")
        || (path.starts_with("/v1/prompts") && path != "/v1/prompts/render")
        || (path.starts_with("/v1/triggers") && path != "/v1/triggers/preview")
        || path.starts_with("/v1/skills")
        || path.starts_with("/v1/templates")
        || path.starts_with("/v1/sources")
        || path.starts_with("/v1/owners")
        || (path.starts_with("/v1/dashboards") && !path.ends_with("/refresh"))
        // Saving or deleting eval sets and evaluations; running one is `run`.
        || (path.starts_with("/v1/evals") && !path.ends_with("/runs"))
        // Applying what the prompt optimizer found.
        || path.starts_with("/v1/optimizations")
        // Saving apps and their keys; running one is `run`.
        || (path.starts_with("/v1/apps") && !path.ends_with("/run"))
        || path.starts_with("/v1/eval-runs/")
        || workflow_build)
        && write;
    let admin_write = (path.starts_with("/v1/mcp")
        || path.starts_with("/v1/settings")
        || path.starts_with("/v1/providers")
        || path.starts_with("/v1/bundles")
        // Publishing, installing and choosing registries; rating is `run`.
        || (path.starts_with("/v1/marketplace") && !path.ends_with("/ratings"))
        || path.starts_with("/v1/members")
        || path.starts_with("/v1/teams")
        || path.starts_with("/v1/orgs")
        // Rules and their tests; a browser subscribing to push is `run`.
        || path.starts_with("/v1/notifications/rules")
        || (path.starts_with("/v1/connections") && !path.ends_with("/query")))
        && write;
    // The audit log, forgetting users, retention, keys and secrets: admin,
    // reads included.
    let admin = path.starts_with("/v1/keys")
        || path.starts_with("/v1/secrets")
        || path.starts_with("/v1/audit")
        || path.starts_with("/v1/users")
        || path.starts_with("/v1/retention");
    if admin || admin_write {
        Scope::Admin
    } else if build {
        Scope::Build
    } else if !write {
        Scope::Read
    } else {
        Scope::Run
    }
}

/// The resource a build-level request changes, for team ownership:
/// `(kind, name)`.
pub fn owned_resource(path: &str) -> Option<(&'static str, String)> {
    let rest = path.strip_prefix("/v1/")?;
    let mut parts = rest.split('/');
    let kind = match parts.next()? {
        "agents" => "agent",
        "workflows" => "workflow",
        "apps" => "app",
        "personas" => "persona",
        "prompts" => "prompt",
        "skills" => "skill",
        "triggers" => "trigger",
        "sources" => "source",
        "evals" => "eval",
        "collections" => "collection",
        _ => return None,
    };
    let name = parts.next().filter(|n| !n.is_empty())?;
    let name = urlencoding_decode(name);
    Some((kind, name))
}

/// The resource a `POST /v1/<kinds>` saves, from its JSON body's `name`
/// (or the `name:` of its `{"yaml": ...}`). The body is put back.
async fn body_resource(path: &str, req: &mut Request) -> Option<(&'static str, String)> {
    let kind = match path {
        "/v1/agents" => "agent",
        "/v1/workflows" => "workflow",
        "/v1/apps" => "app",
        "/v1/personas" => "persona",
        "/v1/prompts" => "prompt",
        "/v1/evals" => "eval",
        "/v1/sources" => "source",
        _ => return None,
    };
    let json = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if !json {
        return None;
    }
    let body = std::mem::take(req.body_mut());
    let bytes = axum::body::to_bytes(body, 4 << 20)
        .await
        .unwrap_or_default();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
    *req.body_mut() = axum::body::Body::from(bytes);
    let name = match v.get("yaml").and_then(|y| y.as_str()) {
        Some(y) => serde_yaml::from_str::<serde_json::Value>(y).ok()?["name"]
            .as_str()?
            .to_owned(),
        None => v["name"].as_str()?.to_owned(),
    };
    Some((kind, name))
}

fn urlencoding_decode(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={s}").as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| s.to_owned())
}

/// The API key a request carries (`Authorization: Bearer` or `x-api-key`;
/// browsers opening a WebSocket, which can't set headers, offer it as the
/// subprotocol `bearer.<key>`).
pub(crate) fn header_token(h: &HeaderMap) -> Option<String> {
    h.get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.strip_prefix("Bearer ")
                .or_else(|| v.strip_prefix("bearer "))
        })
        .or_else(|| h.get("x-api-key").and_then(|v| v.to_str().ok()))
        .map(|t| t.trim().to_owned())
        .or_else(|| {
            h.get(header::SEC_WEBSOCKET_PROTOCOL)
                .and_then(|v| v.to_str().ok())?
                .split(',')
                .find_map(|p| p.trim().strip_prefix("bearer.").map(str::to_owned))
        })
        .filter(|t| !t.is_empty())
}

/// A cookie's value.
pub(crate) fn cookie(h: &HeaderMap, name: &str) -> Option<String> {
    h.get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|c| c.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_owned())
        .filter(|v| !v.is_empty())
}

/// The organization a request names: the header, or `?org=` (webhooks and
/// OAuth callbacks, which can't set headers).
fn org_hint(req: &Request) -> Option<String> {
    req.headers()
        .get(ORG_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .or_else(|| {
            url::form_urlencoded::parse(req.uri().query().unwrap_or_default().as_bytes())
                .find(|(k, _)| k == "org")
                .map(|(_, v)| v.into_owned())
        })
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn deny(status: StatusCode, msg: impl Into<String>) -> Response {
    ApiError::new(status, msg).into_response()
}

/// A browser's write must come from this server's own pages: cookies ride
/// along with requests other sites trigger.
fn same_origin(h: &HeaderMap) -> bool {
    if let Some(origin) = h.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        let host = h
            .get("x-forwarded-host")
            .or_else(|| h.get(header::HOST))
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        let origin_host = origin.split_once("://").map(|(_, r)| r).unwrap_or(origin);
        return !host.is_empty() && origin_host.eq_ignore_ascii_case(host);
    }
    h.get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .is_none_or(|v| v != "cross-site")
}

/// A signed-in user's place in an organization: the one they asked for,
/// else the first they belong to.
// Middleware answers with whole responses (a Retry-After, say).
#[allow(clippy::result_large_err)]
async fn user_principal(
    s: &AppState,
    user: &User,
    wanted: Option<&str>,
) -> Result<Principal, Response> {
    let control = s.orgs.control();
    let memberships = control
        .user_memberships(&user.id)
        .await
        .map_err(|e| ApiError::from(e).into_response())?;
    let chosen = match wanted {
        Some(w) => Some(
            memberships
                .iter()
                .find(|m| m.org_slug == w || m.org_id == w)
                .ok_or_else(|| {
                    deny(StatusCode::FORBIDDEN, format!("you aren't a member of {w}"))
                })?,
        ),
        None => memberships
            .iter()
            .find(|m| m.org_id == DEFAULT_ORG)
            .or(memberships.first()),
    };
    let Some(m) = chosen else {
        // Signed in but in no organization: only /v1/me and signing out.
        let org = s.org.as_ref().clone();
        return Ok(Principal::from_user(user, &org, None, Vec::new()));
    };
    let org = control
        .get_org(&m.org_id)
        .await
        .map_err(|e| ApiError::from(e).into_response())?
        .ok_or_else(|| deny(StatusCode::FORBIDDEN, "that organization is gone"))?;
    let teams = control
        .list_teams(&org.id)
        .await
        .map_err(|e| ApiError::from(e).into_response())?
        .into_iter()
        .filter(|t| t.members.contains(&user.id))
        .map(|t| t.id)
        .collect();
    Ok(Principal::from_user(user, &org, Some(m.role), teams))
}

/// Who is calling, from a key, a sign-in cookie, a trusted proxy header,
/// or nobody (when authentication is open).
// Middleware answers with whole responses (a Retry-After, say).
#[allow(clippy::result_large_err)]
async fn identify(
    s: &AppState,
    headers: &HeaderMap,
    wanted: Option<String>,
) -> Result<(Principal, bool), Response> {
    let control = s.orgs.control();
    if let Some(t) = header_token(headers) {
        // A sign-in token works as a bearer token too (scripts, tests).
        if t.starts_with(crate::sso::LOGIN_PREFIX) {
            return match control.find_login(&keys::hash(&t)).await {
                Ok(Some((_, u))) => Ok((user_principal(s, &u, wanted.as_deref()).await?, false)),
                Ok(None) => Err(deny(
                    StatusCode::UNAUTHORIZED,
                    "this sign-in has ended; sign in again",
                )),
                Err(e) => Err(ApiError::from(e).into_response()),
            };
        }
        let k = match control.find_key(&keys::hash(&t)).await {
            Ok(Some(k)) => k,
            Ok(None) => return Err(deny(StatusCode::UNAUTHORIZED, "invalid or revoked API key")),
            Err(e) => return Err(ApiError::from(e).into_response()),
        };
        let org = control
            .get_org(&k.org_id)
            .await
            .map_err(|e| ApiError::from(e).into_response())?
            .ok_or_else(|| deny(StatusCode::UNAUTHORIZED, "this key's organization is gone"))?;
        if let Some(w) = &wanted
            && *w != org.slug
            && *w != org.id
        {
            return Err(deny(
                StatusCode::FORBIDDEN,
                format!("this key belongs to {}, not {w}", org.slug),
            ));
        }
        return Ok((Principal::from_key(k, &org), false));
    }
    if let Some(t) = cookie(headers, SESSION_COOKIE) {
        match control.find_login(&keys::hash(&t)).await {
            Ok(Some((_, u))) => return Ok((user_principal(s, &u, wanted.as_deref()).await?, true)),
            Ok(None) => {}
            Err(e) => return Err(ApiError::from(e).into_response()),
        }
    }
    if let Some(u) = crate::sso::trusted_user(s, headers).await? {
        return Ok((user_principal(s, &u, wanted.as_deref()).await?, false));
    }
    if s.auth.open(control).await {
        let org = match &wanted {
            Some(w) => control
                .get_org(w)
                .await
                .map_err(|e| ApiError::from(e).into_response())?
                .ok_or_else(|| deny(StatusCode::NOT_FOUND, format!("no organization {w}")))?,
            None => s.org.as_ref().clone(),
        };
        return Ok((Principal::anonymous(&org), false));
    }
    Err(deny(
        StatusCode::UNAUTHORIZED,
        "an API key is required: send `Authorization: Bearer <key>` (create one with `gai keys create`), or sign in",
    ))
}

pub async fn authenticate(State(s): State<AppState>, mut req: Request, next: Next) -> Response {
    match authorize(&s, &mut req).await {
        Ok(()) => next.run(req).await,
        Err(resp) => resp,
    }
}

// Middleware answers with whole responses (a Retry-After, say).
#[allow(clippy::result_large_err)]
async fn authorize(s: &AppState, req: &mut Request) -> Result<(), Response> {
    let path = req.uri().path().to_owned();
    // The OAuth callback is reached by a browser redirect, without a key;
    // it only completes a sign-in an admin started (matched by `state`).
    // Webhooks carry their trigger's signature instead of a key.
    // Setup status and claiming check for themselves who may use them, and
    // signing in is how a browser gets credentials.
    let public = path == "/v1/health"
        || path == "/v1/setup/status"
        || path == "/v1/setup/claim"
        || path == "/v1/mcp/oauth/callback"
        || path.starts_with("/v1/auth/")
        || path.starts_with("/v1/webhooks/");
    // Agent cards are public: they are how other agents find gai.
    let a2a = (path == "/a2a" || path.starts_with("/a2a/")) && !path.contains("/.well-known/");
    let mcp = path == "/mcp" || path.starts_with("/mcp/");
    let guarded =
        (path.starts_with("/v1/") || path.starts_with("/openai/") || mcp || a2a) && !public;
    if req.method() == Method::OPTIONS || !(guarded || public) {
        return Ok(());
    }
    if public {
        // Webhooks and OAuth callbacks of another organization name it.
        if !path.starts_with("/v1/auth/") {
            enter_named_org(s, req).await?;
        }
        return Ok(());
    }
    let headers = req.headers().clone();
    // Using an app takes its own key, or no credentials at all for a
    // public app; the app's handlers check which.
    let app_use = crate::apps::is_app_use(&path);
    let token = header_token(&headers);
    if app_use
        && token
            .as_deref()
            .is_some_and(|t| t.starts_with(crate::apps::APP_KEY_PREFIX))
    {
        return enter_named_org(s, req).await;
    }
    let (principal, by_cookie) = match identify(s, &headers, org_hint(req)).await {
        Ok(found) => found,
        Err(_) if app_use && token.is_none() => return enter_named_org(s, req).await,
        Err(e) => return Err(e),
    };
    if by_cookie && !matches!(*req.method(), Method::GET | Method::HEAD) && !same_origin(&headers) {
        return Err(deny(StatusCode::FORBIDDEN, "cross-site request refused"));
    }
    let signed_in_only = path == "/v1/me" || path == "/v1/orgs";
    if principal.scopes.is_empty() && !signed_in_only {
        return Err(deny(
            StatusCode::FORBIDDEN,
            format!(
                "{} isn't a member of any organization yet: ask an admin to add you",
                principal.name
            ),
        ));
    }
    if mcp && principal.org_id != DEFAULT_ORG {
        return Err(deny(
            StatusCode::FORBIDDEN,
            "the /mcp endpoint serves the default organization only",
        ));
    }
    let needed = required_scope(req.method(), &path);
    if !signed_in_only && !Scope::allows(&principal.scopes, needed) {
        let what = match (principal.role, principal.key.is_some()) {
            (Some(r), _) => format!("your role ({r:?}) doesn't allow this").to_lowercase(),
            _ => format!("this key lacks the {needed:?} scope").to_lowercase(),
        };
        return Err(deny(StatusCode::FORBIDDEN, what));
    }
    let org = s
        .orgs
        .control()
        .get_org(&principal.org_id)
        .await
        .map_err(|e| ApiError::from(e).into_response())?
        .ok_or_else(|| deny(StatusCode::FORBIDDEN, "that organization is gone"))?;
    // Team ownership: builders and team keys change only their teams' things.
    let target = match owned_resource(&path) {
        Some(t) => Some(t),
        // Saving by POSTing the whole spec names it in the body.
        None if needed == Scope::Build
            && !principal.is_admin()
            && *req.method() == Method::POST =>
        {
            body_resource(&path, req).await
        }
        None => None,
    };
    if needed == Scope::Build
        && !principal.is_admin()
        && let Some((kind, name)) = target
    {
        let owner = s
            .orgs
            .control()
            .get_owner(&org.id, kind, &name)
            .await
            .map_err(|e| ApiError::from(e).into_response())?;
        if !principal.can_change(owner.as_deref()) {
            let team = match &owner {
                Some(t) => s
                    .orgs
                    .control()
                    .get_team(&org.id, t)
                    .await
                    .ok()
                    .flatten()
                    .map(|t| t.name)
                    .unwrap_or_else(|| t.clone()),
                None => String::new(),
            };
            return Err(deny(
                StatusCode::FORBIDDEN,
                format!("{kind} {name} belongs to the {team} team"),
            ));
        }
    }
    if let Some(limiter) = &s.auth.limiter
        && let Err(not_until) = limiter.check_key(&principal.id)
    {
        return Err(too_many(not_until.wait_time_from(
            governor::clock::Clock::now(&governor::clock::DefaultClock::default()),
        )));
    }
    if let Some(wait) = s.orgs.rate_limited(&org) {
        return Err(too_many(wait));
    }
    // Starting work spends the organization's budget.
    if needed == Scope::Run
        && let Some(e) = s.orgs.over_budget(s, &org).await
    {
        return Err(e.into_response());
    }
    if let Some(id) = principal.key_id().map(str::to_owned) {
        let store = s.orgs.control().clone();
        tokio::spawn(async move {
            let _ = store.touch_key(&id).await;
        });
    }
    if org.id != DEFAULT_ORG {
        let state = s
            .orgs
            .state(s, &org)
            .await
            .map_err(IntoResponse::into_response)?;
        req.extensions_mut().insert(state);
    }
    let mut principal = principal;
    principal.access = access(s.orgs.control(), &principal).await;
    req.extensions_mut().insert(principal);
    Ok(())
}

/// Serve a request without a caller in the organization it names (the
/// default one when it names none).
// Middleware answers with whole responses.
#[allow(clippy::result_large_err)]
async fn enter_named_org(s: &AppState, req: &mut Request) -> Result<(), Response> {
    if let Some(slug) = org_hint(req) {
        let org = s
            .orgs
            .control()
            .get_org(&slug)
            .await
            .map_err(|e| ApiError::from(e).into_response())?
            .ok_or_else(|| deny(StatusCode::NOT_FOUND, format!("no organization {slug}")))?;
        let state = s
            .orgs
            .state(s, &org)
            .await
            .map_err(IntoResponse::into_response)?;
        req.extensions_mut().insert(state);
    }
    Ok(())
}

/// What a non-admin may find in knowledge with an `acl`: who they are,
/// their teams, and their role and every role below it.
async fn access(control: &SharedStore, p: &Principal) -> Option<gai::memory::Access> {
    if p.is_admin() {
        return None;
    }
    let mut identities = Vec::new();
    if p.key.is_none() && p.user_id.is_some() {
        identities.push(format!("user:{}", p.name.to_lowercase()));
    }
    if !p.teams.is_empty() {
        for t in control.list_teams(&p.org_id).await.unwrap_or_default() {
            if p.teams.contains(&t.id) {
                identities.push(format!("team:{}", t.name));
            }
        }
    }
    let role = p.role.unwrap_or(match Scope::highest(&p.scopes) {
        Scope::Read => Role::Viewer,
        Scope::Run => Role::Member,
        Scope::Build => Role::Builder,
        Scope::Admin => Role::Admin,
    });
    for r in [
        Role::Viewer,
        Role::Member,
        Role::Builder,
        Role::Admin,
        Role::Owner,
    ] {
        if r <= role {
            identities.push(format!("role:{}", format!("{r:?}").to_lowercase()));
        }
    }
    Some(gai::memory::Access { identities })
}

fn too_many(wait: std::time::Duration) -> Response {
    let mut resp = deny(StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded");
    if let Ok(v) = (wait.as_secs() + 1).to_string().parse() {
        resp.headers_mut().insert(header::RETRY_AFTER, v);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_by_route() {
        assert_eq!(required_scope(&Method::GET, "/v1/runs"), Scope::Read);
        assert_eq!(required_scope(&Method::POST, "/v1/runs"), Scope::Run);
        assert_eq!(required_scope(&Method::GET, "/v1/keys"), Scope::Admin);
        assert_eq!(required_scope(&Method::POST, "/v1/agents"), Scope::Build);
        assert_eq!(required_scope(&Method::GET, "/v1/agents"), Scope::Read);
        assert_eq!(required_scope(&Method::POST, "/v1/personas"), Scope::Build);
        assert_eq!(
            required_scope(&Method::PUT, "/v1/personas/defaults"),
            Scope::Build
        );
        assert_eq!(required_scope(&Method::GET, "/v1/personas"), Scope::Read);
        assert_eq!(
            required_scope(&Method::POST, "/v1/personas/render"),
            Scope::Run
        );
        assert_eq!(required_scope(&Method::POST, "/v1/triggers"), Scope::Build);
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
        assert_eq!(
            required_scope(&Method::PUT, "/v1/members/a@b.c"),
            Scope::Admin
        );
        assert_eq!(required_scope(&Method::GET, "/v1/members"), Scope::Read);
        assert_eq!(
            required_scope(&Method::POST, "/v1/notifications/rules"),
            Scope::Admin
        );
        assert_eq!(
            required_scope(&Method::POST, "/v1/notifications/push"),
            Scope::Run
        );
        assert_eq!(required_scope(&Method::POST, "/v1/apps"), Scope::Build);
        assert_eq!(required_scope(&Method::POST, "/v1/prompts"), Scope::Build);
        assert_eq!(
            required_scope(&Method::POST, "/v1/evals/support/optimize"),
            Scope::Build
        );
        assert_eq!(
            required_scope(&Method::POST, "/v1/optimizations/job_1/apply"),
            Scope::Build
        );
        assert_eq!(
            required_scope(&Method::POST, "/v1/prompts/render"),
            Scope::Run
        );
        assert_eq!(
            required_scope(&Method::POST, "/v1/marketplace/install"),
            Scope::Admin
        );
        assert_eq!(
            required_scope(&Method::POST, "/v1/marketplace/listings/kit/ratings"),
            Scope::Run
        );
        assert_eq!(
            required_scope(&Method::GET, "/v1/marketplace/browse"),
            Scope::Read
        );
        assert_eq!(
            required_scope(&Method::POST, "/v1/apps/intake/key"),
            Scope::Build
        );
        assert_eq!(
            required_scope(&Method::POST, "/v1/apps/intake/run"),
            Scope::Run
        );
        assert_eq!(
            required_scope(&Method::GET, "/v1/apps/intake/form"),
            Scope::Read
        );
        assert_eq!(required_scope(&Method::GET, "/v1/secrets"), Scope::Admin);
        assert_eq!(
            required_scope(&Method::PUT, "/v1/owners/agent/x"),
            Scope::Build
        );
    }

    #[test]
    fn owned_resources_by_route() {
        assert_eq!(
            owned_resource("/v1/agents/helper"),
            Some(("agent", "helper".into()))
        );
        assert_eq!(
            owned_resource("/v1/agents/helper/labels/prod"),
            Some(("agent", "helper".into()))
        );
        assert_eq!(owned_resource("/v1/agents"), None);
        assert_eq!(
            owned_resource("/v1/collections/my%20docs/documents"),
            Some(("collection", "my docs".into()))
        );
        assert_eq!(owned_resource("/v1/runs/x"), None);
    }
}
