use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::future::BoxFuture;
use gai_core::{ContentBlock, Message, Role, ToolSpec, Usage};
use gai_llm::{
    Accumulator, ChatEvent, ChatRequest, LlmError, ModelRegistry, ResponseFormat, SharedChatModel,
    StopReason,
};
use gai_memory::{Knowledge, KnowledgeSettings, MemoryError, RunMemory};
use gai_skills::{SkillError, Skills};
use gai_store::{
    NewRun, NewSession, Run, RunStatus, RunUpdate, SharedStore, StepKind, StepStatus, StoreError,
};
use gai_tools::mcp::{McpManager, McpServer, McpServerInfo, tool_prefix};
use gai_tools::{SharedTool, ToolCtx, ToolOutput, ToolPolicy, ToolRegistry, ToolSettings};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::sync::{Notify, Semaphore, broadcast};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::approval::{ApprovalHub, Decision, PendingApproval};
use crate::context::{Conversation, transcript};
use crate::event::{Emitter, EventEnvelope, RunEvent};
use crate::hooks::{Blocked, HookCtx, RunHook, ToolCall};
use crate::media::{MEDIA_TOOLS, MediaResolver, media_tools};
use crate::multi::{RunLauncher, run_kind};
use crate::persona::{
    PersonaDefaults, PersonaRef, ResolvedPersona, compose, template_places_persona,
};
use crate::replay::{ForkInfo, RecordedStep, RecordedTool, Replay};
use crate::spec::AgentSpec;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("invalid agent: {0}")]
    Invalid(String),
    #[error(transparent)]
    Model(#[from] LlmError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("system prompt template: {0}")]
    Template(String),
    #[error(transparent)]
    Memory(#[from] MemoryError),
    #[error(transparent)]
    Skill(#[from] SkillError),
}

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    /// Used when an agent names no model.
    pub default_model: Option<String>,
    /// Per-session workspaces live under `<data_dir>/workspaces`.
    pub data_dir: PathBuf,
    pub tools: ToolSettings,
    pub knowledge: KnowledgeSettings,
    /// Folders of Agent Skills (each skill a subfolder with a SKILL.md).
    pub skills_dirs: Vec<PathBuf>,
    /// Folders searched for agent files by name (`<name>.yaml` and so on).
    pub agents_dirs: Vec<PathBuf>,
    /// Journal each model call's full request, so runs can be inspected and
    /// replayed exactly. Costs storage on long runs.
    pub record_requests: bool,
    /// Default models for the media tools (agents can override them).
    pub media: crate::media::MediaSettings,
    /// Persona for runs whose agent and request pick none.
    pub persona: PersonaDefaults,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            default_model: None,
            data_dir: std::env::temp_dir().join("gai"),
            tools: ToolSettings::default(),
            knowledge: KnowledgeSettings::default(),
            skills_dirs: Vec::new(),
            agents_dirs: Vec::new(),
            record_requests: true,
            media: Default::default(),
            persona: PersonaDefaults::default(),
        }
    }
}

/// Called with the outcome of every run that finishes in this process.
pub type FinishHook = Arc<dyn Fn(RunOutcome) -> BoxFuture<'static, ()> + Send + Sync>;

/// Tools the runtime builds for each run from the agent's `memory`,
/// `knowledge` and `skills` settings, rather than taking from the registry.
const AGENT_TOOLS: [&str; 10] = [
    "image_gen",
    "speak",
    "transcribe",
    "memory_append",
    "memory_replace",
    "memory_rethink",
    "remember",
    "memory_search",
    "rag_search",
    "load_skill",
];

/// What to run.
#[derive(Clone, Debug)]
pub struct RunRequest {
    pub agent: AgentSpec,
    /// Stored version of the agent, recorded on the run.
    pub agent_version: Option<u32>,
    /// Continue this session; a new one is created when `None`.
    pub session_id: Option<String>,
    pub input: Vec<ContentBlock>,
    /// Template variables for the system prompt.
    pub vars: Map<String, Value>,
    pub parent_run_id: Option<String>,
    pub user_id: Option<String>,
    /// Treat every `approve` tool as `auto`.
    pub auto_approve: bool,
    /// Caller data recorded on the run (e.g. which API key started it).
    pub metadata: Map<String, Value>,
    /// POST the outcome here (JSON [`RunOutcome`]) when the run ends.
    pub callback_url: Option<String>,
    /// Speak as this persona instead of the agent's or the global default;
    /// `"none"` for no persona.
    pub persona: Option<PersonaRef>,
}

impl RunRequest {
    pub fn new(agent: AgentSpec, text: impl Into<String>) -> Self {
        Self::with_input(agent, vec![ContentBlock::text(text)])
    }

    pub fn with_input(agent: AgentSpec, input: Vec<ContentBlock>) -> Self {
        Self {
            agent,
            agent_version: None,
            session_id: None,
            input,
            vars: Map::new(),
            parent_run_id: None,
            user_id: None,
            auto_approve: false,
            metadata: Map::new(),
            callback_url: None,
            persona: None,
        }
    }

    pub fn session(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }
}

/// How a run ended.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunOutcome {
    pub run_id: String,
    pub session_id: String,
    pub status: RunStatus,
    /// The final answer: parsed JSON when the agent has an output schema,
    /// otherwise the text.
    pub output: Option<Value>,
    pub text: String,
    pub error: Option<String>,
    pub usage: Usage,
    pub cost: f64,
    pub steps: u32,
}

pub struct RunHandle {
    pub run_id: String,
    pub session_id: String,
    pub events: broadcast::Receiver<EventEnvelope>,
    join: JoinHandle<RunOutcome>,
}

impl RunHandle {
    /// The next event, skipping over any this receiver fell behind on.
    /// `None` once the run has finished and its events are drained.
    pub async fn next_event(&mut self) -> Option<EventEnvelope> {
        loop {
            match self.events.recv().await {
                Ok(e) => return Some(e),
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "event receiver lagged")
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }

    /// Kill the run's task on the spot, recording nothing, as a crash
    /// would. The run stays unfinished in the store and `Runtime::resume`
    /// can pick it up. For shutdown paths and tests.
    pub fn abort(self) {
        self.join.abort();
    }

    pub async fn wait(self) -> RunOutcome {
        self.join.await.expect("run task panicked")
    }
}

struct Active {
    cancel: CancellationToken,
    tx: broadcast::Sender<EventEnvelope>,
}

struct Inner {
    models: Arc<ModelRegistry>,
    tools: RwLock<ToolRegistry>,
    store: SharedStore,
    config: RuntimeConfig,
    /// Set at runtime (from the dashboard); wins over `config.default_model`.
    default_model: RwLock<Option<String>>,
    /// Set at runtime (from the dashboard); wins over `config.persona`.
    default_persona: RwLock<Option<PersonaDefaults>>,
    http: reqwest::Client,
    approvals: ApprovalHub,
    active: Mutex<HashMap<String, Active>>,
    hooks: RwLock<Vec<Arc<dyn RunHook>>>,
    mcp: McpManager,
    mcp_started: AtomicBool,
    knowledge: Arc<Knowledge>,
    skills: Skills,
    /// Wakes the background-run queue when a run is enqueued.
    queue: Notify,
    queue_started: AtomicBool,
    finish_hooks: RwLock<BTreeMap<String, FinishHook>>,
    launchers: RwLock<HashMap<String, RunLauncher>>,
}

/// Runs agents. Cheap to clone; clones share everything.
#[derive(Clone)]
pub struct Runtime {
    inner: Arc<Inner>,
}

impl Runtime {
    pub fn new(
        models: Arc<ModelRegistry>,
        tools: ToolRegistry,
        store: SharedStore,
        config: RuntimeConfig,
    ) -> Self {
        let knowledge = Arc::new(Knowledge::new(
            store.clone(),
            models.clone(),
            config.knowledge.clone(),
        ));
        let skills = Skills::new(config.skills_dirs.clone(), store.clone());
        Self {
            inner: Arc::new(Inner {
                models,
                tools: RwLock::new(tools),
                store,
                config,
                default_model: RwLock::new(None),
                default_persona: RwLock::new(None),
                knowledge,
                skills,
                queue: Notify::new(),
                queue_started: AtomicBool::new(false),
                finish_hooks: RwLock::new(BTreeMap::new()),
                launchers: RwLock::new(HashMap::new()),
                http: reqwest::Client::builder()
                    .user_agent(concat!("gai/", env!("CARGO_PKG_VERSION")))
                    .build()
                    .expect("http client"),
                approvals: ApprovalHub::default(),
                active: Mutex::new(HashMap::new()),
                hooks: RwLock::new(Vec::new()),
                mcp: McpManager::default(),
                mcp_started: AtomicBool::new(false),
            }),
        }
    }

    /// Knowledge collections and search.
    pub fn knowledge(&self) -> &Arc<Knowledge> {
        &self.inner.knowledge
    }

    pub fn skills(&self) -> &Skills {
        &self.inner.skills
    }

    /// Call `hook` with the outcome of every run that finishes here. A hook
    /// set again under the same name replaces the old one.
    pub fn set_finish_hook(&self, name: &str, hook: FinishHook) {
        self.inner
            .finish_hooks
            .write()
            .expect("hooks lock")
            .insert(name.to_owned(), hook);
    }

    /// Tell the background queue a run was enqueued.
    pub fn wake_queue(&self) {
        self.inner.queue.notify_one();
    }

    /// Let another engine run (and recover, and dequeue) runs whose
    /// `metadata.kind` is `kind`.
    pub fn set_launcher(&self, kind: &str, launcher: RunLauncher) {
        self.inner
            .launchers
            .write()
            .expect("launchers lock")
            .insert(kind.to_owned(), launcher);
    }

