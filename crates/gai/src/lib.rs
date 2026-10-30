//! gai: an agentic AI platform.
//!
//! This crate is the entry point. It loads `gai.toml` and hands out the
//! subsystems; the individual crates are re-exported for direct use.
//!
//! ```no_run
//! # async fn demo() -> Result<(), gai::Error> {
//! use gai::agent::AgentSpec;
//!
//! /// Add two numbers.
//! #[gai::tool]
//! async fn add(a: f64, b: f64) -> f64 {
//!     a + b
//! }
//!
//! let gai = gai::Gai::builder().default_model("mock/echo").tool(add_tool()).build()?;
//!
//! // An agent from gai.toml's agents_dirs or the store, by name...
//! let reply = gai.agent("researcher").run("What changed in Rust 1.91?").await?;
//! println!("{}", reply.text);
//!
//! // ...or one written in code, streaming its events.
//! let spec = AgentSpec::new("helper").system("Be brief.").tools(["add", "calculator"]);
//! let mut run = gai.agent_spec(spec).stream("What is 17% of 2,340?").await?;
//! while let Some(event) = run.next_event().await {
//!     println!("{:?}", event.event);
//! }
//! # Ok(()) }
//! ```
//!
//! [`client::Client`] talks to a running `gai serve` over HTTP instead.

pub mod bundle;
pub mod client;
mod config;
#[cfg(feature = "sandbox")]
pub mod deployments;
pub mod finetune;
pub mod notifications;
pub mod suggestions;

use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use config::{
    A2aServeSettings, AuthMode, Config, ConfigError, DeploySettings, McpServeSettings,
    OidcSettings, ServerSettings, SsoSettings, TrustedHeader,
};
pub use gai_agent as agent;
pub use gai_browser as browser;
pub use gai_core as core;
pub use gai_data as data;
pub use gai_eval as eval;
pub use gai_llm as llm;
/// Write a tool as an async function; see [`gai_macros::tool`].
pub use gai_macros::tool;
pub use gai_media as media;
pub use gai_memory as memory;
pub use gai_orchestra as orchestra;
pub use gai_sandbox as sandbox;
pub use gai_skills as skills;
pub use gai_store as store;
pub use gai_tools as tools;
pub use gai_triggers as triggers;

/// For code the macros generate. Not part of the API.
#[doc(hidden)]
pub mod __private {
    pub use schemars;
    pub use serde;
}

