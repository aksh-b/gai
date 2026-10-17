use std::convert::Infallible;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use futures::Stream;
use gai::agent::{AgentSpec, Decision, EventEnvelope, PendingApproval, RunEvent, RunRequest};
use gai::core::ContentBlock;
use gai::llm::{CatalogEntry, KeyState};
use gai::store::{
    AgentSummary, AgentVersion, ApiKey, NewSession, Run, RunFilter, RunStatus, Scope, Session,
    Step, StoredMessage, UsageTotal, keys,
};
use gai::tools::ToolPolicy;
use gai::tools::mcp::{McpServer, McpServerConfig, McpServerInfo};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

use crate::AppState;
use crate::auth::Principal;
use crate::error::{ApiError, ApiResult};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/me", get(me))
        .route("/v1/models", get(models))
        .route("/v1/providers", get(providers))
        .route("/v1/tools", get(tools))
        .route("/v1/agents", get(list_agents).post(save_agent))
        .route("/v1/agents/default", get(default_agent))
        .route("/v1/agents/{name}", get(get_agent).delete(delete_agent))
        .route("/v1/agents/{name}/versions", get(agent_versions))
        .route(
            "/v1/agents/{name}/labels/{label}",
            post(set_label).delete(remove_label),
        )
        .route("/v1/sessions", get(list_sessions).post(create_session))
        .route("/v1/sessions/{id}", get(get_session).delete(delete_session))
        .route("/v1/sessions/{id}/messages", get(session_messages))
        .route("/v1/runs", get(list_runs).post(create_run))
        .route("/v1/runs/{id}", get(get_run))
        .route("/v1/runs/{id}/steps", get(run_steps))
        .route("/v1/runs/{id}/trace", get(run_trace))
        .route("/v1/runs/{id}/events", get(run_events))
        .route("/v1/runs/{id}/cancel", post(cancel_run))
        .route("/v1/runs/{id}/approvals/{call_id}", post(resolve_approval))
        .route("/v1/approvals", get(approvals))
        .route("/v1/usage", get(usage))
        .route("/v1/keys", get(list_keys).post(create_key))
        .route("/v1/keys/{id}", delete(revoke_key))
        .route("/v1/runs/{id}/resume", post(resume_run))
        .route("/v1/mcp", get(list_mcp).post(add_mcp))
        .route("/v1/mcp/{name}", delete(remove_mcp))
        .route("/v1/mcp/{name}/reconnect", post(reconnect_mcp))
        .route(
            "/v1/mcp/{name}/sign-in",
            post(mcp_sign_in).delete(mcp_sign_out),
        )
        .route("/v1/mcp/oauth/callback", get(mcp_oauth_callback))
        .route("/v1/hooks", get(hooks))
}

// ---- response shapes (also used for the OpenAPI document) ----

#[derive(Serialize, JsonSchema)]
pub struct Health {
    pub status: String,
    pub version: String,
    pub default_model: Option<String>,
    /// `open` when requests without a key are accepted, else `keys`.
    pub auth: String,
}

#[derive(Serialize, JsonSchema)]
pub struct Me {
    pub principal: Principal,
    pub auth: String,
    pub budget_usd: Option<f64>,
    pub spent_usd: Option<f64>,
}

#[derive(Serialize, JsonSchema)]
pub struct ProviderView {
    pub name: String,
    pub dialect: String,
    pub base_url: Option<String>,
    pub key: KeyState,
    pub models: Vec<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct ToolView {
    pub spec: gai::core::ToolSpec,
    pub policy: ToolPolicy,
}

#[derive(Serialize, JsonSchema)]
pub struct AgentEntry {
    pub summary: AgentSummary,
    pub spec: Option<AgentSpec>,
}

#[derive(Serialize, JsonSchema)]
pub struct AgentView {
    pub name: String,
    pub version: u32,
    pub spec: AgentSpec,
}

#[derive(Serialize, JsonSchema)]
pub struct Saved {
    pub name: String,
    pub version: u32,
}

#[derive(Serialize, JsonSchema)]
pub struct RunIds {
    pub run_id: String,
    pub session_id: String,
}

#[derive(Serialize, JsonSchema)]
pub struct RunView {
    #[serde(flatten)]
    pub run: Run,
    /// Running in this server process right now.
    pub active: bool,
}

#[derive(Serialize, JsonSchema)]
pub struct Trace {
    pub run: Run,
    pub steps: Vec<Step>,
    /// Runs this run started (sub-agents), each with its own trace.
    pub children: Vec<Trace>,
}

#[derive(Serialize, JsonSchema)]
pub struct Done {
    pub ok: bool,
}

#[derive(Serialize, JsonSchema)]
pub struct CreatedKey {
    pub key: ApiKey,
    /// The full key. It is shown only this once.
    pub secret: String,
}

// ---- request shapes ----

#[derive(Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum AgentRef {
    /// A stored agent (`name`, `name@v3`, `name@prod`) or an agent file.
    Name(String),
    /// An inline agent definition.
    Spec(Box<AgentSpec>),
}