    fn launcher_for(&self, run: &Run) -> Option<RunLauncher> {
        let kind = run_kind(run)?;
        self.inner
            .launchers
            .read()
            .expect("launchers lock")
            .get(kind)
            .cloned()
    }

    pub(crate) fn deregister_active(&self, run_id: &str) {
        self.inner
            .active
            .lock()
            .expect("active lock")
            .remove(run_id);
    }

    /// A tool from the registry.
    pub fn tool(&self, name: &str) -> Option<SharedTool> {
        self.inner
            .tools
            .read()
            .expect("tools lock")
            .get(name)
            .cloned()
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.inner.http
    }

    pub fn models(&self) -> &Arc<ModelRegistry> {
        &self.inner.models
    }

    pub fn store(&self) -> &SharedStore {
        &self.inner.store
    }

    pub fn config(&self) -> &RuntimeConfig {
        &self.inner.config
    }

    /// The model used when an agent names none: the one set with
    /// [`Runtime::set_default_model`], else the configured one.
    pub fn default_model(&self) -> Option<String> {
        self.inner
            .default_model
            .read()
            .expect("default model lock")
            .clone()
            .or_else(|| self.inner.config.default_model.clone())
    }

    /// The model set with [`Runtime::set_default_model`], if any.
    pub fn stored_default_model(&self) -> Option<String> {
        self.inner
            .default_model
            .read()
            .expect("default model lock")
            .clone()
    }

    /// Override the configured default model; `None` goes back to it.
    pub fn set_default_model(&self, model: Option<String>) {
        *self
            .inner
            .default_model
            .write()
            .expect("default model lock") = model;
    }

    /// The persona used when neither a run nor its agent picks one: the
    /// one set with [`Runtime::set_default_persona`], else the configured
    /// one.
    pub fn default_persona(&self) -> PersonaDefaults {
        self.inner
            .default_persona
            .read()
            .expect("default persona lock")
            .clone()
            .unwrap_or_else(|| self.inner.config.persona.clone())
    }

    /// Override the configured persona defaults; `None` goes back to them.
    pub fn set_default_persona(&self, defaults: Option<PersonaDefaults>) {
        *self
            .inner
            .default_persona
            .write()
            .expect("default persona lock") = defaults;
    }

    pub fn approvals(&self) -> &ApprovalHub {
        &self.inner.approvals
    }

    /// Add or replace a tool for all future runs.
    pub fn register_tool(&self, tool: SharedTool) {
        self.inner.tools.write().expect("tools lock").register(tool);
    }

    /// Run `hook` around every model and tool call of future runs.
    pub fn add_hook(&self, hook: Arc<dyn RunHook>) {
        self.inner.hooks.write().expect("hooks lock").push(hook);
    }

    pub fn hook_names(&self) -> Vec<String> {
        self.inner
            .hooks
            .read()
            .expect("hooks lock")
            .iter()
            .map(|h| h.name().to_owned())
            .collect()
    }

    pub fn mcp(&self) -> &McpManager {
        &self.inner.mcp
    }

    pub fn mcp_servers(&self) -> Vec<McpServerInfo> {
        self.inner.mcp.list()
    }

    /// Add (or replace) an MCP server, connect it and register its tools.
    /// Returns how many tools it provides.
    pub async fn connect_mcp(&self, server: Arc<McpServer>) -> Result<usize, String> {
        if let Some(old) = self.inner.mcp.add(server.clone()) {
            old.close().await;
        }
        let tools = server.connect().await;
        let mut registry = self.inner.tools.write().expect("tools lock");
        registry.unregister_prefix(&tool_prefix(&server.name));
        let tools = tools?;
        let n = tools.len();
        for t in tools {
            registry.register(t);
        }
        Ok(n)
    }

    /// Disconnect an MCP server and drop its tools.
    pub async fn remove_mcp(&self, name: &str) -> bool {
        let Some(server) = self.inner.mcp.remove(name) else {
            return false;
        };
        self.inner
            .tools
            .write()
            .expect("tools lock")
            .unregister_prefix(&tool_prefix(name));
        server.close().await;
        true
    }

    /// Register an MCP server without connecting yet; it connects on first
    /// need (see [`ensure_mcp`](Self::ensure_mcp)).
    pub fn register_mcp(&self, server: Arc<McpServer>) {
        self.inner.mcp.add(server);
    }

    /// Connect every registered MCP server, once, in the background. Runs
    /// that use MCP tools call this and wait (up to a minute) for it.
    pub fn ensure_mcp(&self) {
        if self.inner.mcp_started.swap(true, Ordering::SeqCst) {
            return;
        }
        self.start_mcp(self.inner.mcp.servers());
    }

    fn start_mcp(&self, servers: Vec<Arc<McpServer>>) {
        let rt = self.clone();
        tokio::spawn(async move {
            let jobs = servers.into_iter().map(|s| {
                let rt = rt.clone();
                async move {
                    let name = s.name.clone();
                    match rt.connect_mcp(s).await {
                        Ok(n) => tracing::info!(server = %name, tools = n, "MCP server connected"),
                        Err(e) => tracing::warn!(server = %name, error = %e, "MCP server failed to connect"),
                    }
                }
            });
            futures::future::join_all(jobs).await;
            rt.inner.mcp.mark_settled();
        });
    }

    pub fn tool_specs(&self) -> Vec<(ToolSpec, ToolPolicy)> {
        let tools = self.inner.tools.read().expect("tools lock");
        tools
            .iter()
            .map(|(_, t)| (t.spec(), t.default_policy()))
            .collect()
    }

    /// Save an agent as a new version (or return the current version when
    /// nothing changed).
    pub async fn save_agent(
        &self,
        spec: &AgentSpec,
        note: Option<&str>,
    ) -> Result<u32, AgentError> {
        spec.validate().map_err(AgentError::Invalid)?;
        let value = serde_json::to_value(spec).expect("serializable spec");
        Ok(self.inner.store.put_agent(&spec.name, &value, note).await?)
    }

    /// Load a stored agent by name and optional version.
    pub async fn load_agent(
        &self,
        name: &str,
        version: Option<u32>,
    ) -> Result<(AgentSpec, u32), AgentError> {
        let v = self
            .inner
            .store
            .get_agent(name, version)
            .await?
            .ok_or_else(|| {
                AgentError::NotFound(format!(
                    "agent {name}{}",
                    version.map(|v| format!(" v{v}")).unwrap_or_default()
                ))
            })?;
        let spec: AgentSpec = serde_json::from_value(v.spec)
            .map_err(|e| AgentError::Invalid(format!("stored agent {name}: {e}")))?;
        Ok((spec, v.version))
    }

    /// Receive events of a run in progress.
    pub fn subscribe(&self, run_id: &str) -> Option<broadcast::Receiver<EventEnvelope>> {
        self.inner
            .active
            .lock()
            .expect("active lock")
            .get(run_id)
            .map(|a| a.tx.subscribe())
    }

    pub fn is_active(&self, run_id: &str) -> bool {
        self.inner
            .active
            .lock()
            .expect("active lock")
            .contains_key(run_id)
    }

    /// Ask a run to stop. Returns false when it is not running here.
    pub fn cancel(&self, run_id: &str) -> bool {
        match self.inner.active.lock().expect("active lock").get(run_id) {
            Some(a) => {
                a.cancel.cancel();
                true
            }
            None => false,
        }
    }

    pub fn resolve_approval(
        &self,
        run_id: &str,
        call_id: &str,
        decision: Decision,
    ) -> Result<(), String> {
        self.inner.approvals.resolve(run_id, call_id, decision)
    }

    /// Run to completion.
    pub async fn run(&self, req: RunRequest) -> Result<RunOutcome, AgentError> {
        Ok(self.start(req).await?.wait().await)
    }

    /// Start a run in the background and return a handle to its events and
    /// outcome. Validation, model and tool resolution happen here, so a bad
    /// request fails before any run is recorded.
    pub async fn start(&self, req: RunRequest) -> Result<RunHandle, AgentError> {
        let input = req.input.clone();
        let l = self.create(req, Map::new()).await?;
        Ok(self.launch(l, Mode::Start(input), Resume::default()))
    }

    /// Record a run and queue it for the background executor (see
    /// [`start_queue`](Self::start_queue)) instead of running it now. The
    /// queue lives in the store, so queued runs survive a restart. Returns
    /// the run and session ids.
    pub async fn enqueue(&self, req: RunRequest) -> Result<(String, String), AgentError> {
        let mut meta = Map::new();
        meta.insert("background".into(), json!(true));
        let l = self.create(req, meta).await?;
        self.inner.queue.notify_one();
        Ok((l.run_id, l.session_id))
    }