use gai_agent::{AgentSpec, RunHandle, RunOutcome, RunRequest, Runtime, RuntimeConfig};
use gai_llm::{LlmError, ModelRegistry, SharedChatModel};
use gai_store::{SharedStore, SqliteStore, StoreError};
use gai_tools::{SharedTool, ToolRegistry};
use tokio::sync::OnceCell;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Llm(#[from] LlmError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Agent(#[from] gai_agent::AgentError),
    #[error(
        "no model selected: pass one (e.g. anthropic/claude-opus-5-5), set default_model in gai.toml, or export ANTHROPIC_API_KEY. `mock/echo` works without a key"
    )]
    NoModel,
    #[error("{0}")]
    NotFound(String),
    #[error(transparent)]
    Memory(#[from] gai_memory::MemoryError),
    #[error(transparent)]
    Data(#[from] gai_data::DataError),
    #[error(transparent)]
    Skill(#[from] gai_skills::SkillError),
    #[error(transparent)]
    Workflow(#[from] gai_orchestra::WorkflowError),
    #[error(transparent)]
    Eval(#[from] gai_eval::EvalError),
    /// A setting that doesn't make sense.
    #[error("{0}")]
    Invalid(String),
    /// A `.gaibundle` that can't be written or read.
    #[error("bundle: {0}")]
    Bundle(String),
}

/// Setting names in the store.
const SETTING_DEFAULT_MODEL: &str = "default_model";
const SETTING_PROVIDER_KEY: &str = "provider_key:";
const SETTING_MODEL: &str = "model:";
const SETTING_DEFAULT_PERSONA: &str = "default_persona";
pub const SETTING_DEFAULT_LANGUAGE: &str = "default_language";

/// The general assistant used when no agent is named.
pub fn default_agent() -> AgentSpec {
    AgentSpec::new("assistant")
        .system(
            "You are a capable, careful assistant. Use the tools when they help: files you read or write live in the workspace {{ workspace }}. Think before acting, check your work, and be concise in your final answer. Today is {{ date }}.",
        )
        .tools([
            "calculator",
            "current_time",
            "fs_*",
            "http_fetch",
            "shell",
            "rag_search",
        ])
}

/// Send a finished report run (one with `metadata.deliver`, set by its
/// trigger): its answer, and the files it and its sub-runs made.
async fn deliver_report(
    notifier: &gai_tools::notify::Notifier,
    store: &SharedStore,
    o: &gai_agent::RunOutcome,
) {
    let Ok(Some(run)) = store.get_run(&o.run_id).await else {
        return;
    };
    let Some(to) = run
        .metadata
        .get("deliver")
        .and_then(|d| serde_json::from_value::<gai_tools::notify::Delivery>(d.clone()).ok())
        .filter(|d| !d.is_empty())
    else {
        return;
    };
    let name = run.metadata["trigger"]
        .as_str()
        .unwrap_or("Report")
        .to_owned();
    let mut runs = vec![run.id.clone()];
    if let Ok(children) = store
        .list_runs(gai_store::RunFilter {
            parent_run_id: Some(run.id.clone()),
            limit: Some(100),
            ..Default::default()
        })
        .await
    {
        runs.extend(children.into_iter().map(|r| r.id));
    }
    let mut attachments = Vec::new();
    let mut size = 0usize;
    for id in &runs {
        for b in store.list_blobs(Some(id), 20).await.unwrap_or_default() {
            let Ok(Some(data)) = store.blob_data(&b.id).await else {
                continue;
            };
            // Mail servers refuse big messages; links are better past this.
            if size + data.len() > 15 << 20 {
                continue;
            }
            size += data.len();
            let name = b.metadata["filename"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    format!(
                        "{}.{}",
                        b.id,
                        b.media_type.rsplit('/').next().unwrap_or("bin")
                    )
                });
            attachments.push(gai_tools::notify::Attachment {
                name,
                media_type: b.media_type,
                data,
            });
        }
    }
    let ok = o.status == gai_store::RunStatus::Completed;
    let notice = gai_tools::notify::Notice {
        subject: format!("{name}: {}", chrono::Utc::now().format("%Y-%m-%d")),
        text: if ok {
            o.text.clone()
        } else {
            format!(
                "The report didn't finish: {}",
                o.error.as_deref().unwrap_or("stopped")
            )
        },
        attachments,
        meta: serde_json::json!({"kind": "report", "trigger": name, "run_id": run.id, "status": o.status}),
    };
    match notifier.send(&to, &notice).await {
        Ok(sent) => tracing::info!(run = %run.id, ?sent, "report delivered"),
        Err(e) => tracing::warn!(run = %run.id, error = %e, "could not deliver the report"),
    }
}

/// An organization slug as a SQL identifier part (`-` becomes `_`).
fn org_ident(slug: &str) -> String {
    slug.replace('-', "_")
}

/// A loaded gai instance. The store and runtime open on first use, so
/// commands that only talk to models never touch the database.
pub struct Gai {
    config: Config,
    config_path: Option<PathBuf>,
    models: Arc<ModelRegistry>,
    store_override: Option<SharedStore>,
    /// Tools added in code, registered when the runtime opens.
    extra_tools: Vec<SharedTool>,
    runtime: OnceCell<Runtime>,
    workflows: OnceCell<gai_orchestra::Workflows>,
    triggers: OnceCell<gai_triggers::Triggers>,
    /// Database connections, made when the runtime opens.
    data: std::sync::OnceLock<gai_data::Data>,
    dashboards: std::sync::OnceLock<gai_data::dashboards::Dashboards>,
    notifications: std::sync::OnceLock<notifications::Notifications>,
    suggestions: std::sync::OnceLock<suggestions::Suggestions>,
    fine_tunes: std::sync::OnceLock<finetune::FineTunes>,
    #[cfg(feature = "sandbox")]
    deployments: std::sync::OnceLock<deployments::Deployments>,
}

impl Gai {
    /// Load config from `path`, or from the usual places when `None`
    /// (see [`Config::load`]).
    pub fn load(path: Option<&Path>) -> Result<Self, Error> {
        let (config, config_path) = Config::load(path)?;
        Self::from_config(config, config_path)
    }

    pub fn from_config(config: Config, config_path: Option<PathBuf>) -> Result<Self, Error> {
        config.safety.validate().map_err(Error::Invalid)?;
        let models = Arc::new(ModelRegistry::from_config(config.llm.clone())?);
        Ok(Self {
            config,
            config_path,
            models,
            store_override: None,
            extra_tools: Vec::new(),
            runtime: OnceCell::new(),
            workflows: OnceCell::new(),
            triggers: OnceCell::new(),
            data: std::sync::OnceLock::new(),
            dashboards: std::sync::OnceLock::new(),
            notifications: std::sync::OnceLock::new(),
            suggestions: std::sync::OnceLock::new(),
            fine_tunes: std::sync::OnceLock::new(),
            #[cfg(feature = "sandbox")]
            deployments: std::sync::OnceLock::new(),
        })
    }

    /// Build a gai instance in code: config file, data folder, default
    /// model, store and extra tools.
    pub fn builder() -> GaiBuilder {
        GaiBuilder::default()
    }

    /// The instance an organization (other than the default one) works in:
    /// its own database (a SQLite file under `<data_dir>/orgs/<slug>`, or a
    /// Postgres schema `org_<slug>`), files, workspaces and vector index.
    /// It shares the server's models and settings from gai.toml, but not
    /// its MCP servers or A2A agents, and it is confined: no `shell` tool,
    /// no private network addresses, MCP over HTTP only, file triggers and
    /// folder sources only under its own `files` folder, and source tokens
    /// from its own secrets instead of the server's environment.
    pub fn for_org(&self, slug: &str) -> Result<Self, Error> {
        if slug.is_empty()
            || !slug
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(Error::Invalid(format!("bad organization slug {slug:?}")));
        }
        let mut c = self.config.clone();
        let dir = self.config.data_dir().join("orgs").join(slug);
        c.data_dir = Some(dir.clone());
        if c.postgres_url().is_none() {
            c.database = None;
        }
        c.org = Some(slug.to_owned());
        c.agents_dirs = vec![dir.join("agents")];
        c.skills_dirs = vec![dir.join("skills")];
        c.workflows_dirs = vec![dir.join("workflows")];
        c.mcp_servers.clear();
        c.a2a_agents.clear();
        c.tools.allow_private_network = false;
        c.tools.confined = true;
        c.browser.allow_private_network = false;
        let prefix = c
            .knowledge
            .vector_index
            .prefix
            .clone()
            .unwrap_or_else(|| "gai_".into());
        c.knowledge.vector_index.prefix = Some(format!("{prefix}{}_", org_ident(slug)));
        let mut g = Self::from_config(c, self.config_path.clone())?;
        g.extra_tools = self.extra_tools.clone();
        Ok(g)
    }

    /// The general assistant for this instance: [`default_agent`], without
    /// the tools a confined organization doesn't have.
    pub fn default_agent(&self) -> AgentSpec {
        let mut spec = default_agent();
        if self.config.tools.confined {
            spec.tools.retain(|t| t != "shell");
        }
        spec
    }

    /// The organization this instance belongs to; `None` for the default one.
    pub fn org(&self) -> Option<&str> {
        self.config.org.as_deref()
    }

    /// Use this store instead of the configured database.
    pub fn with_store(mut self, store: SharedStore) -> Self {
        self.store_override = Some(store);
        self
    }

    /// Offer this tool to agents (they list it in `tools` by name). Takes
    /// effect if the runtime has not opened yet.
    pub fn with_tool(mut self, tool: SharedTool) -> Self {
        self.extra_tools.push(tool);
        self
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The file the config came from, if any.
    pub fn config_path(&self) -> Option<&Path> {
        self.config_path.as_deref()
    }

    pub fn models(&self) -> &Arc<ModelRegistry> {
        &self.models
    }

    /// The model to use when none is named: the one chosen in the dashboard
    /// (once the runtime is open), `default_model` from config, or the first
    /// provider with a key.
    pub fn default_model(&self) -> Option<String> {
        self.default_model_with_source().map(|(m, _)| m)
    }

    /// The default model and where it came from: `settings` (chosen at
    /// runtime), `config` (gai.toml or `GAI_DEFAULT_MODEL`), or `detected`
    /// (the first provider with a key).
    pub fn default_model_with_source(&self) -> Option<(String, &'static str)> {
        if let Some(m) = self.runtime.get().and_then(|rt| rt.stored_default_model()) {
            return Some((m, "settings"));
        }
        if let Some(m) = &self.config.default_model {
            return Some((m.clone(), "config"));
        }
        self.models.default_model().map(|m| (m, "detected"))
    }

    /// Choose the default model, kept in the database across restarts.
    /// `None` goes back to the configured one.
    pub async fn set_default_model(&self, model: Option<String>) -> Result<(), Error> {
        let rt = self.runtime().await?;
        match &model {
            Some(m) => {
                self.models.resolve(m)?;
                rt.store().put_setting(SETTING_DEFAULT_MODEL, m).await?;
            }
            None => {
                rt.store().delete_setting(SETTING_DEFAULT_MODEL).await?;
            }
        }
        rt.set_default_model(model);
        Ok(())
    }

    /// The language agents answer in by default (`None`: gai.toml's, else
    /// the user's own). Kept in the database.
    pub async fn set_default_language(&self, language: Option<String>) -> Result<(), Error> {
        let rt = self.runtime().await?;
        let language = language
            .map(|l| l.trim().to_owned())
            .filter(|l| !l.is_empty());
        match &language {
            Some(l) => rt.store().put_setting(SETTING_DEFAULT_LANGUAGE, l).await?,
            None => {
                rt.store().delete_setting(SETTING_DEFAULT_LANGUAGE).await?;
            }
        }
        rt.set_default_language(language.or_else(|| self.config.language.clone()));
        Ok(())
    }

    /// The persona defaults and where they came from: `settings` (chosen at
    /// runtime) or `config` (gai.toml).
    pub async fn persona_defaults(
        &self,
    ) -> Result<(gai_agent::PersonaDefaults, &'static str), Error> {
        let rt = self.runtime().await?;
        let source = if rt
            .store()
            .get_setting(SETTING_DEFAULT_PERSONA)
            .await?
            .is_some()
        {
            "settings"
        } else {
            "config"
        };
        Ok((rt.default_persona(), source))
    }

    /// Choose the persona defaults, kept in the database across restarts.
    /// `None` goes back to the configured ones. A named persona must exist.
    pub async fn set_persona_defaults(
        &self,
        defaults: Option<gai_agent::PersonaDefaults>,
    ) -> Result<(), Error> {
        let rt = self.runtime().await?;
        match &defaults {
            Some(d) => {
                if let Some(p) = d.persona.as_deref().filter(|p| *p != gai_agent::NO_PERSONA) {
                    rt.resolve_persona(p).await?;
                }
                let value = serde_json::to_string(d).expect("serializable defaults");
                rt.store()
                    .put_setting(SETTING_DEFAULT_PERSONA, &value)
                    .await?;
            }
            None => {
                rt.store().delete_setting(SETTING_DEFAULT_PERSONA).await?;
            }
        }
        rt.set_default_persona(defaults);
        Ok(())
    }

    /// Add a model to a provider's catalog (say, one just pulled into
    /// Ollama), kept in the database across restarts.
    pub async fn add_model(
        &self,
        provider: &str,
        model: gai_llm::ModelConfig,
    ) -> Result<(), Error> {
        let rt = self.runtime().await?;
        self.models.add_model(provider, model.clone())?;
        let value = serde_json::to_string(&model).expect("model config serializes");
        rt.store()
            .put_setting(&format!("{SETTING_MODEL}{provider}/{}", model.id), &value)
            .await?;
        Ok(())
    }

    /// Save an API key for a provider (kept in the database, used instead
    /// of the configured key). `None` removes it.
    pub async fn set_provider_key(&self, provider: &str, key: Option<String>) -> Result<(), Error> {
        let rt = self.runtime().await?;
        let name = format!("{SETTING_PROVIDER_KEY}{provider}");
        let key = key.map(|k| k.trim().to_owned()).filter(|k| !k.is_empty());
        self.models
            .set_key(provider, key.clone().map(secrecy::SecretString::from))?;
        match key {
            Some(k) => rt.store().put_setting(&name, &k).await?,
            None => {
                rt.store().delete_setting(&name).await?;
            }
        }
        Ok(())
    }

    /// A chat model by address, or the default when `spec` is `None`.
    pub fn chat_model(&self, spec: Option<&str>) -> Result<SharedChatModel, Error> {
        let spec = match spec {
            Some(s) => s.to_owned(),
            None => self.default_model().ok_or(Error::NoModel)?,
        };
        Ok(self.models.chat(&spec)?)
    }

    /// The agent runtime, opening the database on first call.
    pub async fn runtime(&self) -> Result<&Runtime, Error> {
        self.runtime
            .get_or_try_init(|| async {
                let store: SharedStore = match &self.store_override {
                    Some(s) => s.clone(),
                    None => match self.config.postgres_url() {
                        #[cfg(feature = "postgres")]
                        Some(url) => match &self.config.org {
                            Some(org) => Arc::new(
                                gai_store::PostgresStore::connect_schema(&url, &format!("org_{}", org_ident(org)))
                                    .await?,
                            ),
                            None => Arc::new(gai_store::PostgresStore::connect(&url).await?),
                        },
                        #[cfg(not(feature = "postgres"))]
                        Some(_) => {
                            return Err(Error::Invalid(
                                "database is a Postgres URL, but this gai was built without the postgres feature".into(),
                            ));
                        }
                        None => Arc::new(SqliteStore::open(self.config.database()).await?),
                    },
                };
                let config = RuntimeConfig {
                    default_model: self.default_model(),
                    data_dir: self.config.data_dir(),
                    tools: self.config.tools.clone(),
                    knowledge: {
                        let mut k = self.config.knowledge.clone();
                        // Graphs are read with the default model unless one is set.
                        if k.graph.model.is_none() {
                            k.graph.model = self.default_model();
                        }
                        k
                    },
                    skills_dirs: self.config.skills_dirs(),
                    agents_dirs: self.config.agents_dirs(),
                    record_requests: self.config.record_requests.unwrap_or(true),
                    media: self.config.media.clone(),
                    persona: self.config.persona.clone(),
                    safety: self.config.safety.clone(),
                };
                for (name, key) in store.list_settings(SETTING_PROVIDER_KEY).await? {
                    let provider = &name[SETTING_PROVIDER_KEY.len()..];
                    if let Err(e) = self
                        .models
                        .set_key(provider, Some(secrecy::SecretString::from(key)))
                    {
                        tracing::warn!(provider, error = %e, "ignoring a stored API key");
                    }
                }
                for (name, value) in store.list_settings(SETTING_MODEL).await? {
                    let Some((provider, _)) = name[SETTING_MODEL.len()..].split_once('/') else {
                        continue;
                    };
                    match serde_json::from_str::<gai_llm::ModelConfig>(&value) {
                        Ok(m) => {
                            if let Err(e) = self.models.add_model(provider, m) {
                                tracing::warn!(setting = %name, error = %e, "ignoring a stored model");
                            }
                        }
                        Err(e) => tracing::warn!(setting = %name, error = %e, "ignoring a stored model"),
                    }
                }
                let mut tools = ToolRegistry::with_builtins();
                if self.config.tools.confined {
                    // It runs anything as the server's user.
                    tools.unregister_prefix("shell");
                }
                let rt = Runtime::new(
                    self.models.clone(),
                    tools,
                    store.clone(),
                    config,
                );
                if self.config.knowledge.vector_index.backend == gai_memory::VectorBackend::Pgvector {
                    let url = self
                        .config
                        .knowledge
                        .vector_index
                        .url
                        .clone()
                        .or_else(|| self.config.postgres_url())
                        .ok_or_else(|| {
                            Error::Invalid(
                                "the pgvector index needs a Postgres URL: [knowledge.vector_index] url, or database = \"postgres://...\"".into(),
                            )
                        })?;
                    #[cfg(feature = "postgres")]
                    {
                        let table = match &self.config.org {
                            Some(org) => format!("gai_vectors_{}", org_ident(org)),
                            None => "gai_vectors".into(),
                        };
                        rt.knowledge().set_index(Some(Arc::new(
                            gai_store::PgVectorIndex::connect_table(&url, &table).await?,
                        )));
                    }
                    #[cfg(not(feature = "postgres"))]
                    return Err(Error::Invalid(format!(
                        "the pgvector index ({url}) needs gai built with the postgres feature"
                    )));
                }
                // Model servers gai started, so agents naming their models
                // resolve before the servers are checked.
                #[cfg(feature = "sandbox")]
                if let Some(d) = self.deployments_for(&store) {
                    d.restore().await?;
                }
                self.fine_tunes.get_or_init(|| {
                    let f = finetune::FineTunes::new(store.clone(), self.models.clone());
                    #[cfg(feature = "sandbox")]
                    let f = f.with_local(self.deployments.get().cloned());
                    f
                });
                if let Some(m) = store.get_setting(SETTING_DEFAULT_MODEL).await? {
                    rt.set_default_model(Some(m));
                }
                rt.set_default_language(
                    store
                        .get_setting(SETTING_DEFAULT_LANGUAGE)
                        .await?
                        .or_else(|| self.config.language.clone()),
                );
                if let Some(p) = store.get_setting(SETTING_DEFAULT_PERSONA).await? {
                    match serde_json::from_str(&p) {
                        Ok(d) => rt.set_default_persona(Some(d)),
                        Err(e) => tracing::warn!(error = %e, "ignoring the stored persona defaults"),
                    }
                }
                let data = self.data.get_or_init(|| {
                    if self.config.tools.confined {
                        gai_data::Data::confined(store.clone(), self.config.data_dir().join("files"))
                    } else {
                        gai_data::Data::new(store.clone())
                    }
                });
                for tool in gai_data::tools::tools(data.clone()) {
                    rt.register_tool(tool);
                }
                let dashboards = self.dashboards.get_or_init(|| {
                    gai_data::dashboards::Dashboards::new(data.clone(), Some(self.notifier()))
                });
                rt.register_tool(dashboards.tool());
                let notifications = self.notifications.get_or_init(|| {
                    notifications::Notifications::new(store.clone(), self.notifier())
                });
                notifications.attach(&rt);
                dashboards.set_alert_hook(Some(notifications.alert_hook()));
                let suggestions = self.suggestions.get_or_init(|| {
                    suggestions::Suggestions::new(store.clone(), notifications.clone())
                });
                rt.register_tool(suggestions.tool());
                // Scheduled reports: a trigger's run sends what it made.
                let (notifier, reports_store) = (self.notifier(), store.clone());
                rt.set_finish_hook(
                    "deliver-report",
                    Arc::new(move |o: gai_agent::RunOutcome| {
                        let (notifier, store) = (notifier.clone(), reports_store.clone());
                        Box::pin(async move { deliver_report(&notifier, &store, &o).await })
                    }),
                );
                match gai_sandbox::from_settings(&self.config.sandbox) {
                    Ok(Some(sb)) => rt.register_tool(Arc::new(gai_sandbox::code_exec(
                        sb,
                        self.config.sandbox.clone(),
                        Some(Arc::new(BlobArtifacts(store.clone(), "code_exec"))),
                    ))),
                    Ok(None) => {}
                    Err(e) => tracing::warn!(error = %e, "no code sandbox"),
                }
                #[cfg(feature = "browser")]
                if let Some(chrome) = gai_browser::chrome::Chrome::from_settings(&self.config.browser) {
                    let images: Arc<dyn gai_browser::ImageSink> =
                        Arc::new(BlobArtifacts(store.clone(), "browser"));
                    for tool in gai_browser::tools(chrome, Some(images)) {
                        rt.register_tool(tool);
                    }
                }
                for tool in &self.extra_tools {
                    rt.register_tool(tool.clone());
                }
                for (name, cfg) in &self.config.a2a_agents {
                    let agent = gai_tools::a2a::A2aAgent::new(name, cfg.clone(), rt.http().clone());
                    rt.register_tool(gai_tools::a2a::a2a_tool(agent, &format!("a2a__{name}")));
                }
                let h = &self.config.hooks;
                if h.redact_secrets {
                    let hook = gai_agent::hooks::RedactSecrets::new(&h.secret_patterns)
                        .map_err(|e| Error::NotFound(format!("hooks.secret_patterns: {e}")))?;
                    rt.add_hook(Arc::new(hook));
                }
                if !h.deny_shell.is_empty() {
                    let hook = gai_agent::hooks::DenyShell::new(&h.deny_shell)
                        .map_err(|e| Error::NotFound(format!("hooks.deny_shell: {e}")))?;
                    rt.add_hook(Arc::new(hook));
                }
                rt.mcp().set_credentials(Arc::new(StoredMcpCredentials(store.clone())));
                // MCP servers connect on first use, or when `gai serve` starts.
                for (name, cfg) in &self.config.mcp_servers {
                    rt.register_mcp(gai_tools::mcp::McpServer::new(name, cfg.clone(), "config"));
                }
                for (name, cfg) in store.list_mcp_servers().await? {
                    if self.config.mcp_servers.contains_key(&name) {
                        continue; // gai.toml wins over a stored server of the same name
                    }
                    match serde_json::from_value::<gai_tools::mcp::McpServerConfig>(cfg) {
                        Ok(cfg) => rt.register_mcp(gai_tools::mcp::McpServer::new(&name, cfg, "api")),
                        Err(e) => tracing::warn!(server = %name, error = %e, "stored MCP server has an invalid config"),
                    }
                }
                Ok::<_, Error>(rt)
            })
            .await
    }

    /// Find an agent by reference:
    ///
    /// - a path to an agent file (`agents/researcher.yaml`);
    /// - a stored agent: `name`, `name@v3`, or `name@<label>`;
    /// - a file named `<name>.yaml|.yml|.toml|.json` in an agents directory.
    ///
    /// Returns the spec and, for stored agents, its version.
    pub async fn find_agent(&self, reference: &str) -> Result<(AgentSpec, Option<u32>), Error> {
        match self.runtime().await?.resolve_agent(reference).await {
            Ok(found) => Ok(found),
            Err(gai_agent::AgentError::NotFound(m)) => Err(Error::NotFound(m)),
            Err(e) => Err(e.into()),
        }
    }

    /// An agent to run, by reference (see [`find_agent`](Self::find_agent)
    /// for the forms). Nothing is looked up until it runs.
    pub fn agent(&self, reference: impl Into<String>) -> Agent<'_> {
        Agent::new(self, AgentTarget::Ref(reference.into()))
    }

    /// An agent written in code.
    pub fn agent_spec(&self, spec: AgentSpec) -> Agent<'_> {
        Agent::new(self, AgentTarget::Spec(Box::new(spec)))
    }

    /// Evaluations: eval sets, running them against agents and models, and
    /// their results.
    pub async fn evals(&self) -> Result<gai_eval::Evaluator, Error> {
        Ok(gai_eval::Evaluator::new(self.runtime().await?.clone()))
    }

    /// Email and webhooks, from `[notify]`.
    pub fn notifier(&self) -> gai_tools::notify::Notifier {
        gai_tools::notify::Notifier::new(
            self.config.notify.clone(),
            self.config.tools.allow_private_network && !self.config.tools.confined,
        )
    }

    /// Dashboards over the database connections.
    pub async fn dashboards(&self) -> Result<&gai_data::dashboards::Dashboards, Error> {
        self.runtime().await?;
        Ok(self.dashboards.get().expect("made with the runtime"))
    }

    /// Notification rules and web push.
    pub async fn notifications(&self) -> Result<&notifications::Notifications, Error> {
        self.runtime().await?;
        Ok(self.notifications.get().expect("made with the runtime"))
    }

    /// Model servers this gai runs in containers. Only the default
    /// organization has them; `None` for others.
    #[cfg(feature = "sandbox")]
    pub async fn deployments(&self) -> Result<Option<&deployments::Deployments>, Error> {
        self.runtime().await?;
        Ok(self.deployments.get())
    }

    #[cfg(feature = "sandbox")]
    fn deployments_for(&self, store: &SharedStore) -> Option<&deployments::Deployments> {
        if self.config.org.is_some() || self.config.tools.confined {
            return None;
        }
        Some(self.deployments.get_or_init(|| {
            let engine = self.config.deploy.engine.as_deref().or(self
                .config
                .sandbox
                .engine
                .as_deref());
            deployments::Deployments::new(
                store.clone(),
                self.models.clone(),
                Arc::new(gai_sandbox::deploy::DockerEngine::new(engine)),
                self.config.deploy.clone(),
            )
        }))
    }

    /// Fine-tuning jobs on rated runs.
    pub async fn fine_tunes(&self) -> Result<&finetune::FineTunes, Error> {
        self.runtime().await?;
        Ok(self.fine_tunes.get().expect("made with the runtime"))
    }

    /// Check running fine-tunes; models that finished are added to the
    /// catalog (and kept across restarts).
    pub async fn poll_fine_tunes(&self) -> Result<Vec<finetune::FineTuneJob>, Error> {
        let f = self.fine_tunes().await?;
        f.poll(&|provider: &str, model: gai_llm::ModelConfig| {
            let provider = provider.to_owned();
            Box::pin(async move { self.add_model(&provider, model).await })
        })
        .await
    }

    /// Suggestions from usage and from agents' `suggest` tool.
    pub async fn suggestions(&self) -> Result<&suggestions::Suggestions, Error> {
        self.runtime().await?;
        Ok(self.suggestions.get().expect("made with the runtime"))
    }

    /// Meeting recordings, calls and videos: transcribed with speakers,
    /// with notes or a summary, using the `[media]` models and the
    /// default model to write.
    pub async fn recordings(&self) -> Result<Arc<gai_media::Recordings>, Error> {
        let rt = self.runtime().await?;
        let m = &rt.config().media;
        Ok(Arc::new(gai_media::Recordings::new(
            rt.store().clone(),
            rt.models().clone(),
            gai_media::MediaModels {
                transcription: m.transcription.clone(),
                chat: rt.default_model(),
                vision: m.vision.clone(),
            },
        )))
    }

    /// Database connections and querying them.
    pub async fn data(&self) -> Result<&gai_data::Data, Error> {
        self.runtime().await?;
        Ok(self.data.get().expect("made with the runtime"))
    }

    /// Sources synced into knowledge collections (folders, websites,
    /// GitHub, Confluence, Notion).
    pub async fn sources(&self) -> Result<gai_memory::sources::Sources, Error> {
        let sources = gai_memory::sources::Sources::new(self.runtime().await?.knowledge().clone());
        Ok(if self.config.tools.confined {
            sources.confined(self.config.data_dir().join("files"))
        } else {
            sources
        })
    }

    /// Apply retention: delete conversations older than each agent's
    /// `retention_days` (from `[safety]`, overridden by the agent's own
    /// `safety:`). Returns what was deleted, per agent.
    pub async fn sweep_retention(&self) -> Result<Vec<(String, gai_store::Deleted)>, Error> {
        let rt = self.runtime().await?;
        let mut out = Vec::new();
        for agent in rt.store().session_agents().await? {
            let own = rt
                .resolve_agent(&agent)
                .await
                .ok()
                .and_then(|(s, _)| s.safety);
            let Some(days) = self.config.safety.merged(own.as_ref()).retention_days else {
                continue;
            };
            let before = chrono::Utc::now() - chrono::Duration::days(i64::from(days));
            let deleted = rt.store().prune_sessions(&agent, before).await?;
            if deleted.sessions > 0 {
                tracing::info!(
                    agent,
                    days,
                    sessions = deleted.sessions,
                    "retention: deleted old conversations"
                );
                out.push((agent, deleted));
            }
        }
        Ok(out)
    }

    /// Delete everything kept about a user: conversations, runs and their
    /// files, memory blocks, archival memory and ratings.
    pub async fn forget_user(&self, user_id: &str) -> Result<gai_store::Deleted, Error> {
        let rt = self.runtime().await?;
        let deleted = rt.store().forget_user(user_id).await?;
        rt.knowledge()
            .delete_collection(&format!("memory:user:{user_id}"))
            .await?;
        Ok(deleted)
    }

    /// Workflows: stored and file workflows, and running them. Registers
    /// the workflow engine with the runtime on first use.
    pub async fn workflows(&self) -> Result<&gai_orchestra::Workflows, Error> {
        let rt = self.runtime().await?;
        Ok(self
            .workflows
            .get_or_init(|| async {
                gai_orchestra::Workflows::new(rt.clone(), self.config.workflows_dirs())
            })
            .await)
    }

    /// Triggers: schedules, webhooks and file watchers that start runs.
    pub async fn triggers(&self) -> Result<&gai_triggers::Triggers, Error> {
        let rt = self.runtime().await?;
        let wf = self.workflows().await?;
        Ok(self
            .triggers
            .get_or_init(|| async { gai_triggers::Triggers::new(rt.clone(), wf.clone()) })
            .await)
    }
}

/// Builds a [`Gai`]; see [`Gai::builder`].
#[derive(Default)]
pub struct GaiBuilder {
    config_path: Option<PathBuf>,
    config: Option<Config>,
    data_dir: Option<PathBuf>,
    default_model: Option<String>,
    store: Option<SharedStore>,
    tools: Vec<SharedTool>,
}

impl GaiBuilder {
    /// Read this config file (instead of looking for `gai.toml` in the
    /// usual places).
    pub fn config(mut self, path: impl Into<PathBuf>) -> Self {
        self.config_path = Some(path.into());
        self
    }

    /// Use this config as is, reading no file.
    pub fn with_config(mut self, config: Config) -> Self {
        self.config = Some(config);
        self
    }

    /// Where the database, workspaces and files live.
    pub fn data_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.data_dir = Some(dir.into());
        self
    }

    /// The model agents use when they name none (`provider/model`).
    pub fn default_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = Some(model.into());
        self
    }

    /// Use this store instead of the configured SQLite database.
    pub fn store(mut self, store: SharedStore) -> Self {
        self.store = Some(store);
        self
    }

    /// Offer a tool to agents, e.g. one made with [`tool`](macro@crate::tool).
    pub fn tool(mut self, tool: SharedTool) -> Self {
        self.tools.push(tool);
        self
    }

    pub fn tools(mut self, tools: impl IntoIterator<Item = SharedTool>) -> Self {
        self.tools.extend(tools);
        self
    }

    pub fn build(self) -> Result<Gai, Error> {
        let (mut config, path) = match self.config {
            Some(c) => (c, None),
            None => Config::load(self.config_path.as_deref())?,
        };
        if let Some(d) = self.data_dir {
            config.data_dir = Some(d);
        }
        if let Some(m) = self.default_model {
            config.default_model = Some(m);
        }
        let mut gai = Gai::from_config(config, path)?;
        gai.store_override = self.store;
        gai.extra_tools = self.tools;
        Ok(gai)
    }
}

