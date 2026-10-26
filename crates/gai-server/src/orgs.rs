//! Organizations: each one but the default works in a gai instance of its
//! own (its own database and files), opened on first use and kept open.
//! Members, roles, teams, which team owns what, keys and secrets.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{FromRequestParts, Path};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::routing::{delete, get, put};
use axum::{Extension, Json, Router};
use chrono::{Datelike, TimeZone, Utc};
use gai::Gai;
use gai::agent::Runtime;
use gai::memory::sources::SECRET_PREFIX;
use gai::orchestra::Workflows;
use gai::store::{DEFAULT_ORG, Membership, Org, Role, SharedStore, Team};
use governor::{DefaultDirectRateLimiter, Quota, RateLimiter};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex as AsyncMutex;

use crate::AppState;
use crate::api::Done;
use crate::auth::{Principal, charge};
use crate::error::{ApiError, ApiResult};

/// What one organization works with.
#[derive(Clone)]
pub struct Instance {
    pub gai: Arc<Gai>,
    pub rt: Runtime,
    pub wf: Workflows,
}

/// Open the runtime and its background work: MCP servers, unfinished
/// runs, the run queue, triggers and charging keys. `control` is the main
/// database, where keys live.
pub(crate) async fn start(gai: Arc<Gai>, control: SharedStore) -> Result<Instance, gai::Error> {
    let rt = gai.runtime().await?.clone();
    // Before recovery: the workflow engine must be registered to pick up
    // its own unfinished and queued runs.
    let wf = gai.workflows().await?.clone();
    // A server is long-lived: connect MCP servers now so their status and
    // tools show up, and pick up runs a previous process left unfinished.
    rt.ensure_mcp();
    let resumed = rt.recover().await;
    if !resumed.is_empty() {
        tracing::info!(org = gai.org().unwrap_or(DEFAULT_ORG), runs = ?resumed, "resumed unfinished runs");
    }
    rt.start_queue(gai.config().server.max_background_runs);
    // Schedules and file watchers start runs on their own from now on.
    gai.triggers().await?.start_loops();
    // Runs started with an API key are charged to it when they end, however
    // they were started and whichever process finishes them.
    let store = rt.store().clone();
    rt.set_finish_hook(
        "charge-key",
        Arc::new(move |o: gai::agent::RunOutcome| {
            let store = store.clone();
            let control = control.clone();
            Box::pin(async move {
                if let Ok(Some(run)) = store.get_run(&o.run_id).await
                    // A workflow's cost is its agent runs', which are charged themselves.
                    && run.metadata["kind"] != "workflow"
                    && let Some(key) = run.metadata["key_id"].as_str()
                {
                    charge(&control, Some(key), o.cost).await;
                }
            })
        }),
    );
    if gai.org().is_some() {
        crate::governance::start_retention(gai.clone());
        crate::sources::start_scheduler(gai.clone());
        crate::data::start_scheduler(gai.clone());
        crate::recordings::recover(gai.clone());
    }
    Ok(Instance { gai, rt, wf })
}

/// Every organization's instance, opened on first use.
#[derive(Clone)]
pub struct Orgs {
    inner: Arc<OrgsInner>,
}

struct OrgsInner {
    root: Instance,
    open: AsyncMutex<HashMap<String, Instance>>,
    /// Spend this month per organization, refreshed every minute.
    spend: Mutex<HashMap<String, (Instant, f64)>>,
    limiters: Mutex<HashMap<String, (u32, Arc<DefaultDirectRateLimiter>)>>,
}

impl Orgs {
    pub(crate) fn new(root: Instance) -> Self {
        Self {
            inner: Arc::new(OrgsInner {
                root,
                open: AsyncMutex::default(),
                spend: Mutex::default(),
                limiters: Mutex::default(),
            }),
        }
    }

    /// The main database: the directory, every key, and the default
    /// organization's data.
    pub fn control(&self) -> &SharedStore {
        self.inner.root.rt.store()
    }

