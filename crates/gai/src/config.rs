use std::path::{Path, PathBuf};

use figment::Figment;
use figment::providers::{Env, Format, Toml};
use gai_llm::LlmConfig;
use serde::{Deserialize, Serialize};

/// Everything in `gai.toml`.
///
/// Provider and alias tables sit at the top level:
///
/// ```toml
/// default_model = "anthropic/claude-opus-5-5"
///
/// [providers.ollama]
/// base_url = "http://gpu-box:11434/v1"
///
/// [aliases]
/// fast = "groq/llama-3.3-70b-versatile"
/// ```
///
/// Any value can also come from `GAI_`-prefixed environment variables, with
/// `__` between nested keys: `GAI_DEFAULT_MODEL=mock/echo`,
/// `GAI_PROVIDERS__OLLAMA__BASE_URL=http://gpu-box:11434/v1`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub default_model: Option<String>,
    /// Where databases, workspaces and artifacts live. Defaults to the
    /// platform data directory (`~/.local/share/gai` on Linux).
    pub data_dir: Option<PathBuf>,
    /// The database: a SQLite file (default `<data_dir>/gai.db`), or a
    /// Postgres URL (`postgres://user:pass@host/db`).
    pub database: Option<PathBuf>,
    /// Directories searched for agent files (`<name>.yaml|.yml|.toml|.json`).
    pub agents_dirs: Vec<PathBuf>,
    /// Settings for the built-in tools.
    pub tools: gai_tools::ToolSettings,
    /// `gai serve` settings.
    pub server: ServerSettings,
    /// MCP servers whose tools agents can use (as `mcp__<name>__<tool>`).
    pub mcp_servers: std::collections::BTreeMap<String, gai_tools::mcp::McpServerConfig>,
    /// Guardrail hooks run around every model and tool call.
    pub hooks: HookSettings,
    /// Knowledge collections and archival memory: embedding model and
    /// chunking.
    pub knowledge: gai_memory::KnowledgeSettings,
    /// Folders of Agent Skills (each skill a subfolder with a SKILL.md).
    /// Defaults to `./skills` and `<data_dir>/skills`.
    pub skills_dirs: Vec<PathBuf>,
    /// Folders searched for workflow files (`<name>.yaml|.yml|.toml|.json`).
    /// Defaults to `./workflows` and `<data_dir>/workflows`.
    pub workflows_dirs: Vec<PathBuf>,
    /// Journal each model call's full request so runs can be replayed
    /// exactly (default true). Turn off to save space on long runs.
    pub record_requests: Option<bool>,
    /// Remote agents reached over A2A, each offered as a tool `a2a__<name>`.
    pub a2a_agents: std::collections::BTreeMap<String, gai_tools::a2a::A2aAgentConfig>,
    /// Default models for the image_gen, speak and transcribe tools and the
    /// /v1/audio and /v1/images endpoints: transcription, speech, voice,
    /// image.
    pub media: gai_agent::MediaSettings,
    /// Where `code_exec` runs code: a container per call by default.
    pub sandbox: gai_sandbox::SandboxSettings,
    /// Personal data, prompt injection, guardrails and retention for every
    /// agent (agents override fields in their own `safety:`).
    pub safety: gai_agent::safety::Safety,
    /// The browser tools: which Chrome, which sites, and limits.
    pub browser: gai_browser::BrowserSettings,
    /// The persona runs speak as when neither the run nor its agent picks
    /// one, and where it goes in the prompt.
    pub persona: gai_agent::PersonaDefaults,
    /// Email and webhooks for reports and alerts.
    pub notify: gai_tools::notify::NotifySettings,
    /// Model servers gai starts in containers (Ollama, vLLM, llama.cpp).
    pub deploy: DeploySettings,
    /// The language agents answer in unless the agent or the run says
    /// otherwise (`fr`, `pt-BR`, or a name). Unset: the user's language.
    pub language: Option<String>,
    #[serde(flatten)]
    pub llm: LlmConfig,
    /// Set on an organization's instance (see `Gai::for_org`): its slug.
    #[serde(skip)]
    pub org: Option<String>,
}

/// ```toml
/// [deploy]
/// engine = "unix:///run/podman/podman.sock"   # default: [sandbox] engine, DOCKER_HOST, usual sockets
/// bind = "127.0.0.1"                           # host address model servers are published on
/// host = "127.0.0.1"                           # how gai reaches them (host.containers.internal when gai runs in a container)
/// ```
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DeploySettings {
    pub engine: Option<String>,
    pub bind: String,
    pub host: String,
}