enum AgentTarget {
    Ref(String),
    Spec(Box<AgentSpec>),
}

/// An agent ready to run; see [`Gai::agent`].
pub struct Agent<'a> {
    gai: &'a Gai,
    target: AgentTarget,
    session: Option<String>,
    persona: Option<String>,
    model: Option<String>,
    auto_approve: bool,
}

impl<'a> Agent<'a> {
    fn new(gai: &'a Gai, target: AgentTarget) -> Self {
        Self {
            gai,
            target,
            session: None,
            persona: None,
            model: None,
            auto_approve: false,
        }
    }

    /// Continue a conversation: runs in the same session see what came
    /// before.
    pub fn session(mut self, id: impl Into<String>) -> Self {
        self.session = Some(id.into());
        self
    }

    /// Speak as this persona (`name`, `name@v2`, or `"none"`).
    pub fn persona(mut self, persona: impl Into<String>) -> Self {
        self.persona = Some(persona.into());
        self
    }

    /// Use this model instead of the agent's.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Run tools that need approval without asking. Without it such a run
    /// waits for [`Runtime::resolve_approval`].
    pub fn auto_approve(mut self) -> Self {
        self.auto_approve = true;
        self
    }

    /// The agent's spec, as it would run.
    pub async fn spec(&self) -> Result<AgentSpec, Error> {
        Ok(self.resolve().await?.0)
    }