    /// The organization's instance, opening it if needed.
    pub async fn instance(&self, org: &Org) -> Result<Instance, ApiError> {
        if org.id == DEFAULT_ORG {
            return Ok(self.inner.root.clone());
        }
        let mut open = self.inner.open.lock().await;
        if let Some(i) = open.get(&org.id) {
            return Ok(i.clone());
        }
        let gai = Arc::new(self.inner.root.gai.for_org(&org.slug)?);
        let i = start(gai, self.control().clone()).await?;
        tracing::info!(org = %org.slug, "organization opened");
        open.insert(org.id.clone(), i.clone());
        Ok(i)
    }

    /// The state handlers see for this organization.
    pub async fn state(&self, base: &AppState, org: &Org) -> Result<AppState, ApiError> {
        let i = self.instance(org).await?;
        Ok(AppState {
            gai: i.gai,
            rt: i.rt,
            wf: i.wf,
            auth: base.auth.clone(),
            orgs: self.clone(),
            org: Arc::new(org.clone()),
        })
    }

    /// Open every organization, so their schedules and watchers run.
    pub(crate) async fn open_all(&self) {
        let orgs = match self.control().list_orgs().await {
            Ok(o) => o,
            Err(e) => return tracing::warn!(error = %e, "could not list organizations"),
        };
        for org in orgs.iter().filter(|o| o.id != DEFAULT_ORG) {
            if let Err(e) = self.instance(org).await {
                tracing::warn!(org = %org.slug, error = %e.message(), "could not open organization");
            }
        }
    }

    /// Stop serving a deleted organization. Its data stays on disk.
    async fn close(&self, org_id: &str) {
        self.inner.open.lock().await.remove(org_id);
    }

    /// The organization's `limits.requests_per_minute`, if it is over it.
    pub fn rate_limited(&self, org: &Org) -> Option<Duration> {
        let n = limits(org).requests_per_minute.and_then(NonZeroU32::new)?;
        let limiter = {
            let mut map = self.inner.limiters.lock().expect("limiters lock");
            let entry = map
                .entry(org.id.clone())
                .or_insert_with(|| (n.get(), Arc::new(RateLimiter::direct(Quota::per_minute(n)))));
            if entry.0 != n.get() {
                *entry = (n.get(), Arc::new(RateLimiter::direct(Quota::per_minute(n))));
            }
            entry.1.clone()
        };
        limiter.check().err().map(|not_until| {
            not_until.wait_time_from(governor::clock::Clock::now(
                &governor::clock::DefaultClock::default(),
            ))
        })
    }

    /// Refuse new work once the organization has spent its monthly budget.
    pub async fn over_budget(&self, base: &AppState, org: &Org) -> Option<ApiError> {
        let budget = limits(org).budget_usd_per_month?;
        let cached = self
            .inner
            .spend
            .lock()
            .expect("spend lock")
            .get(&org.id)
            .filter(|(at, _)| at.elapsed() < Duration::from_secs(60))
            .map(|(_, v)| *v);
        let spent = match cached {
            Some(v) => v,
            None => {
                let state = self.state(base, org).await.ok()?;
                let v = month_spend(state.rt.store()).await;
                self.inner
                    .spend
                    .lock()
                    .expect("spend lock")
                    .insert(org.id.clone(), (Instant::now(), v));
                v
            }
        };
        (spent >= budget).then(|| {
            ApiError::new(
                StatusCode::PAYMENT_REQUIRED,
                format!(
                    "{} has used its ${budget:.2} budget for this month (${spent:.2} spent)",
                    org.name
                ),
            )
        })
    }
}

async fn month_spend(store: &SharedStore) -> f64 {
    let now = Utc::now();
    let start = Utc
        .with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
        .single()
        .unwrap_or(now);
    store
        .usage_totals(Some(start))
        .await
        .map(|t| t.iter().map(|u| u.cost).sum())
        .unwrap_or(0.0)
}