impl Default for DeploySettings {
    fn default() -> Self {
        Self {
            engine: None,
            bind: "127.0.0.1".into(),
            host: "127.0.0.1".into(),
        }
    }
}

/// ```toml
/// [hooks]
/// redact_secrets = true                       # mask keys and tokens in tool output
/// secret_patterns = ["ACME-[0-9]{8}"]         # extra regexes to mask
/// deny_shell = ['rm\s+-rf\s+/', 'mkfs']       # refuse matching shell commands
/// ```
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct HookSettings {
    pub redact_secrets: bool,
    pub secret_patterns: Vec<String>,
    pub deny_shell: Vec<String>,
}

impl Default for HookSettings {
    fn default() -> Self {
        Self {
            redact_secrets: true,
            secret_patterns: Vec::new(),
            deny_shell: vec![
                // -rf, -fr, -Rf, -rfv ... at /, ~ or $HOME
                r"rm\s+-(?:[a-zA-Z]*[rR][a-zA-Z]*f|[a-zA-Z]*f[a-zA-Z]*[rR])[a-zA-Z]*\s+(/|~|\$HOME)(\s|$)".into(),
                r"\bmkfs(\.\w+)?\b".into(),
                r":\(\)\s*\{\s*:\|:&\s*\};:".into(),
                r"\bdd\s+.*of=/dev/(sd|nvme|hd)".into(),
            ],
        }
    }
}

/// ```toml
/// [server]
/// auth = "auto"              # auto | required | off
/// rate_limit_per_minute = 600
/// max_background_runs = 4
/// ```
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerSettings {
    pub auth: AuthMode,
    /// Requests per minute per API key (or per anonymous caller). 0 = no limit.
    pub rate_limit_per_minute: u32,
    /// Background runs executed at once; more wait in the queue.
    pub max_background_runs: usize,
    /// gai as an MCP server, at `/mcp` and through `gai mcp serve`.
    pub mcp: McpServeSettings,
    /// gai agents over A2A, at `/a2a/{agent}` with cards under `.well-known`.
    pub a2a: A2aServeSettings,
    /// Signing in to the dashboard as a user: OIDC, or a trusted header
    /// from a proxy that did the sign-in (SAML and the like).
    pub sso: SsoSettings,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            auth: AuthMode::Auto,
            rate_limit_per_minute: 600,
            max_background_runs: 4,
            mcp: McpServeSettings::default(),
            a2a: A2aServeSettings::default(),
            sso: SsoSettings::default(),
        }
    }
}

/// Users sign in through an identity provider and get the roles their
/// organizations gave them.
///
/// ```toml
/// [server.sso]
/// admins = ["ada@example.com"]     # owners of the default organization
/// default_role = "member"          # others join the default organization as this (default: invited only)
/// allowed_domains = ["example.com"]
///
/// [server.sso.oidc]
/// issuer = "https://accounts.example.com"
/// client_id = "gai"
/// client_secret_env = "GAI_OIDC_CLIENT_SECRET"
/// redirect_url = "https://gai.example.com/v1/auth/callback"
/// ```
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SsoSettings {
    pub oidc: Option<OidcSettings>,
    /// Behind a proxy that signs people in (oauth2-proxy, a SAML gateway):
    /// the header carrying the signed-in email. Only set this when the
    /// proxy removes the header from incoming requests.
    pub trusted_header: Option<TrustedHeader>,
    /// Emails that become owners of the default organization when they
    /// sign in.
    pub admins: Vec<String>,
    /// The role people who weren't invited get in the default organization
    /// at their first sign-in. None: they must be invited.
    pub default_role: Option<gai_store::Role>,
    /// Only emails at these domains may sign in (empty: any).
    pub allowed_domains: Vec<String>,
    /// How long a sign-in lasts, in hours (default 168, a week).
    pub session_hours: Option<u32>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OidcSettings {
    /// The provider's issuer URL; its `/.well-known/openid-configuration`
    /// says the rest.
    pub issuer: String,
    pub client_id: String,
    /// Environment variable holding the client secret (none for a public
    /// client, which uses PKCE alone).
    pub client_secret_env: Option<String>,
    /// This server's `/v1/auth/callback`, as the provider redirects to it.
    /// Defaults to the request's own address.
    pub redirect_url: Option<String>,
    /// Defaults to `openid email profile`.
    pub scopes: Option<String>,
    /// The button's label (default "Sign in with SSO").
    pub label: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TrustedHeader {
    /// e.g. `X-Forwarded-Email`.
    pub email: String,
    /// e.g. `X-Forwarded-User`.
    pub name: Option<String>,
}

/// What gai offers other agents over A2A.
///
/// ```toml
/// [server.a2a]
/// agents = ["researcher"]
/// default_agent = "researcher"      # for POST /a2a without an agent named
/// public_url = "https://gai.example.com"
/// ```
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct A2aServeSettings {
    pub enabled: bool,
    /// Stored agents offered (names, or prefixes ending in `*`).
    pub agents: Vec<String>,
    /// The agent `POST /a2a` uses when the message names none.
    pub default_agent: Option<String>,
    /// Base URL written into agent cards. Defaults to the request's host.
    pub public_url: Option<String>,
    /// How long a blocking `message/send` waits for the run to finish
    /// before answering with the task as it stands.
    pub wait_secs: u64,
}

impl Default for A2aServeSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            agents: vec!["*".into()],
            default_agent: None,
            public_url: None,
            wait_secs: 600,
        }
    }
}