    async fn resolve(&self) -> Result<(AgentSpec, Option<u32>), Error> {
        let (mut spec, version) = match &self.target {
            AgentTarget::Ref(r) => self.gai.find_agent(r).await?,
            AgentTarget::Spec(s) => ((**s).clone(), None),
        };
        if let Some(m) = &self.model {
            spec.model = Some(m.clone());
        }
        Ok((spec, version))
    }

    async fn request(&self, input: Vec<gai_core::ContentBlock>) -> Result<RunRequest, Error> {
        let (spec, version) = self.resolve().await?;
        let mut req = RunRequest::with_input(spec, input);
        req.agent_version = version;
        req.session_id = self.session.clone();
        req.persona = self.persona.clone().map(gai_agent::PersonaRef::Name);
        req.auto_approve = self.auto_approve;
        Ok(req)
    }

    /// Run to the end and return the outcome.
    pub async fn run(&self, text: impl Into<String>) -> Result<RunOutcome, Error> {
        self.run_with(vec![gai_core::ContentBlock::text(text)])
            .await
    }

    /// [`run`](Self::run) with any input: text, images, audio.
    pub async fn run_with(&self, input: Vec<gai_core::ContentBlock>) -> Result<RunOutcome, Error> {
        let req = self.request(input).await?;
        Ok(self.gai.runtime().await?.run(req).await?)
    }