    /// Run queued background runs, at most `max_concurrent` at a time, for
    /// as long as the process lives. Calling it again does nothing.
    pub fn start_queue(&self, max_concurrent: usize) {
        if self.inner.queue_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let rt = self.clone();
        let slots = Arc::new(Semaphore::new(max_concurrent.max(1)));
        tokio::spawn(async move {
            loop {
                let permit = slots.clone().acquire_owned().await.expect("semaphore open");
                match rt.inner.store.claim_queued_run().await {
                    Ok(Some(run)) if rt.launcher_for(&run).is_some() => {
                        let launch = rt.launcher_for(&run).expect("checked");
                        tokio::spawn(async move {
                            launch(run, true).await;
                            drop(permit);
                        });
                    }
                    Ok(Some(run)) if run_kind(&run).is_some() => {
                        drop(permit);
                        let update = RunUpdate {
                            status: Some(RunStatus::Failed),
                            error: Some(format!(
                                "nothing here runs {} runs",
                                run_kind(&run).unwrap_or_default()
                            )),
                            ..Default::default()
                        };
                        let _ = rt.inner.store.update_run(&run.id, update).await;
                    }
                    Ok(Some(run)) => {
                        let id = run.id.clone();
                        // Visible as active from the moment it leaves the
                        // queue, so event streams attach to it.
                        rt.register_active(&id);
                        match rt.relaunch(run, true).await {
                            Ok(handle) => {
                                tokio::spawn(async move {
                                    handle.wait().await;
                                    drop(permit);
                                });
                            }
                            Err(e) => {
                                rt.inner.active.lock().expect("active lock").remove(&id);
                                tracing::warn!(run = %id, error = %e, "could not start queued run");
                                let update = RunUpdate {
                                    status: Some(RunStatus::Failed),
                                    error: Some(format!("could not start: {e}")),
                                    ..Default::default()
                                };
                                let _ = rt.inner.store.update_run(&id, update).await;
                            }
                        }
                    }
                    Ok(None) => {
                        drop(permit);
                        // Woken by enqueue; the timeout catches runs queued
                        // by another process sharing the database.
                        let _ =
                            tokio::time::timeout(Duration::from_secs(5), rt.inner.queue.notified())
                                .await;
                    }
                    Err(e) => {
                        drop(permit);
                        tracing::warn!(error = %e, "could not read the run queue");
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        });
    }

    /// Tools and prompt text an agent gets from its `memory`, `knowledge`
    /// and `skills` settings.
    async fn agent_extras(&self, spec: &AgentSpec, user: &str) -> Result<Extras, AgentError> {
        let mut extras = Extras::default();
        let k = &self.inner.knowledge;
        if !spec.memory.blocks.is_empty() {
            let m = Arc::new(
                RunMemory::prepare(self.inner.store.clone(), &spec.memory, &spec.name, user)
                    .await?,
            );
            if m.any_writable().await {
                extras
                    .tools
                    .extend(gai_memory::tools::block_tools(m.clone()));
            }
            extras.memory = Some(m);
        }
        if spec.memory.archival {
            let c = gai_memory::archival_collection(&spec.memory, &spec.name, user);
            extras
                .tools
                .extend(gai_memory::tools::archival_tools(k.clone(), c));
        }
        if !spec.knowledge.is_empty() {
            for c in &spec.knowledge {
                k.collection(c).await?;
            }
            extras.tools.push(gai_memory::tools::rag_tool(
                k.clone(),
                spec.knowledge.clone(),
            ));
        } else if spec.tools.iter().any(|t| t == "rag_search") {
            extras
                .tools
                .push(gai_memory::tools::rag_tool(k.clone(), Vec::new()));
        }
        if !spec.delegates.is_empty() {
            extras
                .tools
                .extend(self.delegate_tools(&spec.delegates).await?);
        }
        let wanted: Vec<&str> = MEDIA_TOOLS
            .into_iter()
            .filter(|t| spec.tools.iter().any(|x| x == t))
            .collect();
        if !wanted.is_empty() {
            let settings = spec.media.over(&self.inner.config.media);
            extras.tools.extend(
                media_tools(
                    self.inner.models.clone(),
                    self.inner.store.clone(),
                    settings,
                    &wanted,
                )
                .map_err(AgentError::Invalid)?,
            );
        }
        if !spec.skills.is_empty() {
            let (skills, missing) = self.inner.skills.select(&spec.skills).await?;
            if !missing.is_empty() {
                return Err(AgentError::Invalid(format!(
                    "unknown skills: {}",
                    missing.join(", ")
                )));
            }
            if !skills.is_empty() {
                extras.prompt = gai_skills::index(&skills);
                extras.tools.push(gai_skills::load_skill_tool(skills));
            }
        }
        Ok(extras)
    }

    /// Check a callback URL before accepting a run that would POST to it.
    pub fn check_callback_url(&self, url: &str) -> Result<(), AgentError> {
        let u = reqwest::Url::parse(url)
            .map_err(|e| AgentError::Invalid(format!("callback_url: {e}")))?;
        if !matches!(u.scheme(), "http" | "https") {
            return Err(AgentError::Invalid("callback_url must be http(s)".into()));
        }
        let host = u.host_str().unwrap_or_default();
        if !self.inner.config.tools.allow_private_network
            && gai_tools::builtin::is_private_host(host, u.port_or_known_default().unwrap_or(80))
        {
            return Err(AgentError::Invalid(
                "callback_url points at a private network address (set tools.allow_private_network to allow it)".into(),
            ));
        }
        Ok(())
    }

    /// Validate a request, resolve its model and tools, and record the run.
    /// A bad request fails before anything is recorded.
    pub(crate) async fn create(
        &self,
        req: RunRequest,
        extra_meta: Map<String, Value>,
    ) -> Result<Launch, AgentError> {
        let spec = req.agent.clone();
        spec.validate().map_err(AgentError::Invalid)?;
        if let Some(url) = &req.callback_url {
            self.check_callback_url(url)?;
        }
        let model_spec = spec
            .model
            .clone()
            .or_else(|| self.default_model())
            .ok_or_else(|| {
                AgentError::Invalid(format!(
                    "agent {:?} names no model and there is no default model",
                    spec.name
                ))
            })?;
        let model = self.inner.models.chat(&model_spec)?;
        let summary_model = match &spec.context.summary_model {
            Some(s) => self.inner.models.chat(s)?,
            None => model.clone(),
        };
        if spec.tools.iter().any(|t| t.starts_with("mcp__")) {
            self.ensure_mcp();
            self.inner.mcp.wait_settled(Duration::from_secs(60)).await;
        }
        let (tools, unknown) = self
            .inner
            .tools
            .read()
            .expect("tools lock")
            .select(&spec.tools);
        let unknown: Vec<String> = unknown
            .into_iter()
            .filter(|t| !AGENT_TOOLS.contains(&t.as_str()))
            .collect();
        if !unknown.is_empty() {
            return Err(AgentError::Invalid(format!(
                "unknown tools: {}",
                unknown.join(", ")
            )));
        }
        if req.input.is_empty() {
            return Err(AgentError::Invalid("empty input".into()));
        }

        let store = &self.inner.store;
        let existing = match &req.session_id {
            Some(id) => Some(
                store
                    .get_session(id)
                    .await?
                    .ok_or_else(|| AgentError::NotFound(format!("session {id}")))?,
            ),
            None => None,
        };
        let user = req
            .user_id
            .clone()
            .or_else(|| existing.as_ref().and_then(|s| s.user_id.clone()))
            .unwrap_or_else(|| "default".to_owned());
        let extras = self.agent_extras(&spec, &user).await?;
        let persona = self.pick_persona(req.persona.as_ref(), &spec).await?;
        let session = match existing {
            Some(s) => s,
            None => {
                let first_text: String =
                    req.input.iter().filter_map(ContentBlock::as_text).collect();
                let title: String = first_text.chars().take(80).collect();
                store
                    .create_session(NewSession {
                        agent: Some(spec.name.clone()),
                        title: (!title.is_empty()).then_some(title),
                        user_id: req.user_id.clone(),
                        metadata: Value::Null,
                    })
                    .await?
            }
        };
        let workspace = spec.workspace.clone().unwrap_or_else(|| {
            self.inner
                .config
                .data_dir
                .join("workspaces")
                .join(&session.id)
        });
        std::fs::create_dir_all(&workspace)
            .map_err(|e| AgentError::Invalid(format!("workspace {}: {e}", workspace.display())))?;
        let system = join_prompt(
            system_with_persona(
                &spec,
                &model.info().id,
                &workspace,
                &req.vars,
                persona.as_ref(),
            )?,
            &extras.prompt,
        );

        let mut metadata = req.metadata.clone();
        if let Some(p) = &persona {
            metadata.insert(
                "persona".into(),
                serde_json::to_value(p).expect("serializable persona"),
            );
        }
        metadata.insert(
            "spec".into(),
            serde_json::to_value(&spec).expect("serializable spec"),
        );
        metadata.insert("model".into(), json!(model.info().id));
        metadata.insert("auto_approve".into(), json!(req.auto_approve));
        metadata.insert("vars".into(), Value::Object(req.vars.clone()));
        metadata.insert("workspace".into(), json!(workspace.display().to_string()));
        metadata.insert("user_id".into(), json!(user));
        if let Some(url) = &req.callback_url {
            metadata.insert("callback_url".into(), json!(url));
        }
        metadata.extend(extra_meta);
        let mut tools = tools;
        tools.extend(extras.tools);
        let run = store
            .create_run(NewRun {
                id: None,
                session_id: session.id.clone(),
                agent: spec.name.clone(),
                agent_version: req.agent_version,
                parent_run_id: req.parent_run_id.clone(),
                input: serde_json::to_value(&req.input).expect("serializable input"),
                metadata: Value::Object(metadata),
            })
            .await?;

        Ok(Launch {
            run_id: run.id,
            session_id: session.id,
            agent_version: req.agent_version,
            spec,
            model,
            summary_model,
            tools,
            system,
            workspace,
            auto_approve: req.auto_approve,
            memory: extras.memory,
        })
    }

    /// Mark a run as running here, so it can be subscribed to and cancelled
    /// even before its task starts. Reuses an existing registration.
    pub(crate) fn register_active(
        &self,
        run_id: &str,
    ) -> (
        broadcast::Sender<EventEnvelope>,
        broadcast::Receiver<EventEnvelope>,
        CancellationToken,
    ) {
        let mut active = self.inner.active.lock().expect("active lock");
        let a = active.entry(run_id.to_owned()).or_insert_with(|| Active {
            cancel: CancellationToken::new(),
            tx: broadcast::channel(4096).0,
        });
        (a.tx.clone(), a.tx.subscribe(), a.cancel.clone())
    }

    pub(crate) fn launch(&self, l: Launch, mode: Mode, resume: Resume) -> RunHandle {
        let (tx, rx, cancel) = self.register_active(&l.run_id);
        // Sending tools to a model that cannot call them only gets the
        // request rejected; run without them and say so.
        let (tools, notice) = if !l.tools.is_empty() && !l.model.info().capabilities.tools {
            let notice = format!(
                "{} cannot call tools, so this run has none; pick a tool-capable model to use them",
                l.model.info().id
            );
            (Vec::new(), Some(notice))
        } else {
            (l.tools, None)
        };
        let mut tool_specs: Vec<ToolSpec> = tools.iter().map(|t| t.spec()).collect();
        // Tool definitions go with every request. Keep them to well under
        // half of the model's window, so the conversation has room.
        let window = l
            .spec
            .context
            .max_context_tokens
            .or(l.model.info().capabilities.context_window)
            .unwrap_or(128_000) as u64;
        let fit = crate::context::fit_tools(&mut tool_specs, window * 2 / 5);
        let fit_notice = if fit.after > window * 4 / 5 {
            Some(format!(
                "This assistant's {} tools need about {} tokens, more than {} can hold ({} tokens). Choose fewer tools, or a model with a bigger context window.",
                tool_specs.len(),
                fit.after,
                l.model.info().id,
                window
            ))
        } else if fit.shortened {
            Some(format!(
                "Shortened the descriptions of {} tools (about {} → {} tokens) to fit {}'s {}-token context window.",
                tool_specs.len(),
                fit.before,
                fit.after,
                l.model.info().id,
                window
            ))
        } else {
            None
        };
        let notice = match (notice, fit_notice) {
            (Some(a), Some(b)) => Some(format!("{a}. {b}")),
            (a, b) => a.or(b),
        };
        let tool_map: HashMap<String, SharedTool> =
            tools.into_iter().map(|t| (t.spec().name, t)).collect();
        let ctx = ToolCtx {
            run_id: l.run_id.clone(),
            call_id: String::new(),
            session_id: l.session_id.clone(),
            agent: l.spec.name.clone(),
            workspace: l.workspace,
            cancel: cancel.child_token(),
            http: self.inner.http.clone(),
            settings: Arc::new(self.inner.config.tools.clone()),
        };
        let task = RunTask {
            rt: self.clone(),
            run_id: l.run_id.clone(),
            session_id: l.session_id.clone(),
            agent_version: l.agent_version,
            spec: l.spec,
            model: l.model,
            summary_model: l.summary_model,
            system: l.system,
            tools: tool_map,
            tool_specs,
            ctx,
            emitter: Emitter::new(&l.run_id, tx, self.inner.store.clone(), resume.event_seq),
            cancel,
            auto_approve: l.auto_approve,
            journal: AtomicU32::new(resume.journal_seq),
            waiting: AtomicUsize::new(0),
            steps: resume.steps,
            usage: resume.usage,
            cost: resume.cost,
            last_text: String::new(),
            hooks: self.inner.hooks.read().expect("hooks lock").clone(),
            completed_tools: resume.completed_tools,
            notice,
            memory: l.memory,
            memory_text: String::new(),
            replay: resume.replay,
            media: MediaResolver::default(),
            tool_cost: Mutex::new(0.0),
        };
        let rt = self.clone();
        let run_id = l.run_id.clone();
        let join = tokio::spawn(async move {
            let outcome = task.execute(mode).await;
            rt.inner.active.lock().expect("active lock").remove(&run_id);
            rt.finished(&outcome).await;
            outcome
        });
        RunHandle {
            run_id: l.run_id,
            session_id: l.session_id,
            events: rx,
            join,
        }
    }

    /// Continue a run that stopped without finishing (the process exited
    /// mid-run). Tool calls that finished are not repeated; calls that were
    /// waiting for approval ask again; calls that were in flight run again
    /// if their tool is idempotent, and otherwise go back through approval.
    pub async fn resume(&self, run_id: &str) -> Result<RunHandle, AgentError> {
        if self.is_active(run_id) {
            return Err(AgentError::Invalid(format!(
                "run {run_id} is already running"
            )));
        }
        let store = &self.inner.store;
        let run = store
            .get_run(run_id)
            .await?
            .ok_or_else(|| AgentError::NotFound(format!("run {run_id}")))?;
        if run.status.is_terminal() {
            return Err(AgentError::Invalid(format!(
                "run {run_id} already ended ({})",
                run.status.as_str()
            )));
        }
        if let Some(kind) = run_kind(&run) {
            return Err(AgentError::Invalid(format!(
                "run {run_id} is a {kind} run; resume it through its engine"
            )));
        }
        self.relaunch(run, false).await
    }

    /// Resume any unfinished run, of any kind, in the background.
    pub async fn resume_any(&self, run_id: &str) -> Result<(), AgentError> {
        let run = self
            .inner
            .store
            .get_run(run_id)
            .await?
            .ok_or_else(|| AgentError::NotFound(format!("run {run_id}")))?;
        if run.status.is_terminal() || self.is_active(run_id) {
            return Err(AgentError::Invalid(format!(
                "run {run_id} is {}",
                if self.is_active(run_id) {
                    "already running"
                } else {
                    run.status.as_str()
                }
            )));
        }
        match self.launcher_for(&run) {
            Some(launch) => {
                tokio::spawn(launch(run, false));
            }
            None => {
                let h = self.resume(run_id).await?;
                tokio::spawn(h.wait());
            }
        }
        Ok(())
    }

    /// Start a recorded run from its stored snapshot: a queued run from the
    /// beginning (`fresh`), or an interrupted one from where it stopped.
    async fn relaunch(&self, run: Run, fresh: bool) -> Result<RunHandle, AgentError> {
        let store = &self.inner.store;
        let run_id = run.id.as_str();
        let meta = run.metadata.as_object().cloned().unwrap_or_default();
        let spec: AgentSpec = meta
            .get("spec")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
            .ok_or_else(|| {
                AgentError::Invalid(format!("run {run_id} has no agent snapshot to resume from"))
            })?;
        let model_spec = meta
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| spec.model.clone())
            .or_else(|| self.default_model())
            .ok_or_else(|| AgentError::Invalid("no model to resume with".into()))?;
        let model = self.inner.models.chat(&model_spec)?;
        let summary_model = match &spec.context.summary_model {
            Some(s) => self.inner.models.chat(s)?,
            None => model.clone(),
        };
        if spec.tools.iter().any(|t| t.starts_with("mcp__")) {
            self.ensure_mcp();
            self.inner.mcp.wait_settled(Duration::from_secs(60)).await;
        }
        let (tools, unknown) = self
            .inner
            .tools
            .read()
            .expect("tools lock")
            .select(&spec.tools);
        let unknown: Vec<&String> = unknown
            .iter()
            .filter(|t| !AGENT_TOOLS.contains(&t.as_str()))
            .collect();
        if !unknown.is_empty() {
            tracing::warn!(run = run_id, tools = ?unknown, "running without tools that are no longer available");
        }
        let user = meta
            .get("user_id")
            .and_then(Value::as_str)
            .unwrap_or("default")
            .to_owned();
        let extras = self.agent_extras(&spec, &user).await?;
        let mut tools = tools;
        tools.extend(extras.tools);
        let vars = meta
            .get("vars")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let workspace = meta
            .get("workspace")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                self.inner
                    .config
                    .data_dir
                    .join("workspaces")
                    .join(&run.session_id)
            });
        std::fs::create_dir_all(&workspace)
            .map_err(|e| AgentError::Invalid(format!("workspace {}: {e}", workspace.display())))?;
        // The persona the run started with, even if it has changed since.
        let persona: Option<ResolvedPersona> = meta
            .get("persona")
            .and_then(|p| serde_json::from_value(p.clone()).ok());
        let system = join_prompt(
            system_with_persona(&spec, &model.info().id, &workspace, &vars, persona.as_ref())?,
            &extras.prompt,
        );

        let steps = store.list_steps(run_id).await?;
        let mut completed_tools = HashMap::new();
        for st in &steps {
            if st.kind == StepKind::Tool
                && st.status != StepStatus::Started
                && let (Some(id), Some(out)) = (
                    st.input.as_ref().and_then(|i| i["call_id"].as_str()),
                    st.output
                        .as_ref()
                        .and_then(|o| serde_json::from_value::<ToolOutput>(o.clone()).ok()),
                )
            {
                completed_tools.insert(id.to_owned(), out);
            }
        }
        let event_seq = store
            .list_events(run_id, None, u32::MAX)
            .await?
            .last()
            .map(|e| e.seq)
            .unwrap_or(0);
        // A fork that stopped part-way keeps replaying its recording.
        let replay = match meta
            .get("fork")
            .and_then(|f| serde_json::from_value::<ForkInfo>(f.clone()).ok())
        {
            Some(fork) => Some(Arc::new(self.load_replay(&fork).await?)),
            None => None,
        };
        let resume = Resume {
            event_seq,
            journal_seq: steps.iter().map(|s| s.seq).max().unwrap_or(0),
            steps: run.steps,
            usage: run.usage,
            cost: run.cost,
            completed_tools,
            replay,
        };
        let auto_approve = meta
            .get("auto_approve")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        // A run that never got as far as saving its input starts over.
        let started = !fresh
            && (run.steps > 0
                || store
                    .list_messages(&run.session_id)
                    .await?
                    .iter()
                    .any(|m| m.run_id.as_deref() == Some(run_id)));
        let mode = if started {
            Mode::Resume
        } else {
            let input: Vec<ContentBlock> = serde_json::from_value(run.input.clone())
                .map_err(|e| AgentError::Invalid(format!("run {run_id} input: {e}")))?;
            Mode::Start(input)
        };
        Ok(self.launch(
            Launch {
                run_id: run.id,
                session_id: run.session_id,
                agent_version: run.agent_version,
                spec,
                model,
                summary_model,
                tools,
                system,
                workspace,
                auto_approve,
                memory: extras.memory,
            },
            mode,
            resume,
        ))
    }