#[derive(Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Input {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Deserialize, JsonSchema, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Respond with the outcome once the run ends.
    #[default]
    Wait,
    /// Respond with the run's events as Server-Sent Events.
    Stream,
    /// Queue the run and respond at once with its id; follow
    /// `/v1/runs/{id}/events`. Queued runs survive a restart.
    Background,
}

#[derive(Deserialize, JsonSchema)]
pub struct CreateRun {
    /// Defaults to the built-in general assistant.
    pub agent: Option<AgentRef>,
    pub input: Input,
    /// Continue this session; a new one is created when omitted.
    pub session_id: Option<String>,
    /// Template variables for the system prompt.
    #[serde(default)]
    pub vars: Map<String, Value>,
    #[serde(default)]
    pub mode: Mode,
    /// Override the agent's model.
    pub model: Option<String>,
    /// Run `approve`-policy tools without asking.
    #[serde(default)]
    pub auto_approve: bool,
    /// Who the run is for. Selects `user`-scoped memory blocks (the user's
    /// profile); defaults to the session's user, then `default`.
    pub user_id: Option<String>,
    /// POST the outcome (a `RunOutcome`) here when the run ends.
    pub callback_url: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct CreateKey {
    pub name: String,
    #[serde(default = "default_scopes")]
    pub scopes: Vec<Scope>,
    pub budget_usd: Option<f64>,
}

fn default_scopes() -> Vec<Scope> {
    vec![Scope::Run]
}

#[derive(Deserialize, JsonSchema)]
pub struct AddMcpServer {
    pub name: String,
    pub config: McpServerConfig,
}

#[derive(Deserialize, JsonSchema)]
pub struct SignInRequest {
    /// Where the browser returns after approving: this server's
    /// `/v1/mcp/oauth/callback`, as the browser reaches it.
    pub redirect_uri: String,
}

#[derive(Serialize, JsonSchema)]
pub struct SignInStarted {
    /// Open this in a browser to approve access.
    pub auth_url: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct LabelBody {
    pub version: u32,
}

// ---- handlers ----

async fn auth_label(s: &AppState) -> String {
    if s.auth.open(s.rt.store()).await {
        "open"
    } else {
        "keys"
    }
    .to_owned()
}

async fn health(State(s): State<AppState>) -> Json<Health> {
    Json(Health {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        default_model: s.gai.default_model(),
        auth: auth_label(&s).await,
    })
}

async fn me(State(s): State<AppState>, Extension(p): Extension<Principal>) -> Json<Me> {
    let (budget_usd, spent_usd) = p
        .key
        .as_ref()
        .map(|k| (k.budget_usd, Some(k.spent_usd)))
        .unwrap_or((None, None));
    Json(Me {
        auth: auth_label(&s).await,
        principal: p,
        budget_usd,
        spent_usd,
    })
}

async fn models(State(s): State<AppState>) -> Json<Vec<CatalogEntry>> {
    Json(s.gai.models().catalog())
}

async fn providers(State(s): State<AppState>) -> Json<Vec<ProviderView>> {
    Json(
        s.gai
            .models()
            .providers()
            .map(|(name, c)| ProviderView {
                name: name.to_owned(),
                dialect: c.dialect.clone(),
                base_url: c.base_url.clone(),
                key: KeyState::from(c.key_status()),
                models: c.models.iter().map(|m| m.id.clone()).collect(),
            })
            .collect(),
    )
}

async fn tools(State(s): State<AppState>) -> Json<Vec<ToolView>> {
    Json(
        s.rt.tool_specs()
            .into_iter()
            .map(|(spec, policy)| ToolView { spec, policy })
            .collect(),
    )
}

async fn list_agents(State(s): State<AppState>) -> ApiResult<Json<Vec<AgentEntry>>> {
    let mut out = Vec::new();
    for summary in s.rt.store().list_agents().await? {
        let spec =
            s.rt.load_agent(&summary.name, None)
                .await
                .ok()
                .map(|(spec, _)| spec);
        out.push(AgentEntry { summary, spec });
    }
    Ok(Json(out))
}

async fn default_agent() -> Json<AgentSpec> {
    Json(gai::default_agent())
}

#[derive(Deserialize)]
struct SaveQuery {
    note: Option<String>,
}

async fn save_agent(
    State(s): State<AppState>,
    Query(q): Query<SaveQuery>,
    Json(spec): Json<AgentSpec>,
) -> ApiResult<(StatusCode, Json<Saved>)> {
    let version = s.rt.save_agent(&spec, q.note.as_deref()).await?;
    Ok((
        StatusCode::CREATED,
        Json(Saved {
            name: spec.name,
            version,
        }),
    ))
}

#[derive(Deserialize)]
struct VersionQuery {
    version: Option<u32>,
}

async fn get_agent(
    State(s): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<VersionQuery>,
) -> ApiResult<Json<AgentView>> {
    let (spec, version) = s.rt.load_agent(&name, q.version).await?;
    Ok(Json(AgentView {
        name,
        version,
        spec,
    }))
}

async fn agent_versions(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<Vec<AgentVersion>>> {
    Ok(Json(s.rt.store().list_agent_versions(&name).await?))
}

async fn delete_agent(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<StatusCode> {
    if s.rt.store().delete_agent(&name).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("agent {name}")))
    }
}

async fn set_label(
    State(s): State<AppState>,
    Path((name, label)): Path<(String, String)>,
    Json(b): Json<LabelBody>,
) -> ApiResult<Json<Done>> {
    s.rt.store()
        .set_agent_label(&name, &label, Some(b.version))
        .await?;
    Ok(Json(Done { ok: true }))
}

async fn remove_label(
    State(s): State<AppState>,
    Path((name, label)): Path<(String, String)>,
) -> ApiResult<Json<Done>> {
    s.rt.store().set_agent_label(&name, &label, None).await?;
    Ok(Json(Done { ok: true }))
}

#[derive(Deserialize)]
struct ListQuery {
    agent: Option<String>,
    session_id: Option<String>,
    status: Option<String>,
    /// Forks (replays) of this run.
    fork_of: Option<String>,
    limit: Option<u32>,
}

async fn list_sessions(
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Vec<Session>>> {
    Ok(Json(
        s.rt.store()
            .list_sessions(q.agent.as_deref(), q.limit.unwrap_or(50))
            .await?,
    ))
}

async fn create_session(
    State(s): State<AppState>,
    Json(new): Json<NewSession>,
) -> ApiResult<(StatusCode, Json<Session>)> {
    Ok((
        StatusCode::CREATED,
        Json(s.rt.store().create_session(new).await?),
    ))
}

async fn get_session(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Session>> {
    let ses =
        s.rt.store()
            .get_session(&id)
            .await?
            .ok_or_else(|| ApiError::not_found(format!("session {id}")))?;
    Ok(Json(ses))
}

async fn session_messages(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<StoredMessage>>> {
    Ok(Json(s.rt.store().list_messages(&id).await?))
}

async fn delete_session(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    if s.rt.store().delete_session(&id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("session {id}")))
    }
}

async fn create_run(
    State(s): State<AppState>,
    Extension(p): Extension<Principal>,
    Json(body): Json<CreateRun>,
) -> ApiResult<Response> {
    p.check_budget()?;
    let (mut spec, version) = match body.agent {
        None => (gai::default_agent(), None),
        Some(AgentRef::Name(r)) => s.gai.agent(&r).await?,
        Some(AgentRef::Spec(spec)) => (*spec, None),
    };
    if let Some(m) = body.model {
        spec.model = Some(m);
    }
    let input = match body.input {
        Input::Text(t) => vec![ContentBlock::text(t)],
        Input::Blocks(b) => b,
    };
    let mut req = RunRequest::with_input(spec, input);
    req.agent_version = version;
    req.session_id = body.session_id;
    req.vars = body.vars;
    req.auto_approve = body.auto_approve;
    req.user_id = body.user_id;
    req.callback_url = body.callback_url;
    if let Some(id) = p.key_id() {
        req.metadata.insert("key_id".into(), json!(id));
    }
    // Spend is charged to the key by the runtime's finish hook (see
    // `router`), so every mode and resumed runs are charged alike.
    if body.mode == Mode::Background {
        let (run_id, session_id) = s.rt.enqueue(req).await?;
        return Ok((StatusCode::ACCEPTED, Json(RunIds { run_id, session_id })).into_response());
    }
    let mut handle = s.rt.start(req).await?;
    match body.mode {
        Mode::Stream => {
            let (tx, rx) = mpsc::channel(256);
            tokio::spawn(async move {
                while let Some(e) = handle.next_event().await {
                    // Keep draining when the client leaves, so the run is unaffected.
                    let _ = tx.send(e).await;
                }
                handle.wait().await;
            });
            Ok(sse(rx).into_response())
        }
        _ => Ok(Json(handle.wait().await).into_response()),
    }
}

fn status_filter(s: Option<String>) -> ApiResult<Option<RunStatus>> {
    s.map(|s| {
        RunStatus::parse(&s).ok_or_else(|| ApiError::bad_request(format!("unknown status {s}")))
    })
    .transpose()
}

/// Runs carry a snapshot of their agent spec; lists leave it out.
fn slim(mut r: Run) -> Run {
    if let Some(m) = r.metadata.as_object_mut() {
        m.remove("spec");
    }
    r
}

async fn list_runs(
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Vec<Run>>> {
    let runs =
        s.rt.store()
            .list_runs(RunFilter {
                session_id: q.session_id,
                agent: q.agent,
                status: status_filter(q.status)?,
                parent_run_id: None,
                fork_of: q.fork_of,
                limit: Some(q.limit.unwrap_or(50)),
            })
            .await?;
    Ok(Json(runs.into_iter().map(slim).collect()))
}

async fn get_run(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<RunView>> {
    let run =
        s.rt.store()
            .get_run(&id)
            .await?
            .ok_or_else(|| ApiError::not_found(format!("run {id}")))?;
    Ok(Json(RunView {
        active: s.rt.is_active(&id),
        run,
    }))
}

async fn run_steps(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<Step>>> {
    Ok(Json(s.rt.store().list_steps(&id).await?))
}

async fn run_trace(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Trace>> {
    Ok(Json(trace(&s, id, 0).await?))
}

fn trace(s: &AppState, id: String, depth: u32) -> futures::future::BoxFuture<'_, ApiResult<Trace>> {
    Box::pin(async move {
        let store = s.rt.store();
        let run = store
            .get_run(&id)
            .await?
            .ok_or_else(|| ApiError::not_found(format!("run {id}")))?;
        let steps = store.list_steps(&id).await?;
        let mut children = Vec::new();
        if depth < 8 {
            let kids = store
                .list_runs(RunFilter {
                    parent_run_id: Some(id.clone()),
                    limit: Some(500),
                    ..Default::default()
                })
                .await?;
            for k in kids {
                children.push(trace(s, k.id, depth + 1).await?);
            }
        }
        Ok(Trace {
            run: slim(run),
            steps,
            children,
        })
    })
}

#[derive(Deserialize)]
struct EventsQuery {
    after: Option<u64>,
}

/// Stored events after `after` (or the `Last-Event-ID` header), then live
/// events while the run is active.
async fn run_events(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<EventsQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let after = q.after.or_else(|| {
        headers
            .get("last-event-id")
            .and_then(|v| v.to_str().ok()?.parse().ok())
    });
    if !s.rt.is_active(&id) && s.rt.store().get_run(&id).await?.is_none() {
        return Err(ApiError::not_found(format!("run {id}")));
    }
    let (tx, rx) = mpsc::channel(256);
    let rt = s.rt.clone();
    tokio::spawn(async move {
        // A queued run has no events yet; wait for a worker to start it. A
        // run just taken from the queue gets a moment to register here.
        let since = std::time::Instant::now();
        while !rt.is_active(&id) && !tx.is_closed() {
            match rt.store().get_run(&id).await {
                Ok(Some(r))
                    if r.status == RunStatus::Queued
                        || (!r.status.is_terminal()
                            && since.elapsed() < std::time::Duration::from_secs(3)) =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await
                }
                _ => break,
            }
        }
        // Subscribe before reading the store so nothing falls in between.
        let live = rt.subscribe(&id);
        let stored = match rt.store().list_events(&id, after, 100_000).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(run = %id, error = %e, "could not read run events");
                return;
            }
        };
        let mut last = after.unwrap_or(0);
        for e in stored {
            if let Ok(env) = serde_json::from_value::<EventEnvelope>(e.event) {
                last = env.seq;
                let done = matches!(env.event, RunEvent::RunFinished { .. });
                if tx.send(env).await.is_err() || done {
                    return;
                }
            }
        }
        let Some(mut rx) = live else { return };
        loop {
            match rx.recv().await {
                Ok(env) if env.seq <= last => {}
                Ok(env) => {
                    let done = matches!(env.event, RunEvent::RunFinished { .. });
                    if tx.send(env).await.is_err() || done {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => return,
            }
        }
    });
    Ok(sse(rx).into_response())
}

pub(crate) fn sse(
    rx: mpsc::Receiver<EventEnvelope>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = futures::stream::unfold(rx, |mut rx| async move {
        let env = rx.recv().await?;
        let data = serde_json::to_string(&env).unwrap_or_default();
        let kind = serde_json::to_value(&env.event)
            .ok()
            .and_then(|v| v["type"].as_str().map(str::to_owned))
            .unwrap_or_default();
        Some((
            Ok(Event::default()
                .id(env.seq.to_string())
                .event(kind)
                .data(data)),
            rx,
        ))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn cancel_run(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Done>> {
    if s.rt.cancel(&id) {
        Ok(Json(Done { ok: true }))
    } else {
        Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("run {id} is not running"),
        ))
    }
}

async fn resolve_approval(
    State(s): State<AppState>,
    Path((id, call_id)): Path<(String, String)>,
    Json(decision): Json<Decision>,
) -> ApiResult<Json<Done>> {
    s.rt.resolve_approval(&id, &call_id, decision)
        .map_err(|e| ApiError::new(StatusCode::CONFLICT, e))?;
    Ok(Json(Done { ok: true }))
}

async fn approvals(State(s): State<AppState>) -> Json<Vec<PendingApproval>> {
    Json(s.rt.approvals().pending(None))
}

#[derive(Deserialize)]
struct UsageQuery {
    since: Option<chrono::DateTime<chrono::Utc>>,
}

async fn usage(
    State(s): State<AppState>,
    Query(q): Query<UsageQuery>,
) -> ApiResult<Json<Vec<UsageTotal>>> {
    Ok(Json(s.rt.store().usage_totals(q.since).await?))
}

async fn list_keys(State(s): State<AppState>) -> ApiResult<Json<Vec<ApiKey>>> {
    Ok(Json(s.rt.store().list_keys().await?))
}

async fn create_key(
    State(s): State<AppState>,
    Json(b): Json<CreateKey>,
) -> ApiResult<(StatusCode, Json<CreatedKey>)> {
    if b.name.trim().is_empty() {
        return Err(ApiError::bad_request("a key needs a name"));
    }
    if b.scopes.is_empty() {
        return Err(ApiError::bad_request("a key needs at least one scope"));
    }
    let k = keys::generate();
    let key =
        s.rt.store()
            .create_key(b.name.trim(), &k.prefix, &k.hash, &b.scopes, b.budget_usd)
            .await?;
    Ok((
        StatusCode::CREATED,
        Json(CreatedKey {
            key,
            secret: k.secret,
        }),
    ))
}

async fn revoke_key(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<StatusCode> {
    if s.rt.store().revoke_key(&id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("key {id}")))
    }
}

async fn resume_run(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<RunIds>> {
    s.rt.resume_any(&id).await?;
    let run =
        s.rt.store()
            .get_run(&id)
            .await?
            .ok_or_else(|| ApiError::not_found(format!("run {id}")))?;
    Ok(Json(RunIds {
        run_id: run.id,
        session_id: run.session_id,
    }))
}

async fn list_mcp(State(s): State<AppState>) -> Json<Vec<McpServerInfo>> {
    Json(s.rt.mcp_servers())
}

fn valid_server_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Add (or replace) an MCP server and connect it. The server is kept even
/// when the first connection fails; its status says why.
async fn add_mcp(
    State(s): State<AppState>,
    Json(b): Json<AddMcpServer>,
) -> ApiResult<(StatusCode, Json<McpServerInfo>)> {
    if !valid_server_name(&b.name) {
        return Err(ApiError::bad_request(
            "server names use letters, digits, '-' and '_' (at most 40)",
        ));
    }
    b.config.validate().map_err(ApiError::bad_request)?;
    if s.gai.config().mcp_servers.contains_key(&b.name) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("{} is declared in gai.toml; change it there", b.name),
        ));
    }
    let value = serde_json::to_value(&b.config).expect("serializable config");
    s.rt.store().put_mcp_server(&b.name, &value).await?;
    let server = McpServer::new(&b.name, b.config, "api");
    let _ = s.rt.connect_mcp(server.clone()).await;
    Ok((StatusCode::CREATED, Json(server.info())))
}

async fn remove_mcp(State(s): State<AppState>, Path(name): Path<String>) -> ApiResult<StatusCode> {
    if s.gai.config().mcp_servers.contains_key(&name) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("{name} is declared in gai.toml; remove it there"),
        ));
    }
    let stored = s.rt.store().delete_mcp_server(&name).await?;
    let connected = s.rt.remove_mcp(&name).await;
    if stored || connected {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("MCP server {name}")))
    }
}