    /// Start the run and follow its events (text deltas, tool calls,
    /// approvals) with [`RunHandle::next_event`]; `wait` gives the outcome.
    pub async fn stream(&self, text: impl Into<String>) -> Result<RunHandle, Error> {
        let req = self
            .request(vec![gai_core::ContentBlock::text(text)])
            .await?;
        Ok(self.gai.runtime().await?.start(req).await?)
    }
}

/// Files written by `code_exec` and browser screenshots, kept as blobs of
/// the run. The second field is the blob's `source`.
struct BlobArtifacts(SharedStore, &'static str);

impl BlobArtifacts {
    async fn keep(
        &self,
        ctx: &gai_tools::ToolCtx,
        path: &str,
        media_type: &str,
        data: Vec<u8>,
    ) -> Result<String, String> {
        let name = path.rsplit('/').next().unwrap_or(path).to_owned();
        self.0
            .put_blob(gai_store::NewBlob {
                media_type: media_type.to_owned(),
                name: Some(name),
                run_id: (!ctx.run_id.is_empty()).then(|| ctx.run_id.clone()),
                source: Some(self.1.into()),
                metadata: serde_json::json!({"path": path}),
                data,
            })
            .await
            .map(|b| b.id)
            .map_err(|e| e.to_string())
    }
}

#[async_trait::async_trait]
impl gai_sandbox::ArtifactSink for BlobArtifacts {
    async fn save(
        &self,
        ctx: &gai_tools::ToolCtx,
        path: &str,
        media_type: &str,
        data: Vec<u8>,
    ) -> Result<String, String> {
        self.keep(ctx, path, media_type, data).await
    }
}

#[cfg(feature = "browser")]
#[async_trait::async_trait]
impl gai_browser::ImageSink for BlobArtifacts {
    async fn save(
        &self,
        ctx: &gai_tools::ToolCtx,
        name: &str,
        media_type: &str,
        data: Vec<u8>,
    ) -> Result<String, String> {
        self.keep(ctx, name, media_type, data).await
    }
}

/// MCP OAuth tokens kept in the gai store.
struct StoredMcpCredentials(SharedStore);

#[async_trait::async_trait]
impl gai_tools::mcp::McpCredentials for StoredMcpCredentials {
    async fn load(&self, server: &str) -> Option<serde_json::Value> {
        self.0.get_mcp_oauth(server).await.ok().flatten()
    }

    async fn save(&self, server: &str, credentials: serde_json::Value) {
        if let Err(e) = self.0.put_mcp_oauth(server, &credentials).await {
            tracing::warn!(server, error = %e, "could not save MCP sign-in");
        }
    }

    async fn clear(&self, server: &str) {
        let _ = self.0.delete_mcp_oauth(server).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolves_agents_from_files_store_and_labels() {
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join("agents");
        std::fs::create_dir(&agents).unwrap();
        std::fs::write(
            agents.join("filer.yaml"),
            "model: mock/echo\ntools: [calculator]\n",
        )
        .unwrap();
        let config = Config {
            data_dir: Some(dir.path().to_path_buf()),
            agents_dirs: vec![agents.clone()],
            ..Default::default()
        };
        let gai = Gai::from_config(config, None).unwrap();

        let (spec, v) = gai.find_agent("filer").await.unwrap();
        assert_eq!((spec.name.as_str(), v), ("filer", None));
        let (spec, _) = gai
            .find_agent(agents.join("filer.yaml").to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(spec.tools, ["calculator"]);

        let rt = gai.runtime().await.unwrap();
        rt.save_agent(&AgentSpec::new("stored").model("mock/echo"), None)
            .await
            .unwrap();
        rt.save_agent(&AgentSpec::new("stored").model("mock/other"), None)
            .await
            .unwrap();
        rt.store()
            .set_agent_label("stored", "prod", Some(1))
            .await
            .unwrap();
        assert_eq!(gai.find_agent("stored").await.unwrap().1, Some(2));
        assert_eq!(
            gai.find_agent("stored@v1")
                .await
                .unwrap()
                .0
                .model
                .as_deref(),
            Some("mock/echo")
        );
        assert_eq!(gai.find_agent("stored@prod").await.unwrap().1, Some(1));
        assert!(gai.find_agent("stored@nope").await.is_err());
        assert!(gai.find_agent("missing").await.is_err());
        assert!(dir.path().join("gai.db").exists());
    }

    /// The system prompt a run's first model call was sent.
    async fn sent_system(gai: &Gai, spec: AgentSpec) -> String {
        let out = gai.agent_spec(spec).run("hello").await.unwrap();
        let steps = gai
            .runtime()
            .await
            .unwrap()
            .store()
            .list_steps(&out.run_id)
            .await
            .unwrap();
        let model = steps
            .iter()
            .find(|s| s.kind == gai_store::StepKind::Model)
            .unwrap();
        model.input.as_ref().unwrap()["request"]["system"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    #[tokio::test]
    async fn agents_answer_in_their_language() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            data_dir: Some(dir.path().to_owned()),
            default_model: Some("mock/echo".into()),
            language: Some("de".into()),
            ..Default::default()
        };
        let gai = Gai::from_config(config, None).unwrap();
        let plain = AgentSpec::new("plain").system("Be brief.");
        let system = sent_system(&gai, plain.clone()).await;
        assert!(system.starts_with("Be brief."), "{system}");
        assert!(system.ends_with("Always reply in German, whatever language the user writes in, unless they ask you to use another one."), "{system}");

        let mut french = plain.clone();
        french.language = Some("fr".into());
        assert!(sent_system(&gai, french).await.contains("reply in French"));

        let placed = AgentSpec::new("placed").system("Write in {{ language }} only.");
        let mut placed = placed;
        placed.language = Some("pt-BR".into());
        assert_eq!(
            sent_system(&gai, placed).await,
            "Write in Brazilian Portuguese only."
        );

        gai.set_default_language(Some("ja".into())).await.unwrap();
        assert!(sent_system(&gai, plain.clone()).await.contains("Japanese"));
        // Clearing the setting goes back to gai.toml's.
        gai.set_default_language(None).await.unwrap();
        assert!(sent_system(&gai, plain).await.contains("German"));
    }
}