/// An organization's limits (in its settings, under `limits`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Limits {
    /// Stop starting runs once this much was spent this month (USD).
    pub budget_usd_per_month: Option<f64>,
    /// Requests per minute for the whole organization.
    pub requests_per_minute: Option<u32>,
}

/// How the dashboard looks for an organization (in its settings, under
/// `branding`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Branding {
    /// Shown instead of "gai".
    pub product_name: Option<String>,
    /// An https URL or a `data:` image.
    pub logo_url: Option<String>,
    /// A CSS colour for buttons and links, like `#5b5bd6`.
    pub accent: Option<String>,
}

/// What an organization's settings hold.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct OrgSettings {
    pub branding: Branding,
    pub limits: Limits,
}

fn limits(org: &Org) -> Limits {
    serde_json::from_value(org.settings["limits"].clone()).unwrap_or_default()
}

/// The state for the request's organization: what authentication picked,
/// else the default organization's.
pub struct Tenant(pub AppState);

impl FromRequestParts<AppState> for Tenant {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        Ok(Tenant(
            parts
                .extensions
                .get::<AppState>()
                .cloned()
                .unwrap_or_else(|| state.clone()),
        ))
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/orgs", get(list_orgs).post(create_org))
        .route(
            "/v1/orgs/{org}",
            get(get_org).patch(update_org).delete(delete_org),
        )
        .route("/v1/members", get(list_members))
        .route("/v1/members/{who}", put(set_member).delete(remove_member))
        .route("/v1/teams", get(list_teams).post(create_team))
        .route("/v1/teams/{team}", delete(delete_team))
        .route(
            "/v1/teams/{team}/members/{who}",
            put(add_team_member).delete(remove_team_member),
        )
        .route("/v1/owners", get(list_owners))
        .route("/v1/owners/{kind}/{name}", put(set_owner))
        .route("/v1/secrets", get(list_secrets))
        .route("/v1/secrets/{name}", put(put_secret).delete(delete_secret))
}

/// Slugs: lowercase letters, digits and `-`, starting with a letter.
fn valid_slug(s: &str) -> bool {
    (2..=40).contains(&s.len())
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A server admin: an admin of the default organization.
fn server_admin(p: &Principal) -> bool {
    p.org_id == DEFAULT_ORG && p.is_admin()
}

async fn org_or_404(s: &AppState, id_or_slug: &str) -> ApiResult<Org> {
    s.orgs
        .control()
        .get_org(id_or_slug)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("organization {id_or_slug}")))
}

#[derive(Serialize, JsonSchema)]
pub struct OrgView {
    #[serde(flatten)]
    pub org: Org,
    /// Your role there, when you are a member.
    pub role: Option<Role>,
}

/// Organizations you can work in. Server admins see them all.
async fn list_orgs(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
) -> ApiResult<Json<Vec<OrgView>>> {
    let control = s.orgs.control();
    let mine: Vec<Membership> = match &p.user_id {
        Some(u) if p.key.is_none() => control.user_memberships(u).await?,
        _ => Vec::new(),
    };
    let all = control.list_orgs().await?;
    Ok(Json(
        all.into_iter()
            .filter_map(|o| {
                let role = mine.iter().find(|m| m.org_id == o.id).map(|m| m.role);
                (role.is_some() || server_admin(&p) || o.id == p.org_id)
                    .then_some(OrgView { org: o, role })
            })
            .collect(),
    ))
}

#[derive(Deserialize, JsonSchema)]
pub struct NewOrg {
    /// Lowercase letters, digits and `-`.
    pub slug: String,
    pub name: String,
    /// Made its owner (invited if they haven't signed in yet).
    pub owner_email: Option<String>,
}

