//! gai: an agentic AI platform.
//!
//! This crate is the entry point. It loads `gai.toml` and hands out the
//! subsystems; the individual crates are re-exported for direct use.
//!
//! ```no_run
//! # async fn demo() -> Result<(), Box<dyn std::error::Error>> {
//! use gai::agent::{AgentSpec, RunRequest};
//!
//! let gai = gai::Gai::load(None)?;
//! let agent = AgentSpec::new("helper")
//!     .model("anthropic/claude-opus-5-5")
//!     .system("You are a careful assistant.")
//!     .tools(["calculator", "http_fetch"]);
//! let outcome = gai.runtime().await?.run(RunRequest::new(agent, "What is 17% of 2,340?")).await?;
//! println!("{}", outcome.text);
//! # Ok(()) }
//! ```

mod config;

use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use config::{
    A2aServeSettings, AuthMode, Config, ConfigError, McpServeSettings, ServerSettings,
};
pub use gai_agent as agent;
pub use gai_core as core;
pub use gai_llm as llm;
pub use gai_memory as memory;
pub use gai_orchestra as orchestra;
pub use gai_skills as skills;
pub use gai_store as store;
pub use gai_tools as tools;
pub use gai_triggers as triggers;

use gai_agent::{AgentSpec, Runtime, RuntimeConfig};
use gai_llm::{LlmError, ModelRegistry, SharedChatModel};
use gai_store::{SharedStore, SqliteStore, StoreError};
use gai_tools::ToolRegistry;
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
    Skill(#[from] gai_skills::SkillError),
    #[error(transparent)]
    Workflow(#[from] gai_orchestra::WorkflowError),
}

/// Setting names in the store.
const SETTING_DEFAULT_MODEL: &str = "default_model";
const SETTING_PROVIDER_KEY: &str = "provider_key:";
const SETTING_MODEL: &str = "model:";

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

/// A loaded gai instance. The store and runtime open on first use, so
/// commands that only talk to models never touch the database.
pub struct Gai {
    config: Config,
    config_path: Option<PathBuf>,
    models: Arc<ModelRegistry>,
    store_override: Option<SharedStore>,
    runtime: OnceCell<Runtime>,
    workflows: OnceCell<gai_orchestra::Workflows>,
    triggers: OnceCell<gai_triggers::Triggers>,
}

impl Gai {
    /// Load config from `path`, or from the usual places when `None`
    /// (see [`Config::load`]).
    pub fn load(path: Option<&Path>) -> Result<Self, Error> {
        let (config, config_path) = Config::load(path)?;
        Self::from_config(config, config_path)
    }

    pub fn from_config(config: Config, config_path: Option<PathBuf>) -> Result<Self, Error> {
        let models = Arc::new(ModelRegistry::from_config(config.llm.clone())?);
        Ok(Self {
            config,
            config_path,
            models,
            store_override: None,
            runtime: OnceCell::new(),
            workflows: OnceCell::new(),
            triggers: OnceCell::new(),
        })
    }

    /// Use this store instead of the configured database.
    pub fn with_store(mut self, store: SharedStore) -> Self {
        self.store_override = Some(store);
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
                    None => Arc::new(SqliteStore::open(self.config.database()).await?),
                };
                let config = RuntimeConfig {
                    default_model: self.default_model(),
                    data_dir: self.config.data_dir(),
                    tools: self.config.tools.clone(),
                    knowledge: self.config.knowledge.clone(),
                    skills_dirs: self.config.skills_dirs(),
                    agents_dirs: self.config.agents_dirs(),
                    record_requests: self.config.record_requests.unwrap_or(true),
                    media: self.config.media.clone(),
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
                let rt = Runtime::new(
                    self.models.clone(),
                    ToolRegistry::with_builtins(),
                    store.clone(),
                    config,
                );
                if let Some(m) = store.get_setting(SETTING_DEFAULT_MODEL).await? {
                    rt.set_default_model(Some(m));
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
    pub async fn agent(&self, reference: &str) -> Result<(AgentSpec, Option<u32>), Error> {
        match self.runtime().await?.resolve_agent(reference).await {
            Ok(found) => Ok(found),
            Err(gai_agent::AgentError::NotFound(m)) => Err(Error::NotFound(m)),
            Err(e) => Err(e.into()),
        }
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

        let (spec, v) = gai.agent("filer").await.unwrap();
        assert_eq!((spec.name.as_str(), v), ("filer", None));
        let (spec, _) = gai
            .agent(agents.join("filer.yaml").to_str().unwrap())
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
        assert_eq!(gai.agent("stored").await.unwrap().1, Some(2));
        assert_eq!(
            gai.agent("stored@v1").await.unwrap().0.model.as_deref(),
            Some("mock/echo")
        );
        assert_eq!(gai.agent("stored@prod").await.unwrap().1, Some(1));
        assert!(gai.agent("stored@nope").await.is_err());
        assert!(gai.agent("missing").await.is_err());
        assert!(dir.path().join("gai.db").exists());
    }
}