    /// Finish hooks, then the run's callback, if it has one.
    pub async fn finished(&self, outcome: &RunOutcome) {
        let hooks: Vec<FinishHook> = self
            .inner
            .finish_hooks
            .read()
            .expect("hooks lock")
            .values()
            .cloned()
            .collect();
        for h in hooks {
            h(outcome.clone()).await;
        }
        let url = match self.inner.store.get_run(&outcome.run_id).await {
            Ok(Some(run)) => run.metadata["callback_url"].as_str().map(str::to_owned),
            _ => None,
        };
        if let Some(url) = url {
            let (http, body) = (self.inner.http.clone(), outcome.clone());
            tokio::spawn(post_callback(http, url, body));
        }
    }

    /// Resume every run the store says is unfinished but that is not
    /// running in this process. Call once at startup. Returns the resumed
    /// run ids; runs that cannot resume are marked failed.
    pub async fn recover(&self) -> Vec<String> {
        let mut resumed = Vec::new();
        // Runs other engines own start last, so the agent runs they wait on
        // are already running again and are not resumed twice.
        let mut later = Vec::new();
        for status in [
            RunStatus::Running,
            RunStatus::AwaitingApproval,
            RunStatus::Queued,
        ] {
            let filter = gai_store::RunFilter {
                status: Some(status),
                limit: Some(1000),
                ..Default::default()
            };
            let runs = match self.inner.store.list_runs(filter).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "could not list unfinished runs");
                    continue;
                }
            };
            for r in runs.into_iter().filter(|r| !self.is_active(&r.id)) {
                if r.status == RunStatus::Queued && r.metadata["background"] == json!(true) {
                    continue; // the queue starts these
                }
                if let Some(launch) = self.launcher_for(&r) {
                    later.push((launch, r));
                    continue;
                }
                match self.resume(&r.id).await {
                    Ok(_) => resumed.push(r.id),
                    Err(_) if self.is_active(&r.id) => {}
                    Err(e) => {
                        tracing::warn!(run = %r.id, error = %e, "could not resume run");
                        let update = RunUpdate {
                            status: Some(RunStatus::Failed),
                            error: Some(format!("could not resume after restart: {e}")),
                            ..Default::default()
                        };
                        let _ = self.inner.store.update_run(&r.id, update).await;
                    }
                }
            }
        }
        for (launch, r) in later {
            resumed.push(r.id.clone());
            tokio::spawn(launch(r, false));
        }
        resumed
    }
}