/// Create an organization (server admins).
async fn create_org(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Json(b): Json<NewOrg>,
) -> ApiResult<(StatusCode, Json<Org>)> {
    if !server_admin(&p) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "only admins of the default organization create organizations",
        ));
    }
    if !valid_slug(&b.slug) || b.slug == DEFAULT_ORG {
        return Err(ApiError::bad_request(
            "slugs use 2-40 lowercase letters, digits and '-', starting with a letter",
        ));
    }
    if b.name.trim().is_empty() {
        return Err(ApiError::bad_request("an organization needs a name"));
    }
    let control = s.orgs.control();
    let org = control.create_org(&b.slug, b.name.trim()).await?;
    if let Some(email) = &b.owner_email {
        let u = control.create_user(email, None).await?;
        control.set_membership(&org.id, &u.id, Role::Owner).await?;
    }
    if let Some(u) = &p.user_id {
        control.set_membership(&org.id, u, Role::Owner).await?;
    }
    s.orgs.instance(&org).await?;
    Ok((StatusCode::CREATED, Json(org)))
}

/// May the caller manage this organization (rename, settings, delete)?
fn manages(p: &Principal, org: &Org) -> bool {
    server_admin(p) || (p.org_id == org.id && p.is_owner())
}

async fn get_org(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<Json<Org>> {
    let org = org_or_404(&s, &id).await?;
    if org.id != p.org_id && !server_admin(&p) {
        return Err(ApiError::not_found(format!("organization {id}")));
    }
    Ok(Json(org))
}

#[derive(Deserialize, JsonSchema)]
pub struct OrgPatch {
    pub name: Option<String>,
    /// Replaces the settings: branding and limits.
    pub settings: Option<OrgSettings>,
}

async fn update_org(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Path(id): Path<String>,
    Json(b): Json<OrgPatch>,
) -> ApiResult<Json<Org>> {
    let org = org_or_404(&s, &id).await?;
    if !manages(&p, &org) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "only its owners change an organization",
        ));
    }
    // Owners brand their organization; limits are the server admins' to set.
    let settings = match b.settings {
        Some(mut new) => {
            if !server_admin(&p) {
                new.limits = limits(&org);
            }
            Some(serde_json::to_value(new).expect("settings"))
        }
        None => None,
    };
    let name = b.name.as_deref().map(str::trim).filter(|n| !n.is_empty());
    Ok(Json(
        s.orgs
            .control()
            .update_org(&org.id, name, settings.as_ref())
            .await?,
    ))
}

/// Delete an organization: its members, teams and keys go; it stops being
/// served. Its database and files stay where they are, for the operator to
/// archive or remove.
async fn delete_org(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<Json<Done>> {
    let org = org_or_404(&s, &id).await?;
    if !manages(&p, &org) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "only its owners delete an organization",
        ));
    }
    if !s.orgs.control().delete_org(&org.id).await? {
        return Err(ApiError::not_found(format!("organization {id}")));
    }
    s.orgs.close(&org.id).await;
    Ok(Json(Done { ok: true }))
}

async fn list_members(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
) -> ApiResult<Json<Vec<Membership>>> {
    Ok(Json(s.orgs.control().list_members(&p.org_id).await?))
}

#[derive(Deserialize, JsonSchema)]
pub struct SetMember {
    pub role: Role,
    pub name: Option<String>,
}

/// The member by email or user id.
async fn find_member(s: &AppState, org_id: &str, who: &str) -> ApiResult<Option<Membership>> {
    Ok(s.orgs
        .control()
        .list_members(org_id)
        .await?
        .into_iter()
        .find(|m| m.user_id == who || m.email.eq_ignore_ascii_case(who)))
}

async fn owners_left(s: &AppState, org_id: &str, except: &str) -> ApiResult<usize> {
    Ok(s.orgs
        .control()
        .list_members(org_id)
        .await?
        .iter()
        .filter(|m| m.role == Role::Owner && m.user_id != except)
        .count())
}