/// What gai offers MCP clients (Claude Desktop, Claude Code, Cursor...).
///
/// ```toml
/// [server.mcp]
/// agents = ["researcher", "support-*"]   # names, or prefixes ending in *
/// workflows = ["*"]
/// collections = ["handbook"]
/// ```
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct McpServeSettings {
    pub enabled: bool,
    /// Stored agents offered as `agent_<name>` tools.
    pub agents: Vec<String>,
    /// Workflows offered as `workflow_<name>` tools.
    pub workflows: Vec<String>,
    /// Knowledge collections offered as resources and to `search_knowledge`.
    pub collections: Vec<String>,
    /// Run tools that need approval without asking. Otherwise such calls
    /// wait in the dashboard's approvals inbox.
    pub auto_approve: bool,
    /// How long a tool call waits for its run before answering with the
    /// run id (the run keeps going).
    pub call_timeout_secs: u64,
    /// Host names the HTTP endpoint accepts (DNS-rebinding protection).
    /// Empty: localhost only. `["*"]`: any.
    pub allowed_hosts: Vec<String>,
}

impl Default for McpServeSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            agents: vec!["*".into()],
            workflows: vec!["*".into()],
            collections: vec!["*".into()],
            auto_approve: false,
            call_timeout_secs: 600,
            allowed_hosts: Vec::new(),
        }
    }
}