/// Everything resolved for a run before its task starts.
pub(crate) struct Launch {
    run_id: String,
    session_id: String,
    agent_version: Option<u32>,
    spec: AgentSpec,
    model: SharedChatModel,
    summary_model: SharedChatModel,
    tools: Vec<SharedTool>,
    system: String,
    workspace: PathBuf,
    auto_approve: bool,
    memory: Option<Arc<RunMemory>>,
}

/// What an agent's `memory`, `knowledge` and `skills` settings add to a run.
#[derive(Default)]
struct Extras {
    tools: Vec<SharedTool>,
    /// Appended to the system prompt (the skills index).
    prompt: String,
    memory: Option<Arc<RunMemory>>,
}

fn join_prompt(mut system: String, extra: &str) -> String {
    if !extra.is_empty() {
        if !system.is_empty() {
            system.push_str("\n\n");
        }
        system.push_str(extra);
    }
    system
}

/// POST a finished run's outcome to its callback URL, retrying a few times.
async fn post_callback(http: reqwest::Client, url: String, outcome: RunOutcome) {
    for attempt in 0..3u32 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(2u64.pow(attempt * 2))).await;
        }
        let r = http
            .post(&url)
            .header("x-gai-event", "run.finished")
            .timeout(Duration::from_secs(15))
            .json(&outcome)
            .send()
            .await;
        match r {
            Ok(resp) if resp.status().is_success() => return,
            Ok(resp) => {
                tracing::warn!(run = %outcome.run_id, %url, status = %resp.status(), "run callback was refused")
            }
            Err(e) => {
                tracing::warn!(run = %outcome.run_id, %url, error = %e, "run callback failed")
            }
        }
    }
}

pub(crate) enum Mode {
    Start(Vec<ContentBlock>),
    Resume,
}

/// Progress carried over when a run resumes.
#[derive(Default)]
pub(crate) struct Resume {
    event_seq: u64,
    journal_seq: u32,
    steps: u32,
    usage: Usage,
    cost: f64,
    /// Tool results already journaled, by call id.
    completed_tools: HashMap<String, ToolOutput>,
    /// For forks: the recording earlier steps come from.
    pub(crate) replay: Option<Arc<Replay>>,
}

impl Resume {
    /// A fresh start that replays a recording.
    pub(crate) fn replaying(replay: Replay) -> Self {
        Self {
            replay: Some(Arc::new(replay)),
            ..Default::default()
        }
    }
}

/// The agent's rendered prompt with the persona put in its place.
fn system_with_persona(
    spec: &AgentSpec,
    model: &str,
    workspace: &std::path::Path,
    vars: &Map<String, Value>,
    persona: Option<&ResolvedPersona>,
) -> Result<String, AgentError> {
    let Some(p) = persona else {
        return render_system(spec, model, workspace, vars, "");
    };
    let text = p.spec.render();
    let placed = spec.system.as_deref().is_some_and(template_places_persona);
    let system = render_system(spec, model, workspace, vars, &text)?;
    Ok(compose(system, &text, p.placement, placed))
}