/// Add someone by email (they join when they first sign in), or change
/// their role. Only owners make or unmake owners.
async fn set_member(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Path(who): Path<String>,
    Json(b): Json<SetMember>,
) -> ApiResult<Json<Membership>> {
    let control = s.orgs.control();
    let current = find_member(&s, &p.org_id, &who).await?;
    let was_owner = current.as_ref().is_some_and(|m| m.role == Role::Owner);
    if (b.role == Role::Owner || was_owner) && !p.is_owner() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "only owners make or change owners",
        ));
    }
    let user = match &current {
        Some(m) => m.user_id.clone(),
        None => {
            if !who.contains('@') {
                return Err(ApiError::bad_request("add people by their email"));
            }
            control.create_user(&who, b.name.as_deref()).await?.id
        }
    };
    if was_owner && b.role != Role::Owner && owners_left(&s, &p.org_id, &user).await? == 0 {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "an organization needs an owner",
        ));
    }
    control.set_membership(&p.org_id, &user, b.role).await?;
    find_member(&s, &p.org_id, &user)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::not_found(who))
}

async fn remove_member(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Path(who): Path<String>,
) -> ApiResult<Json<Done>> {
    let m = find_member(&s, &p.org_id, &who)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("member {who}")))?;
    if m.role == Role::Owner {
        if !p.is_owner() {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "only owners remove owners",
            ));
        }
        if owners_left(&s, &p.org_id, &m.user_id).await? == 0 {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "an organization needs an owner",
            ));
        }
    }
    s.orgs
        .control()
        .remove_membership(&p.org_id, &m.user_id)
        .await?;
    Ok(Json(Done { ok: true }))
}

async fn list_teams(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
) -> ApiResult<Json<Vec<Team>>> {
    Ok(Json(s.orgs.control().list_teams(&p.org_id).await?))
}

#[derive(Deserialize, JsonSchema)]
pub struct NewTeam {
    pub name: String,
}

async fn create_team(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Json(b): Json<NewTeam>,
) -> ApiResult<(StatusCode, Json<Team>)> {
    let name = b.name.trim();
    if name.is_empty()
        || name.len() > 40
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || " -_.".contains(c))
    {
        return Err(ApiError::bad_request(
            "team names use letters, digits, spaces, '-', '_' and '.' (at most 40)",
        ));
    }
    Ok((
        StatusCode::CREATED,
        Json(s.orgs.control().create_team(&p.org_id, name).await?),
    ))
}

async fn team_or_404(s: &AppState, org_id: &str, team: &str) -> ApiResult<Team> {
    s.orgs
        .control()
        .get_team(org_id, team)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("team {team}")))
}

/// Delete a team: what it owned goes back to the whole organization, and
/// its keys are revoked.
async fn delete_team(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Path(team): Path<String>,
) -> ApiResult<Json<Done>> {
    let t = team_or_404(&s, &p.org_id, &team).await?;
    s.orgs.control().delete_team(&p.org_id, &t.id).await?;
    Ok(Json(Done { ok: true }))
}

async fn add_team_member(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Path((team, who)): Path<(String, String)>,
) -> ApiResult<Json<Team>> {
    let t = team_or_404(&s, &p.org_id, &team).await?;
    let m = find_member(&s, &p.org_id, &who).await?.ok_or_else(|| {
        ApiError::bad_request(format!("{who} isn't a member of this organization"))
    })?;
    s.orgs.control().add_team_member(&t.id, &m.user_id).await?;
    Ok(Json(team_or_404(&s, &p.org_id, &t.id).await?))
}

async fn remove_team_member(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Path((team, who)): Path<(String, String)>,
) -> ApiResult<Json<Team>> {
    let t = team_or_404(&s, &p.org_id, &team).await?;
    let user = find_member(&s, &p.org_id, &who)
        .await?
        .map(|m| m.user_id)
        .unwrap_or(who);
    s.orgs.control().remove_team_member(&t.id, &user).await?;
    Ok(Json(team_or_404(&s, &p.org_id, &t.id).await?))
}