async fn reconnect_mcp(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<McpServerInfo>> {
    let server =
        s.rt.mcp()
            .get(&name)
            .ok_or_else(|| ApiError::not_found(format!("MCP server {name}")))?;
    let fresh = McpServer::new(&server.name, server.config.clone(), &server.source);
    let _ = s.rt.connect_mcp(fresh.clone()).await;
    Ok(Json(fresh.info()))
}

async fn hooks(State(s): State<AppState>) -> Json<Vec<String>> {
    Json(s.rt.hook_names())
}

async fn mcp_sign_in(
    State(s): State<AppState>,
    Path(name): Path<String>,
    Json(b): Json<SignInRequest>,
) -> ApiResult<Json<SignInStarted>> {
    if !b.redirect_uri.ends_with("/v1/mcp/oauth/callback") {
        return Err(ApiError::bad_request(
            "redirect_uri must point at this server's /v1/mcp/oauth/callback",
        ));
    }
    let auth_url =
        s.rt.mcp()
            .begin_sign_in(&name, &b.redirect_uri)
            .await
            .map_err(ApiError::bad_request)?;
    Ok(Json(SignInStarted { auth_url }))
}

async fn mcp_sign_out(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<McpServerInfo>> {
    let server =
        s.rt.mcp()
            .get(&name)
            .ok_or_else(|| ApiError::not_found(format!("MCP server {name}")))?;
    server.sign_out().await;
    let fresh = McpServer::new(&server.name, server.config.clone(), &server.source);
    let _ = s.rt.connect_mcp(fresh.clone()).await;
    Ok(Json(fresh.info()))
}

/// Where the browser lands after approving access. Public (the browser
/// carries no API key); the one-time OAuth `state` ties it to a sign-in
/// an admin started.
async fn mcp_oauth_callback(
    State(s): State<AppState>,
    uri: axum::http::Uri,
) -> axum::response::Html<String> {
    let url = format!("http://gai.local{uri}");
    let (title, detail, server) = match s.rt.mcp().finish_sign_in(&url).await {
        Ok(name) => {
            let config =
                s.rt.mcp()
                    .get(&name)
                    .map(|srv| (srv.config.clone(), srv.source.clone()));
            // A repeated callback finds the server already connected.
            let already =
                s.rt.mcp()
                    .get(&name)
                    .is_some_and(|srv| srv.info().status == gai::tools::mcp::McpStatus::Ready);
            let config = config.filter(|_| !already);
            let connected = match config {
                Some((config, source)) => {
                    let fresh = McpServer::new(&name, config, &source);
                    match s.rt.connect_mcp(fresh).await {
                        Ok(n) => format!("Connected: {n} tools are available."),
                        Err(e) => format!("Signed in, but connecting failed: {e}"),
                    }
                }
                None => "Signed in.".to_owned(),
            };
            (format!("Signed in to {name}"), connected, name)
        }
        Err(e) => ("Sign-in failed".to_owned(), e, String::new()),
    };
    let esc = |t: &str| {
        t.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    axum::response::Html(format!(
        r#"<!doctype html><meta charset="utf-8"><title>{t}</title>
<body style="font-family:system-ui,sans-serif;background:#141210;color:#eee;display:grid;place-items:center;height:100vh;margin:0">
<div style="max-width:440px;text-align:center"><h2>{t}</h2><p style="color:#aaa">{d}</p><p style="color:#777">You can close this tab.</p></div>
<script>try{{window.opener&&window.opener.postMessage({{type:"gai-mcp-signed-in",server:"{srv}"}},location.origin)}}catch(e){{}}</script>
</body>"#,
        t = esc(&title),
        d = esc(&detail),
        srv = esc(&server),
    ))
}