fn render_system(
    spec: &AgentSpec,
    model: &str,
    workspace: &std::path::Path,
    vars: &Map<String, Value>,
    persona: &str,
) -> Result<String, AgentError> {
    let Some(template) = spec.system.as_deref() else {
        return Ok(String::new());
    };
    let now = chrono::Utc::now();
    let mut ctx = Map::new();
    ctx.insert("agent".into(), json!(spec.name));
    ctx.insert("model".into(), json!(model));
    ctx.insert("date".into(), json!(now.format("%Y-%m-%d").to_string()));
    ctx.insert(
        "datetime".into(),
        json!(now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
    );
    ctx.insert("workspace".into(), json!(workspace.display().to_string()));
    ctx.insert("persona".into(), json!(persona));
    ctx.insert("vars".into(), Value::Object(vars.clone()));
    for (k, v) in vars {
        ctx.entry(k.clone()).or_insert_with(|| v.clone());
    }
    let mut env = minijinja::Environment::new();
    env.set_keep_trailing_newline(true);
    env.render_str(template, Value::Object(ctx))
        .map_err(|e| AgentError::Template(e.to_string()))
}

/// Which model a router picked for a call.
struct Routed {
    model: String,
    reason: String,
    pricing: Option<gai_core::Pricing>,
    extra_cost: f64,
}

/// Why the loop stopped, before it is turned into a status.
enum Finish {
    Done,
    Refused,
    Budget(String),
    Failed(String),
}

struct RunTask {
    rt: Runtime,
    run_id: String,
    session_id: String,
    agent_version: Option<u32>,
    spec: AgentSpec,
    model: SharedChatModel,
    summary_model: SharedChatModel,
    system: String,
    tools: HashMap<String, SharedTool>,
    tool_specs: Vec<ToolSpec>,
    ctx: ToolCtx,
    emitter: Emitter,
    cancel: CancellationToken,
    auto_approve: bool,
    /// Journal sequence for steps (model calls, tool calls, approvals).
    journal: AtomicU32,
    /// Tool calls currently waiting for approval.
    waiting: AtomicUsize,
    steps: u32,
    usage: Usage,
    cost: f64,
    last_text: String,
    hooks: Vec<Arc<dyn RunHook>>,
    /// On resume: results of tool calls that finished before the restart.
    completed_tools: HashMap<String, ToolOutput>,
    /// Shown as a warning when the run starts.
    notice: Option<String>,
    /// The agent's memory blocks, re-read before every model call so edits
    /// show up at once.
    memory: Option<Arc<RunMemory>>,
    memory_text: String,
    replay: Option<Arc<Replay>>,
    /// Blob bytes for requests, read once per run.
    media: MediaResolver,
    /// Cost of tool calls since the last model step.
    tool_cost: Mutex<f64>,
}

impl RunTask {
    /// The recording, while the current step is one a fork replays.
    fn replaying(&self) -> Option<&Replay> {
        self.replay.as_deref().filter(|r| self.steps < r.from_step)
    }

    fn store(&self) -> &SharedStore {
        &self.rt.inner.store
    }

    async fn emit(&self, e: RunEvent) {
        self.emitter.emit(e).await;
    }

    /// Log store failures without failing the run: the run's own progress
    /// matters more than its bookkeeping.
    fn soft<T>(&self, r: Result<T, StoreError>) -> Option<T> {
        r.map_err(|e| tracing::warn!(run = %self.run_id, error = %e, "store write failed"))
            .ok()
    }

    async fn set_status(&self, status: RunStatus) {
        let r = self
            .store()
            .update_run(
                &self.run_id,
                RunUpdate {
                    status: Some(status),
                    ..Default::default()
                },
            )
            .await;
        self.soft(r);
    }

    async fn execute(mut self, mode: Mode) -> RunOutcome {
        self.set_status(RunStatus::Running).await;
        self.emit(RunEvent::RunStarted {
            agent: self.spec.name.clone(),
            agent_version: self.agent_version,
            model: self.model.info().id.clone(),
            session_id: self.session_id.clone(),
        })
        .await;

        if let Some(message) = self.notice.take() {
            self.emit(RunEvent::Warning { message }).await;
        }
        let prepared = match mode {
            Mode::Start(input) => self.prepare(input).await,
            Mode::Resume => self.prepare_resume().await,
        };
        let finish = match prepared {
            Err(e) => Finish::Failed(e.to_string()),
            Ok(mut conv) => {
                let cancel = self.cancel.clone();
                let timeout = self.spec.limits.timeout_secs;
                let deadline = async move {
                    match timeout {
                        Some(s) => tokio::time::sleep(Duration::from_secs(s)).await,
                        None => std::future::pending().await,
                    }
                };
                tokio::select! {
                    f = self.drive(&mut conv) => f,
                    _ = cancel.cancelled() => Finish::Failed("cancelled".into()),
                    _ = deadline => Finish::Budget(format!("time limit of {}s reached", timeout.unwrap_or_default())),
                }
            }
        };
        self.cancel.cancel(); // stop any tool still running
        for p in self.rt.inner.approvals.pending(Some(&self.run_id)) {
            self.rt.inner.approvals.forget(&self.run_id, &p.call_id);
        }

        // Tools that ran after the last model step (or on resume).
        self.take_tool_cost();
        let (status, error) = match finish {
            Finish::Done => (RunStatus::Completed, None),
            Finish::Refused => (
                RunStatus::Failed,
                Some("the model declined the request".to_owned()),
            ),
            Finish::Budget(why) => (RunStatus::BudgetExceeded, Some(why)),
            Finish::Failed(why) if why == "cancelled" && self.cancel.is_cancelled() => {
                (RunStatus::Cancelled, None)
            }
            Finish::Failed(why) => (RunStatus::Failed, Some(why)),
        };
        let output = (!self.last_text.is_empty()).then(|| self.structured_output());
        let r = self
            .store()
            .update_run(
                &self.run_id,
                RunUpdate {
                    status: Some(status),
                    output: output.clone(),
                    error: error.clone(),
                    usage: Some(self.usage),
                    cost: Some(self.cost),
                    steps: Some(self.steps),
                },
            )
            .await;
        self.soft(r);
        self.emit(RunEvent::RunFinished {
            status,
            output: output.clone(),
            error: error.clone(),
            usage: self.usage,
            cost: self.cost,
            steps: self.steps,
        })
        .await;
        RunOutcome {
            run_id: self.run_id.clone(),
            session_id: self.session_id.clone(),
            status,
            output,
            text: self.last_text.clone(),
            error,
            usage: self.usage,
            cost: self.cost,
            steps: self.steps,
        }
    }

    fn structured_output(&self) -> Value {
        if self.spec.output_schema.is_some()
            && let Ok(v) = serde_json::from_str::<Value>(strip_fences(&self.last_text))
        {
            return v;
        }
        Value::String(self.last_text.clone())
    }

    /// Load history, repair it, and append the new user input.
    async fn prepare(&self, input: Vec<ContentBlock>) -> Result<Conversation, AgentError> {
        let history = self.store().load_history(&self.session_id).await?;
        let mut conv = Conversation::from_history(history);
        let repaired = conv.normalize();
        if repaired > 0 {
            self.emit(RunEvent::Warning {
                message: format!(
                    "{repaired} interrupted tool call(s) from an earlier run were marked as failed"
                ),
            })
            .await;
        }
        let msg = Message {
            role: Role::User,
            content: input,
        };
        let seq = self
            .store()
            .append_messages(
                &self.session_id,
                Some(&self.run_id),
                std::slice::from_ref(&msg),
            )
            .await?;
        conv.push(msg, seq.first().copied());
        Ok(conv)
    }

    /// Rebuild the conversation of an interrupted run and finish the tool
    /// calls it was in the middle of.
    async fn prepare_resume(&self) -> Result<Conversation, AgentError> {
        self.emit(RunEvent::Warning {
            message: "resumed after the server restarted".into(),
        })
        .await;
        let history = self.store().load_history(&self.session_id).await?;
        let mut conv = Conversation::from_history(history);
        let pending: Vec<(String, String, Value)> = match conv.messages.last() {
            Some(m) if m.role == Role::Assistant => m
                .tool_uses()
                .map(|(id, name, input)| (id.to_owned(), name.to_owned(), input.clone()))
                .collect(),
            _ => Vec::new(),
        };
        if !pending.is_empty() {
            let results = self.run_tools(pending, true).await;
            let msg = Message {
                role: Role::User,
                content: results,
            };
            let seq = self
                .store()
                .append_messages(
                    &self.session_id,
                    Some(&self.run_id),
                    std::slice::from_ref(&msg),
                )
                .await?;
            conv.push(msg, seq.first().copied());
        }
        conv.normalize();
        Ok(conv)
    }

    fn hook_ctx(&self) -> HookCtx {
        HookCtx {
            run_id: self.run_id.clone(),
            session_id: self.session_id.clone(),
            agent: self.spec.name.clone(),
            step: self.steps,
        }
    }

    async fn drive(&mut self, conv: &mut Conversation) -> Finish {
        let mut compacted_for_overflow = false;
        loop {
            if self.steps >= self.spec.limits.max_steps {
                return Finish::Budget(format!(
                    "step limit of {} reached",
                    self.spec.limits.max_steps
                ));
            }
            self.steps += 1;
            let step = self.steps;
            self.emit(RunEvent::StepStarted { step }).await;
            if let Some(m) = &self.memory {
                self.memory_text = m.render().await;
            }
            self.manage_context(conv).await;

            let req = self.request(conv);
            let hook_ctx = self.hook_ctx();
            let sent = conv.messages.len();
            let recorded = self.replaying().and_then(|r| {
                r.steps
                    .get(&step)
                    .map(|st| (r.from_run.clone(), st.clone()))
            });
            let (resp, replayed) = match recorded {
                Some((from_run, rec)) => {
                    self.replay_model_step(step, sent, &from_run, &rec).await;
                    let resp = gai_llm::ChatResponse {
                        id: String::new(),
                        model: self.model.info().id.clone(),
                        content: rec.content,
                        stop_reason: rec.stop_reason,
                        usage: rec.usage,
                    };
                    ((resp, String::new(), None), true)
                }
                None => match self
                    .live_model_step(
                        req,
                        step,
                        sent,
                        &hook_ctx,
                        &mut compacted_for_overflow,
                        conv,
                    )
                    .await
                {
                    Ok(Some(live)) => (live, false),
                    Ok(None) => continue,
                    Err(f) => return f,
                },
            };
            let (resp, model_id, cost) = resp;
            conv.calibrate(resp.usage.total_input(), sent);
            if !replayed {
                self.usage += resp.usage;
                self.cost += cost.unwrap_or(0.0);
                self.emit(RunEvent::Usage {
                    step,
                    model: model_id,
                    usage: resp.usage,
                    cost,
                    total_usage: self.usage,
                    total_cost: self.cost,
                })
                .await;
            }

            let stop = resp.stop_reason.clone();
            let message = resp.into_message();
            let text = message.text();
            if !text.trim().is_empty() {
                self.last_text = text;
            }
            let calls: Vec<(String, String, Value)> = message
                .tool_uses()
                .map(|(id, name, input)| (id.to_owned(), name.to_owned(), input.clone()))
                .collect();
            if !message.content.is_empty() {
                let seq = self
                    .store()
                    .append_messages(
                        &self.session_id,
                        Some(&self.run_id),
                        std::slice::from_ref(&message),
                    )
                    .await;
                let seq = self.soft(seq).and_then(|s| s.first().copied());
                self.emit(RunEvent::AssistantMessage {
                    message: message.clone(),
                })
                .await;
                conv.push(message, seq);
            }

            if let Some(why) = self.over_budget() {
                return Finish::Budget(why);
            }
            if !calls.is_empty() {
                let results = self.run_tools(calls, false).await;
                self.take_tool_cost();
                let msg = Message {
                    role: Role::User,
                    content: results,
                };
                let seq = self
                    .store()
                    .append_messages(
                        &self.session_id,
                        Some(&self.run_id),
                        std::slice::from_ref(&msg),
                    )
                    .await;
                let seq = self.soft(seq).and_then(|s| s.first().copied());
                conv.push(msg, seq);
                continue;
            }
            match stop {
                StopReason::PauseTurn => continue,
                StopReason::Refusal => return Finish::Refused,
                StopReason::MaxTokens => {
                    self.emit(RunEvent::Warning {
                        message: "the reply hit the output token limit and may be cut off".into(),
                    })
                    .await;
                    return Finish::Done;
                }
                _ => return Finish::Done,
            }
        }
    }

    /// Journal a model step taken from a fork's recording. Its output is
    /// the original's, byte for byte; nothing is sent to the model.
    async fn replay_model_step(&self, step: u32, sent: usize, from_run: &str, rec: &RecordedStep) {
        let seq = self.next_journal();
        let input = json!({"messages": sent, "step": step, "replayed_from": from_run});
        let r = self
            .store()
            .start_step(
                &self.run_id,
                seq,
                StepKind::Model,
                Some(rec.model.as_deref().unwrap_or(&self.model.info().id)),
                Some(&input),
            )
            .await;
        self.soft(r);
        let r = self
            .store()
            .finish_step(
                &self.run_id,
                seq,
                StepStatus::Completed,
                Some(&rec.output),
                None,
                None,
            )
            .await;
        self.soft(r);
        self.emit(RunEvent::StepReplayed {
            step,
            from_run_id: from_run.to_owned(),
        })
        .await;
        let text: String = rec
            .content
            .iter()
            .filter_map(ContentBlock::as_text)
            .collect();
        if !text.is_empty() {
            self.emit(RunEvent::TextDelta { text }).await;
        }
    }

    /// Add what tool calls cost since the last model step to the run.
    fn take_tool_cost(&mut self) {
        let c = std::mem::take(&mut *self.tool_cost.lock().expect("tool cost"));
        self.cost += c;
    }

    /// Call the model for a step and journal the call. Returns the reply,
    /// the model that wrote it (a router's pick, for `router/*`) and its
    /// cost. `Ok(None)` means the context overflowed and was compacted:
    /// take the step again.
    async fn live_model_step(
        &mut self,
        mut req: ChatRequest,
        step: u32,
        sent: usize,
        hook_ctx: &HookCtx,
        compacted_for_overflow: &mut bool,
        conv: &mut Conversation,
    ) -> Result<Option<(gai_llm::ChatResponse, String, Option<f64>)>, Finish> {
        for h in &self.hooks {
            if let Err(Blocked(why)) = h.before_model(hook_ctx, &mut req).await {
                return Err(Finish::Failed(format!(
                    "blocked by the {} hook: {why}",
                    h.name()
                )));
            }
        }
        let seq = self.next_journal();
        let mut input = json!({"messages": sent, "step": step});
        if self.rt.inner.config.record_requests {
            input["request"] = serde_json::to_value(&req).expect("serializable request");
        }
        let r = self
            .store()
            .start_step(
                &self.run_id,
                seq,
                StepKind::Model,
                Some(&self.model.info().id),
                Some(&input),
            )
            .await;
        self.soft(r);
        // Blob references become bytes only now, so the journal keeps ids.
        let caps = self.model.info().capabilities.clone();
        self.media
            .prepare(self.store(), &mut req.messages, caps.vision, caps.audio)
            .await;

        let (mut resp, routed) = match self.call_model(req).await {
            Ok(r) => r,
            Err(e) => {
                let r = self
                    .store()
                    .finish_step(
                        &self.run_id,
                        seq,
                        StepStatus::Failed,
                        Some(&json!(e.to_string())),
                        None,
                        None,
                    )
                    .await;
                self.soft(r);
                if matches!(e, LlmError::ContextLength(_)) && !*compacted_for_overflow {
                    *compacted_for_overflow = true;
                    self.emit(RunEvent::Warning {
                        message: "context window exceeded; compacting and retrying".into(),
                    })
                    .await;
                    conv.clear_tool_results(self.spec.context.keep_tool_results);
                    self.compact(conv).await;
                    self.steps -= 1;
                    return Ok(None);
                }
                return Err(Finish::Failed(e.to_string()));
            }
        };
        for h in &self.hooks {
            if let Err(Blocked(why)) = h.after_model(hook_ctx, &mut resp).await {
                return Err(Finish::Failed(format!(
                    "blocked by the {} hook: {why}",
                    h.name()
                )));
            }
        }
        let (model_id, cost) = match &routed {
            Some(r) => (
                r.model.clone(),
                r.pricing
                    .map(|p| p.cost(&resp.usage) + r.extra_cost)
                    .or((r.extra_cost > 0.0).then_some(r.extra_cost)),
            ),
            None => (
                self.model.info().id.clone(),
                self.model.info().cost(&resp.usage),
            ),
        };
        let mut out = json!({"stop_reason": resp.stop_reason, "content": resp.content});
        if let Some(r) = &routed {
            out["routed"] = json!({"model": r.model, "reason": r.reason});
        }
        let r = self
            .store()
            .finish_step(
                &self.run_id,
                seq,
                StepStatus::Completed,
                Some(&out),
                Some(&resp.usage),
                cost,
            )
            .await;
        self.soft(r);
        let r = self
            .store()
            .record_usage(Some(&self.run_id), &model_id, &resp.usage, cost)
            .await;
        self.soft(r);
        Ok(Some((resp, model_id, cost)))
    }

    fn over_budget(&self) -> Option<String> {
        let l = &self.spec.limits;
        if let Some(max) = l.max_tokens
            && self.usage.total() > max
        {
            return Some(format!(
                "token budget of {max} exceeded ({} used)",
                self.usage.total()
            ));
        }
        if let Some(max) = l.max_cost_usd
            && self.cost > max
        {
            return Some(format!(
                "cost budget of ${max:.4} exceeded (${:.4} spent)",
                self.cost
            ));
        }
        None
    }

    fn next_journal(&self) -> u32 {
        self.journal.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn system_prompt(&self, conv: &Conversation) -> Option<String> {
        // Stable parts first, so provider prompt caching covers them.
        let mut s = join_prompt(self.system.clone(), &self.memory_text);
        if let Some(summary) = &conv.summary {
            if !s.is_empty() {
                s.push_str("\n\n");
            }
            s.push_str("<conversation_summary>\nEarlier parts of this conversation, summarized:\n");
            s.push_str(summary);
            s.push_str("\n</conversation_summary>");
        }
        (!s.is_empty()).then_some(s)
    }

    fn request(&self, conv: &Conversation) -> ChatRequest {
        let mut req = ChatRequest::new(conv.messages.clone());
        req.system = self.system_prompt(conv);
        req.tools = self.tool_specs.clone();
        req.reasoning = self.spec.reasoning.clone();
        req.max_tokens = self.spec.max_tokens;
        req.temperature = self.spec.temperature.map(|t| t as f32);
        req.cache = self.spec.context.cache;
        if let Some(schema) = &self.spec.output_schema {
            req.response_format = Some(ResponseFormat::JsonSchema {
                name: "output".into(),
                schema: schema.clone(),
                strict: true,
            });
        }
        req
    }

    async fn call_model(
        &self,
        req: ChatRequest,
    ) -> Result<(gai_llm::ChatResponse, Option<Routed>), LlmError> {
        let mut stream = self.model.stream(req).await?;
        let mut acc = Accumulator::new(&self.model.info().id);
        let mut routed = None;
        while let Some(ev) = stream.next().await {
            let ev = ev?;
            match &ev {
                ChatEvent::Routed {
                    model,
                    reason,
                    pricing,
                    extra_cost,
                } => {
                    self.emit(RunEvent::ModelRouted {
                        step: self.steps,
                        model: model.clone(),
                        reason: reason.clone(),
                    })
                    .await;
                    routed = Some(Routed {
                        model: model.clone(),
                        reason: reason.clone(),
                        pricing: *pricing,
                        extra_cost: *extra_cost,
                    });
                }
                ChatEvent::TextDelta { text, .. } => {
                    self.emit(RunEvent::TextDelta { text: text.clone() }).await
                }
                ChatEvent::ReasoningDelta { text, .. } => {
                    self.emit(RunEvent::ReasoningDelta { text: text.clone() })
                        .await
                }
                _ => {}
            }
            acc.push(&ev);
        }
        Ok((acc.finish(), routed))
    }

    /// Longest tool result passed back, in characters: the configured limit,
    /// but never more than about a quarter of the context window, so one
    /// big page cannot crowd out the rest of the conversation (at roughly
    /// 3.5 characters a token).
    fn max_tool_output_chars(&self) -> usize {
        let quarter = (self.window() as f64 * 0.25 * 3.5) as usize;
        self.ctx.settings.max_output_chars.min(quarter.max(2_000))
    }

    fn window(&self) -> u64 {
        self.spec
            .context
            .max_context_tokens
            .or(self.model.info().capabilities.context_window)
            .unwrap_or(128_000) as u64
    }

    async fn manage_context(&self, conv: &mut Conversation) {
        let window = self.window() as f64;
        let c = &self.spec.context;
        let system = self.system_prompt(conv).unwrap_or_default();
        if conv.estimate(&system, &self.tool_specs) as f64 > window * c.clear_tool_results_at {
            let cleared = conv.clear_tool_results(c.keep_tool_results);
            if cleared > 0 {
                self.emit(RunEvent::ToolResultsCleared { cleared }).await;
            }
        }
        if conv.estimate(&system, &self.tool_specs) as f64 > window * c.compact_at {
            self.compact(conv).await;
        }
    }

    /// Summarize older messages and continue from the summary.
    async fn compact(&self, conv: &mut Conversation) {
        let Some(cut) = conv.compaction_cut(self.spec.context.keep_recent_messages) else {
            self.emit(RunEvent::Warning {
                message: "context is nearly full but there is nothing left to summarize".into(),
            })
            .await;
            return;
        };
        let mut prompt = String::new();
        if let Some(prev) = &conv.summary {
            prompt.push_str(&format!(
                "<previous_summary>\n{prev}\n</previous_summary>\n\n"
            ));
        }
        prompt.push_str(&format!(
            "<transcript>\n{}</transcript>\n\nWrite a summary of the conversation above (including the previous summary, if any) that lets an AI agent continue the work without the transcript. Keep: the user's goals, instructions and constraints; decisions made and why; concrete facts learned (names, paths, numbers, URLs, results of tool calls that still matter); and what remains to be done. Be specific and concise. Plain text, no preamble.",
            transcript(&conv.messages[..cut])
        ));
        let mut req = ChatRequest::user(prompt).with_system(
            "You condense agent conversations into faithful working summaries. Never invent facts.",
        );
        req.max_tokens = Some(4_000);
        req.cache = false;
        let seq = self.next_journal();
        // A fork replays the original's summary rather than writing a new one.
        let recorded = self.replaying().and_then(|r| {
            r.compactions
                .lock()
                .expect("compactions lock")
                .pop_front()
                .map(|s| (r.from_run.clone(), s))
        });
        let mut input = json!({"messages": cut});
        if let Some((from, _)) = &recorded {
            input["replayed_from"] = json!(from);
        }
        let r = self
            .store()
            .start_step(
                &self.run_id,
                seq,
                StepKind::Compaction,
                Some(&self.summary_model.info().id),
                Some(&input),
            )
            .await;
        self.soft(r);
        let result = match recorded {
            Some((_, summary)) => Ok((summary, None, None)),
            None => self.summary_model.complete(req).await.map(|resp| {
                let cost = self.summary_model.info().cost(&resp.usage);
                (resp.text(), Some(resp.usage), cost)
            }),
        };
        match result {
            Ok((summary, usage, cost)) => {
                if let Some(usage) = &usage {
                    let r = self
                        .store()
                        .record_usage(
                            Some(&self.run_id),
                            &self.summary_model.info().id,
                            usage,
                            cost,
                        )
                        .await;
                    self.soft(r);
                }
                let r = self
                    .store()
                    .finish_step(
                        &self.run_id,
                        seq,
                        StepStatus::Completed,
                        Some(&json!(summary)),
                        usage.as_ref(),
                        cost,
                    )
                    .await;
                self.soft(r);
                if let Some(covers) = conv.seqs[..cut].iter().rev().find_map(|s| *s) {
                    let r = self
                        .store()
                        .add_compaction(&self.session_id, Some(&self.run_id), covers, &summary)
                        .await;
                    self.soft(r);
                }
                let chars = summary.len();
                conv.apply_compaction(cut, summary);
                self.emit(RunEvent::Compacted {
                    summarized_messages: cut,
                    summary_chars: chars,
                })
                .await;
            }
            Err(e) => {
                let r = self
                    .store()
                    .finish_step(
                        &self.run_id,
                        seq,
                        StepStatus::Failed,
                        Some(&json!(e.to_string())),
                        None,
                        None,
                    )
                    .await;
                self.soft(r);
                self.emit(RunEvent::Warning {
                    message: format!("could not summarize the conversation: {e}"),
                })
                .await;
            }
        }
    }

    async fn run_tools(
        &self,
        calls: Vec<(String, String, Value)>,
        resumed: bool,
    ) -> Vec<ContentBlock> {
        let sem = Semaphore::new(self.spec.limits.max_parallel_tools.max(1));
        let futures = calls
            .into_iter()
            .map(|(id, name, input)| self.run_tool(&sem, id, name, input, resumed));
        futures::future::join_all(futures).await
    }

    async fn run_tool(
        &self,
        sem: &Semaphore,
        id: String,
        name: String,
        input: Value,
        resumed: bool,
    ) -> ContentBlock {
        let tool = self.tools.get(&name);
        let mut policy = self
            .spec
            .tool_policy
            .get(&name)
            .copied()
            .or_else(|| tool.map(|t| t.default_policy()))
            .unwrap_or(ToolPolicy::Auto);
        // A call that was in flight when the process stopped may have had
        // effects already; only repeat it unasked if repeating is harmless.
        if resumed && policy == ToolPolicy::Auto && tool.is_some_and(|t| !t.idempotent()) {
            policy = ToolPolicy::Approve;
        }
        self.emit(RunEvent::ToolCall {
            id: id.clone(),
            name: name.clone(),
            input: input.clone(),
            policy,
        })
        .await;
        // Execution time only: time spent waiting for approval is not the tool's.
        let mut exec_ms = 0;
        let recorded = self.replaying().and_then(|r| r.tools.get(&id));
        let mut output = match (self.completed_tools.get(&id), recorded) {
            // Already paid for: by the run before a restart, or by the
            // run a fork replays.
            (Some(done), _) => ToolOutput {
                cost: None,
                ..done.clone()
            },
            (None, Some(rec)) => {
                self.journal_replayed_tool(&name, rec).await;
                ToolOutput {
                    cost: None,
                    ..rec.result.clone()
                }
            }
            (None, None) => {
                self.tool_output(sem, tool, policy, &id, &name, input, &mut exec_ms)
                    .await
            }
        };
        if let Some(c) = output.cost.take() {
            *self.tool_cost.lock().expect("tool cost") += c;
        }
        let output = output.truncated(self.max_tool_output_chars());
        self.emit(RunEvent::ToolResult {
            id: id.clone(),
            name,
            output: output.clone(),
            duration_ms: exec_ms,
        })
        .await;
        ContentBlock::ToolResult {
            tool_use_id: id,
            content: output.content,
            is_error: output.is_error,
        }
    }

    /// Journal a tool call a fork took from its recording, exactly as the
    /// original journaled it.
    async fn journal_replayed_tool(&self, name: &str, rec: &RecordedTool) {
        let seq = self.next_journal();
        let r = self
            .store()
            .start_step(
                &self.run_id,
                seq,
                StepKind::Tool,
                Some(name),
                Some(&rec.input),
            )
            .await;
        self.soft(r);
        let status = if rec.result.is_error {
            StepStatus::Failed
        } else {
            StepStatus::Completed
        };
        let r = self
            .store()
            .finish_step(&self.run_id, seq, status, Some(&rec.output), None, None)
            .await;
        self.soft(r);
    }

    #[allow(clippy::too_many_arguments)]
    async fn tool_output(
        &self,
        sem: &Semaphore,
        tool: Option<&SharedTool>,
        policy: ToolPolicy,
        id: &str,
        name: &str,
        input: Value,
        exec_ms: &mut u64,
    ) -> ToolOutput {
        let Some(tool) = tool else {
            let available: Vec<&str> = self.tools.keys().map(String::as_str).collect();
            return ToolOutput::error(format!(
                "Unknown tool {name:?}. Available tools: {}",
                available.join(", ")
            ));
        };
        if let Value::String(raw) = &input {
            return ToolOutput::error(format!(
                "The tool input was not valid JSON ({raw}). Call the tool again with a JSON object."
            ));
        }
        if !input.is_object() {
            return ToolOutput::error("The tool input must be a JSON object.");
        }
        let hook_ctx = self.hook_ctx();
        let mut call = ToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            input,
        };
        for h in &self.hooks {
            if let Err(Blocked(why)) = h.before_tool(&hook_ctx, &mut call).await {
                return ToolOutput::error(format!("Blocked by the {} hook: {why}", h.name()));
            }
        }
        match policy {
            ToolPolicy::Deny => {
                return ToolOutput::error(format!("Calling {name} is not allowed for this agent."));
            }
            ToolPolicy::Approve if !self.auto_approve => {
                let recorded = self.replaying().and_then(|r| r.approvals.get(id)).cloned();
                let decision = match recorded {
                    Some(d) => d,
                    None => self.ask_approval(id, name, &call.input).await,
                };
                if !decision.approved {
                    let why = decision
                        .reason
                        .map(|r| format!(": {r}"))
                        .unwrap_or_default();
                    return ToolOutput::error(format!("The user declined this {name} call{why}"));
                }
                if let Some(edited) = decision.input {
                    call.input = edited;
                }
            }
            _ => {}
        }
        let _permit = sem.acquire().await.expect("semaphore open");
        let seq = self.next_journal();
        let journal_input = json!({"call_id": id, "input": call.input});
        let r = self
            .store()
            .start_step(
                &self.run_id,
                seq,
                StepKind::Tool,
                Some(name),
                Some(&journal_input),
            )
            .await;
        self.soft(r);
        let started = Instant::now();
        let mut ctx = self.ctx.clone();
        ctx.call_id = id.to_owned();
        let mut out = tokio::select! {
            r = tool.call(&ctx, call.input.clone()) => r.unwrap_or_else(|e| e.into_output()),
            _ = self.ctx.cancel.cancelled() => ToolOutput::error("cancelled"),
        };
        *exec_ms = started.elapsed().as_millis() as u64;
        for h in &self.hooks {
            if let Err(Blocked(why)) = h.after_tool(&hook_ctx, &call, &mut out).await {
                out = ToolOutput::error(format!("Output withheld by the {} hook: {why}", h.name()));
            }
        }
        let status = if out.is_error {
            StepStatus::Failed
        } else {
            StepStatus::Completed
        };
        let r = self
            .store()
            .finish_step(
                &self.run_id,
                seq,
                status,
                Some(&serde_json::to_value(&out).expect("serializable")),
                None,
                None,
            )
            .await;
        self.soft(r);
        out
    }

    async fn ask_approval(&self, id: &str, name: &str, input: &Value) -> Decision {
        if self.waiting.fetch_add(1, Ordering::SeqCst) == 0 {
            self.set_status(RunStatus::AwaitingApproval).await;
        }
        let seq = self.next_journal();
        let r = self
            .store()
            .start_step(
                &self.run_id,
                seq,
                StepKind::Approval,
                Some(name),
                Some(&json!({"call_id": id, "input": input})),
            )
            .await;
        self.soft(r);
        let rx = self.rt.inner.approvals.wait(PendingApproval {
            run_id: self.run_id.clone(),
            call_id: id.to_owned(),
            tool: name.to_owned(),
            input: input.clone(),
        });
        self.emit(RunEvent::ApprovalRequired {
            id: id.to_owned(),
            name: name.to_owned(),
            input: input.clone(),
        })
        .await;
        let decision = rx
            .await
            .unwrap_or_else(|_| Decision::reject("approval was abandoned"));
        let r = self
            .store()
            .finish_step(
                &self.run_id,
                seq,
                StepStatus::Completed,
                Some(&serde_json::to_value(&decision).expect("serializable")),
                None,
                None,
            )
            .await;
        self.soft(r);
        self.emit(RunEvent::ApprovalResolved {
            id: id.to_owned(),
            approved: decision.approved,
            reason: decision.reason.clone(),
        })
        .await;
        if self.waiting.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.set_status(RunStatus::Running).await;
        }
        decision
    }
}

/// Models sometimes wrap JSON in a Markdown code fence.
fn strip_fences(s: &str) -> &str {
    let t = s.trim();
    t.strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .and_then(|r| r.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(t)
}