#[derive(Serialize, JsonSchema)]
pub struct Owner {
    pub kind: String,
    pub name: String,
    pub team_id: String,
    pub team: String,
}

async fn list_owners(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
) -> ApiResult<Json<Vec<Owner>>> {
    let control = s.orgs.control();
    let teams = control.list_teams(&p.org_id).await?;
    Ok(Json(
        control
            .list_owners(&p.org_id)
            .await?
            .into_iter()
            .map(|(kind, name, team_id)| Owner {
                team: teams
                    .iter()
                    .find(|t| t.id == team_id)
                    .map(|t| t.name.clone())
                    .unwrap_or_default(),
                kind,
                name,
                team_id,
            })
            .collect(),
    ))
}

#[derive(Deserialize, JsonSchema)]
pub struct SetOwner {
    /// A team (id or name), or null for the whole organization.
    pub team: Option<String>,
}

const OWNED_KINDS: [&str; 8] = [
    "agent",
    "workflow",
    "persona",
    "skill",
    "trigger",
    "source",
    "eval",
    "collection",
];

/// Give a resource to a team, or back to the whole organization. Builders
/// may move things between their own teams.
async fn set_owner(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Path((kind, name)): Path<(String, String)>,
    Json(b): Json<SetOwner>,
) -> ApiResult<Json<Value>> {
    if !OWNED_KINDS.contains(&kind.as_str()) {
        return Err(ApiError::bad_request(format!(
            "kind is one of {}",
            OWNED_KINDS.join(", ")
        )));
    }
    let control = s.orgs.control();
    let current = control.get_owner(&p.org_id, &kind, &name).await?;
    let team = match &b.team {
        Some(t) => Some(team_or_404(&s, &p.org_id, t).await?),
        None => None,
    };
    let to = team.as_ref().map(|t| t.id.as_str());
    if !p.can_change(current.as_deref()) || (to.is_some() && !p.can_change(to)) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "builders move things only between their own teams",
        ));
    }
    if to.is_none() && !p.is_admin() && p.team_id.is_some() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "a team key keeps things in its team",
        ));
    }
    control.set_owner(&p.org_id, &kind, &name, to).await?;
    Ok(Json(json!({
        "kind": kind,
        "name": name,
        "team": team.map(|t| t.name),
    })))
}

/// Names of the organization's secrets (never their values).
async fn list_secrets(Tenant(s): Tenant) -> ApiResult<Json<Vec<String>>> {
    let tenant = s.rt.store();
    Ok(Json(
        tenant
            .list_settings(SECRET_PREFIX)
            .await?
            .into_iter()
            .map(|(k, _)| k[SECRET_PREFIX.len()..].to_owned())
            .collect(),
    ))
}

#[derive(Deserialize, JsonSchema)]
pub struct SecretValue {
    pub value: String,
}

fn valid_secret_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 64 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Save a secret that this organization's sources read by name (their
/// `token_env`, `email_env`).
async fn put_secret(
    Tenant(s): Tenant,
    Path(name): Path<String>,
    Json(b): Json<SecretValue>,
) -> ApiResult<Json<Done>> {
    if !valid_secret_name(&name) {
        return Err(ApiError::bad_request(
            "secret names use letters, digits and '_'",
        ));
    }
    s.rt.store()
        .put_setting(&format!("{SECRET_PREFIX}{name}"), &b.value)
        .await?;
    Ok(Json(Done { ok: true }))
}

async fn delete_secret(Tenant(s): Tenant, Path(name): Path<String>) -> ApiResult<Json<Done>> {
    if !s
        .rt
        .store()
        .delete_setting(&format!("{SECRET_PREFIX}{name}"))
        .await?
    {
        return Err(ApiError::not_found(format!("secret {name}")));
    }
    Ok(Json(Done { ok: true }))
}