impl McpServeSettings {
    /// Whether `name` is in `patterns` (`*`, `prefix*` or an exact name).
    pub fn matches(patterns: &[String], name: &str) -> bool {
        patterns.iter().any(|p| match p.strip_suffix('*') {
            Some(prefix) => name.starts_with(prefix),
            None => p == name,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    /// Open while no API key exists and the server is only reachable
    /// locally; keys are required once the first one is created.
    #[default]
    Auto,
    /// Every request needs a key.
    Required,
    /// No authentication at all. Only behind something that does it for you.
    Off,
}

impl std::str::FromStr for AuthMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "auto" => Ok(Self::Auto),
            "required" => Ok(Self::Required),
            "off" => Ok(Self::Off),
            other => Err(format!(
                "unknown auth mode {other:?}; use auto, required or off"
            )),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("could not load config{}: {source}", path.as_ref().map(|p| format!(" from {}", p.display())).unwrap_or_default())]
pub struct ConfigError {
    path: Option<PathBuf>,
    #[source]
    source: Box<figment::Error>,
}

impl Config {
    /// Load from `path` if given (it must exist), otherwise from the first of
    /// `$GAI_CONFIG`, `./gai.toml` and the user config dir
    /// (`~/.config/gai/gai.toml`), then apply `GAI_*` environment overrides.
    /// Returns the config and the file it came from.
    pub fn load(path: Option<&Path>) -> Result<(Self, Option<PathBuf>), ConfigError> {
        let file = match path {
            Some(p) => Some(p.to_path_buf()),
            None => Self::candidates().into_iter().find(|p| p.is_file()),
        };
        let mut figment = Figment::new();
        if let Some(f) = &file {
            figment = figment.merge(Toml::file_exact(f));
        }
        figment = figment.merge(Env::prefixed("GAI_").split("__").ignore(&["config"]));
        let config = figment.extract::<Config>().map_err(|e| ConfigError {
            path: file.clone(),
            source: Box::new(e),
        })?;
        Ok((config, file))
    }

    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        Figment::from(Toml::string(text))
            .extract()
            .map_err(|e| ConfigError {
                path: None,
                source: Box::new(e),
            })
    }

    fn candidates() -> Vec<PathBuf> {
        let mut c = Vec::new();
        if let Some(p) = std::env::var_os("GAI_CONFIG") {
            c.push(PathBuf::from(p));
        }
        c.push(PathBuf::from("gai.toml"));
        if let Some(dir) = dirs::config_dir() {
            c.push(dir.join("gai").join("gai.toml"));
        }
        c
    }

    pub fn data_dir(&self) -> PathBuf {
        self.data_dir
            .clone()
            .or_else(|| dirs::data_dir().map(|d| d.join("gai")))
            .unwrap_or_else(|| PathBuf::from(".gai"))
    }

    /// The Postgres URL, when `database` is one.
    pub fn postgres_url(&self) -> Option<String> {
        let d = self.database.as_ref()?.to_str()?;
        (d.starts_with("postgres://") || d.starts_with("postgresql://")).then(|| d.to_owned())
    }

    pub fn database(&self) -> PathBuf {
        self.database
            .clone()
            .unwrap_or_else(|| self.data_dir().join("gai.db"))
    }

    /// Skill folders: the configured ones, else `./skills` and
    /// `<data_dir>/skills`.
    pub fn skills_dirs(&self) -> Vec<PathBuf> {
        if self.skills_dirs.is_empty() {
            vec![PathBuf::from("skills"), self.data_dir().join("skills")]
        } else {
            self.skills_dirs.clone()
        }
    }

    /// Workflow folders: the configured ones, else `./workflows` and
    /// `<data_dir>/workflows`.
    pub fn workflows_dirs(&self) -> Vec<PathBuf> {
        if self.workflows_dirs.is_empty() {
            vec![
                PathBuf::from("workflows"),
                self.data_dir().join("workflows"),
            ]
        } else {
            self.workflows_dirs.clone()
        }
    }

    /// Agent file directories: the configured ones, else `./agents` and
    /// `<data_dir>/agents`.
    pub fn agents_dirs(&self) -> Vec<PathBuf> {
        if self.agents_dirs.is_empty() {
            vec![PathBuf::from("agents"), self.data_dir().join("agents")]
        } else {
            self.agents_dirs.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_providers_aliases_and_options() {
        let c = Config::from_toml(
            r#"
            default_model = "fast"

            [providers.box]
            dialect = "openai"
            base_url = "http://gpu-box:8000/v1"
            options = { think_tags = true }

            [[providers.box.models]]
            id = "qwen3-32b"
            capabilities = { tools = true, context_window = 131072 }
            pricing = { input = 0.1, output = 0.2 }

            [aliases]
            fast = "box/qwen3-32b"
            smart = { model = "anthropic/claude-opus-5-5", fallback = ["box/qwen3-32b"] }

            [retry]
            max_retries = 5
            "#,
        )
        .unwrap();
        assert_eq!(c.default_model.as_deref(), Some("fast"));
        assert_eq!(c.llm.retry.max_retries, 5);
        let gai = crate::Gai::from_config(c, None).unwrap();
        let info = gai.models().resolve("fast").unwrap();
        assert_eq!(info.id, "box/qwen3-32b");
        assert_eq!(info.capabilities.context_window, Some(131072));
        assert_eq!(gai.models().resolve("smart").unwrap().provider, "anthropic");
    }
}

#[cfg(test)]
mod hook_tests {
    use super::HookSettings;

    #[test]
    fn default_shell_guard_blocks_destructive_commands_only() {
        let patterns: Vec<regex::Regex> = HookSettings::default()
            .deny_shell
            .iter()
            .map(|p| regex::Regex::new(p).unwrap())
            .collect();
        let blocked = |c: &str| patterns.iter().any(|p| p.is_match(c));
        for bad in [
            "rm -rf /",
            "sudo rm -rf / --no-preserve-root",
            "rm -fr ~",
            "rm -rf $HOME",
            "mkfs.ext4 /dev/sda1",
            "dd if=/dev/zero of=/dev/sda",
            ":(){ :|:& };:",
        ] {
            assert!(blocked(bad), "should block {bad:?}");
        }
        for ok in [
            "rm -rf ./build",
            "rm -rf /tmp/scratch",
            "ls -la /",
            "echo mkfsfoo",
            "dd if=a of=b.img",
        ] {
            assert!(!blocked(ok), "should allow {ok:?}");
        }
    }
}
