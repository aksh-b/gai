//! MCP client: tools from Model Context Protocol servers, as gai tools.
//!
//! Servers speak stdio (a child process) or streamable HTTP. Each server's
//! tools are registered as `mcp__<server>__<tool>`, a name every model
//! provider accepts, so an agent can list `mcp__github__*`.
//!
//! ```toml
//! [mcp_servers.fs]
//! command = "npx"
//! args = ["-y", "@modelcontextprotocol/server-filesystem", "/data"]
//!
//! [mcp_servers.search]
//! url = "https://mcp.example.com/mcp"
//! headers = { Authorization = "Bearer ${SEARCH_TOKEN}" }
//! policy = "approve"
//! ```

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use gai_core::{ContentBlock, MediaSource, ToolSpec};
use rmcp::RoleClient;
use rmcp::model::CallToolRequestParams;
use rmcp::service::{RunningService, ServiceExt};
use rmcp::transport::auth::{
    AuthClient, AuthError, AuthorizationCallback, AuthorizationManager, AuthorizationRequest,
    AuthorizationSession, CredentialStore, StoredCredentials,
};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{ConfigureCommandExt, StreamableHttpClientTransport, TokioChildProcess};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::sync::Mutex;

use crate::{SharedTool, Tool, ToolCtx, ToolError, ToolOutput, ToolPolicy};

/// How to reach one MCP server.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct McpServerConfig {
    /// Program to run for a stdio server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Extra environment for the child. Values may use `${VAR}`.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Endpoint of a streamable HTTP server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// HTTP headers. Values may use `${VAR}`, so tokens stay out of config.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// Policy for this server's tools. Defaults to `auto`, except tools the
    /// server marks destructive, which default to `approve`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<ToolPolicy>,
    /// Per-tool policies, by the server's own tool name.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub tool_policy: BTreeMap<String, ToolPolicy>,
    /// Only expose these tools (server names). Empty means all.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    /// Seconds a tool call may take. Default 120.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

impl McpServerConfig {
    pub fn validate(&self) -> Result<(), String> {
        match (&self.command, &self.url) {
            (Some(_), Some(_)) => {
                Err("give either `command` (stdio) or `url` (HTTP), not both".into())
            }
            (None, None) => Err("give `command` (stdio) or `url` (HTTP)".into()),
            _ => Ok(()),
        }
    }
}

/// Expand `${VAR}` from the environment; unknown variables become empty.
pub(crate) fn expand(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        match rest[start + 2..].find('}') {
            Some(end) => {
                let var = &rest[start + 2..start + 2 + end];
                out.push_str(&std::env::var(var).unwrap_or_default());
                rest = &rest[start + 3 + end..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// A name every provider accepts: `[A-Za-z0-9_-]`, at most 64 characters.
pub fn tool_name(server: &str, tool: &str) -> String {
    let clean = |s: &str| {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>()
    };
    let mut name = format!("mcp__{}__{}", clean(server), clean(tool));
    name.truncate(64);
    name
}

/// The prefix of every tool from `server`.
pub fn tool_prefix(server: &str) -> String {
    let clean: String = server
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("mcp__{clean}__")
}

type Client = RunningService<RoleClient, ()>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "state", content = "message")]
pub enum McpStatus {
    Connecting,
    Ready,
    /// The server wants an OAuth sign-in before it will talk.
    NeedsAuth(String),
    Failed(String),
    Disabled,
}

/// Prefix marking a connection error as "the server requires sign-in".
const AUTH_REQUIRED: &str = "sign-in required: ";

/// Where OAuth credentials for MCP servers are kept (the gai store, in
/// practice). Values are opaque JSON owned by this module.
#[async_trait]
pub trait McpCredentials: Send + Sync {
    async fn load(&self, server: &str) -> Option<Value>;
    async fn save(&self, server: &str, credentials: Value);
    async fn clear(&self, server: &str);
}

/// Bridges [`McpCredentials`] to rmcp's per-server credential store.
struct CredentialAdapter {
    server: String,
    store: Arc<dyn McpCredentials>,
}

#[async_trait]
impl CredentialStore for CredentialAdapter {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        Ok(self
            .store
            .load(&self.server)
            .await
            .and_then(|v| serde_json::from_value(v).ok()))
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let value = serde_json::to_value(&credentials)
            .map_err(|e| AuthError::InternalError(e.to_string()))?;
        self.store.save(&self.server, value).await;
        Ok(())
    }

    async fn clear(&self) -> Result<(), AuthError> {
        self.store.clear(&self.server).await;
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct McpServerInfo {
    pub name: String,
    pub config: McpServerConfig,
    pub status: McpStatus,
    /// The gai names of the tools it provides.
    pub tools: Vec<String>,
    /// Where the server was declared: `config` or `api`.
    pub source: String,
}

/// One connected (or connecting) MCP server.
pub struct McpServer {
    pub name: String,
    pub config: McpServerConfig,
    pub source: String,
    client: Mutex<Option<Arc<Client>>>,
    status: StdMutex<McpStatus>,
    tools: StdMutex<Vec<String>>,
    credentials: std::sync::OnceLock<Arc<dyn McpCredentials>>,
}

impl McpServer {
    pub fn new(name: &str, config: McpServerConfig, source: &str) -> Arc<Self> {
        let disabled = config.enabled == Some(false);
        Arc::new(Self {
            name: name.to_owned(),
            config,
            source: source.to_owned(),
            client: Mutex::new(None),
            status: StdMutex::new(if disabled {
                McpStatus::Disabled
            } else {
                McpStatus::Connecting
            }),
            tools: StdMutex::new(Vec::new()),
            credentials: std::sync::OnceLock::new(),
        })
    }

    /// Where this server's OAuth tokens live. Set once, by the manager.
    pub fn set_credentials(&self, store: Arc<dyn McpCredentials>) {
        let _ = self.credentials.set(store);
    }

    fn adapter(&self) -> Option<CredentialAdapter> {
        self.credentials.get().map(|store| CredentialAdapter {
            server: self.name.clone(),
            store: store.clone(),
        })
    }

    /// An OAuth manager holding this server's saved tokens, if it has any.
    async fn saved_authorization(&self, url: &str) -> Option<AuthorizationManager> {
        let adapter = self.adapter()?;
        adapter.load().await.ok()??;
        let result = async {
            let mut manager = AuthorizationManager::new(url).await?;
            manager.set_credential_store(adapter);
            Ok::<_, AuthError>(manager.initialize_from_store().await?.then_some(manager))
        }
        .await;
        result.unwrap_or_else(|e| {
            tracing::warn!(server = %self.name, error = %e, "could not use saved MCP sign-in");
            None
        })
    }

    /// Start an OAuth sign-in: discover the server's authorization server,
    /// register gai as a client, and return the session whose `auth_url`
    /// the user must open. `redirect_uri` must lead back to gai.
    pub async fn begin_sign_in(&self, redirect_uri: &str) -> Result<AuthorizationSession, String> {
        let url = expand(
            self.config
                .url
                .as_deref()
                .ok_or("only HTTP MCP servers use sign-in")?,
        );
        let adapter = self
            .adapter()
            .ok_or("this server has nowhere to keep sign-in tokens")?;
        let mut manager = AuthorizationManager::new(url.as_str())
            .await
            .map_err(|e| e.to_string())?;
        let resolution = manager
            .resolve_metadata()
            .await
            .map_err(|e| format!("the server does not advertise OAuth sign-in: {e}"))?;
        manager.set_metadata(resolution.metadata);
        manager.set_credential_store(adapter);
        let request = AuthorizationRequest::new(redirect_uri).with_client_name("gai");
        AuthorizationSession::new(manager, request)
            .await
            .map_err(|(_, e)| format!("could not start sign-in: {e}"))
    }

    /// Forget saved tokens.
    pub async fn sign_out(&self) {
        if let Some(store) = self.credentials.get() {
            store.clear(&self.name).await;
        }
    }

    pub fn info(&self) -> McpServerInfo {
        McpServerInfo {
            name: self.name.clone(),
            config: self.config.clone(),
            status: self.status.lock().expect("status lock").clone(),
            tools: self.tools.lock().expect("tools lock").clone(),
            source: self.source.clone(),
        }
    }

    fn set_status(&self, s: McpStatus) {
        *self.status.lock().expect("status lock") = s;
    }

    async fn open(&self) -> Result<Client, String> {
        self.config.validate()?;
        let client = if let Some(cmd) = &self.config.command {
            let cfg = self.config.clone();
            let transport =
                TokioChildProcess::new(tokio::process::Command::new(expand(cmd)).configure(|c| {
                    c.args(cfg.args.iter().map(|a| expand(a)));
                    for (k, v) in &cfg.env {
                        c.env(k, expand(v));
                    }
                    if let Some(dir) = &cfg.cwd {
                        c.current_dir(expand(dir));
                    }
                    c.kill_on_drop(true);
                }))
                .map_err(|e| format!("could not start {cmd}: {e}"))?;
            ().serve(transport)
                .await
                .map_err(|e| format!("handshake failed: {e}"))?
        } else {
            let url = expand(self.config.url.as_deref().unwrap_or_default());
            let mut headers = std::collections::HashMap::new();
            for (k, v) in &self.config.headers {
                let name = reqwest_header_name(k)?;
                let value = expand(v)
                    .parse()
                    .map_err(|_| format!("invalid value for header {k}"))?;
                headers.insert(name, value);
            }
            let config =
                StreamableHttpClientTransportConfig::with_uri(url.clone()).custom_headers(headers);
            let served = match self.saved_authorization(&url).await {
                Some(manager) => {
                    let client = AuthClient::new(reqwest013::Client::default(), manager);
                    ().serve(StreamableHttpClientTransport::with_client(client, config))
                        .await
                }
                None => ().serve(StreamableHttpClientTransport::from_config(config)).await,
            };
            served.map_err(|e| {
                let msg = e.to_string();
                if msg.contains("Auth required") || msg.contains("AuthorizationRequired") {
                    format!("{AUTH_REQUIRED}the server needs you to sign in")
                } else {
                    format!("handshake failed: {msg}")
                }
            })?
        };
        Ok(client)
    }

    /// Connect (or reconnect) and list tools.
    pub async fn connect(self: &Arc<Self>) -> Result<Vec<SharedTool>, String> {
        if self.config.enabled == Some(false) {
            self.set_status(McpStatus::Disabled);
            return Ok(Vec::new());
        }
        self.set_status(McpStatus::Connecting);
        let result = async {
            let client = Arc::new(self.open().await?);
            let listed = tokio::time::timeout(Duration::from_secs(30), client.list_all_tools())
                .await
                .map_err(|_| "listing tools timed out".to_owned())?
                .map_err(|e| format!("listing tools failed: {e}"))?;
            *self.client.lock().await = Some(client);
            Ok::<_, String>(listed)
        }
        .await;
        match result {
            Ok(listed) => {
                let tools: Vec<SharedTool> = listed
                    .into_iter()
                    .filter(|t| {
                        self.config.include.is_empty()
                            || self.config.include.iter().any(|i| i == t.name.as_ref())
                    })
                    .map(|t| Arc::new(McpTool::new(self.clone(), t)) as SharedTool)
                    .collect();
                *self.tools.lock().expect("tools lock") =
                    tools.iter().map(|t| t.spec().name).collect();
                self.set_status(McpStatus::Ready);
                Ok(tools)
            }
            Err(e) => {
                tracing::warn!(server = %self.name, error = %e, "MCP server unavailable");
                match e.strip_prefix(AUTH_REQUIRED) {
                    Some(why) => self.set_status(McpStatus::NeedsAuth(why.to_owned())),
                    None => self.set_status(McpStatus::Failed(e.clone())),
                }
                Err(e)
            }
        }
    }

    pub async fn close(&self) {
        if let Some(c) = self.client.lock().await.take()
            && let Ok(c) = Arc::try_unwrap(c)
        {
            let _ = c.cancel().await;
        }
    }

    async fn call(
        self: &Arc<Self>,
        tool: &str,
        args: Map<String, Value>,
    ) -> Result<rmcp::model::CallToolResult, String> {
        // One reconnect if the connection dropped (the child exited, the
        // HTTP session expired).
        for attempt in 0..2 {
            let client = self.client.lock().await.clone();
            let client = match client {
                Some(c) if !c.is_transport_closed() => c,
                _ => {
                    self.connect().await?;
                    self.client.lock().await.clone().ok_or("not connected")?
                }
            };
            let params = CallToolRequestParams::new(tool.to_owned()).with_arguments(args.clone());
            match client.call_tool(params).await {
                Ok(r) => return Ok(r),
                Err(e) if attempt == 0 && client.is_transport_closed() => {
                    tracing::warn!(server = %self.name, error = %e, "MCP connection lost; reconnecting");
                    *self.client.lock().await = None;
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("MCP server connection lost".into())
    }
}

fn reqwest_header_name(k: &str) -> Result<rmcp_http::HeaderName, String> {
    rmcp_http::HeaderName::from_bytes(k.as_bytes()).map_err(|_| format!("invalid header name {k}"))
}

/// The `http` types rmcp's HTTP transport uses.
mod rmcp_http {
    pub use reqwest::header::HeaderName;
}

/// One MCP tool, presented as a gai tool.
pub struct McpTool {
    server: Arc<McpServer>,
    remote_name: String,
    spec: ToolSpec,
    policy: ToolPolicy,
    idempotent: bool,
}

impl McpTool {
    fn new(server: Arc<McpServer>, t: rmcp::model::Tool) -> Self {
        let remote_name = t.name.to_string();
        let ann = t.annotations.as_ref();
        let read_only = ann.and_then(|a| a.read_only_hint).unwrap_or(false);
        let destructive = ann.and_then(|a| a.destructive_hint).unwrap_or(false);
        let policy = server
            .config
            .tool_policy
            .get(&remote_name)
            .copied()
            .or(server.config.policy)
            .unwrap_or(if destructive && !read_only {
                ToolPolicy::Approve
            } else {
                ToolPolicy::Auto
            });
        let mut description = t.description.map(|d| d.to_string()).unwrap_or_default();
        if description.is_empty() {
            description = format!("{remote_name} (from the {} MCP server)", server.name);
        }
        let mut schema = Value::Object((*t.input_schema).clone());
        if schema.get("type").is_none() {
            schema["type"] = json!("object");
        }
        Self {
            spec: ToolSpec {
                name: tool_name(&server.name, &remote_name),
                description,
                input_schema: schema,
                strict: false,
            },
            idempotent: read_only || ann.and_then(|a| a.idempotent_hint).unwrap_or(false),
            remote_name,
            policy,
            server,
        }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    fn default_policy(&self) -> ToolPolicy {
        self.policy
    }

    fn idempotent(&self) -> bool {
        self.idempotent
    }

    async fn call(&self, ctx: &ToolCtx, input: Value) -> Result<ToolOutput, ToolError> {
        let args = match input {
            Value::Object(m) => m,
            _ => {
                return Err(ToolError::InvalidInput(
                    "arguments must be a JSON object".into(),
                ));
            }
        };
        let secs = self.server.config.timeout_secs.unwrap_or(120);
        let result = tokio::select! {
            r = tokio::time::timeout(Duration::from_secs(secs), self.server.call(&self.remote_name, args)) => {
                r.map_err(|_| ToolError::Timeout(secs))?.map_err(ToolError::Failed)?
            }
            _ = ctx.cancel.cancelled() => return Err(ToolError::Cancelled),
        };
        Ok(convert_result(
            &serde_json::to_value(&result).unwrap_or_default(),
        ))
    }
}

/// MCP content to gai content. Text and images map directly; resources
/// become their text; anything else is shown as JSON.
pub fn convert_result(v: &Value) -> ToolOutput {
    let mut content = Vec::new();
    for item in v["content"].as_array().into_iter().flatten() {
        match item["type"].as_str() {
            Some("text") => content.push(ContentBlock::text(
                item["text"].as_str().unwrap_or_default(),
            )),
            Some("image") => content.push(ContentBlock::Image {
                source: MediaSource::Base64 {
                    media_type: item["mimeType"].as_str().unwrap_or("image/png").to_owned(),
                    data: item["data"].as_str().unwrap_or_default().to_owned(),
                },
            }),
            Some("resource") => {
                let r = &item["resource"];
                let text = r["text"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("[resource {}]", r["uri"].as_str().unwrap_or("")));
                content.push(ContentBlock::text(text));
            }
            Some("resource_link") => content.push(ContentBlock::text(format!(
                "[{}]({})",
                item["name"].as_str().unwrap_or("resource"),
                item["uri"].as_str().unwrap_or("")
            ))),
            _ => content.push(ContentBlock::text(item.to_string())),
        }
    }
    if content.is_empty()
        && let Some(s) = v.get("structuredContent").filter(|s| !s.is_null())
    {
        content.push(ContentBlock::text(
            serde_json::to_string_pretty(s).unwrap_or_default(),
        ));
    }
    if content.is_empty() {
        content.push(ContentBlock::text("(no output)"));
    }
    ToolOutput {
        content,
        is_error: v["isError"].as_bool().unwrap_or(false),
        cost: None,
    }
}

/// How long a started sign-in (and its outcome) is kept.
const SIGN_IN_TTL: Duration = Duration::from_secs(15 * 60);

/// Longest a code exchange may take.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(60);

type SignInOutcome = Option<Result<String, String>>;

/// One sign-in attempt, from the authorization URL to its outcome.
///
/// Some providers' consent pages hit the redirect URI more than once (a
/// background request, then a navigation), and a browser may abandon the
/// first. So the code exchange runs in its own task, started by whichever
/// callback arrives first, and every callback for the attempt waits for
/// that one outcome.
struct SignIn {
    server: String,
    started: std::time::Instant,
    /// The session until a callback claims it; then the outcome channel.
    stage: StdMutex<(
        Option<AuthorizationSession>,
        Option<tokio::sync::watch::Receiver<SignInOutcome>>,
    )>,
}

/// All MCP servers of a runtime.
#[derive(Default)]
pub struct McpManager {
    servers: StdMutex<BTreeMap<String, Arc<McpServer>>>,
    /// Signalled once the initial connections have settled.
    settled: tokio::sync::Notify,
    settled_flag: std::sync::atomic::AtomicBool,
    credentials: std::sync::OnceLock<Arc<dyn McpCredentials>>,
    /// Sign-in attempts by OAuth `state`.
    sign_ins: StdMutex<BTreeMap<String, Arc<SignIn>>>,
}

impl McpManager {
    /// Keep OAuth tokens of every server in `store`.
    pub fn set_credentials(&self, store: Arc<dyn McpCredentials>) {
        let _ = self.credentials.set(store);
    }

    pub fn add(&self, server: Arc<McpServer>) -> Option<Arc<McpServer>> {
        if let Some(store) = self.credentials.get() {
            server.set_credentials(store.clone());
        }
        self.servers
            .lock()
            .expect("mcp lock")
            .insert(server.name.clone(), server)
    }

    /// Begin signing in to `server`; returns the URL the user must open.
    /// After they approve, the browser lands on `redirect_uri`, which must
    /// call [`finish_sign_in`](Self::finish_sign_in) with the full URL.
    pub async fn begin_sign_in(&self, server: &str, redirect_uri: &str) -> Result<String, String> {
        let s = self
            .get(server)
            .ok_or_else(|| format!("no MCP server {server}"))?;
        let session = s.begin_sign_in(redirect_uri).await?;
        let url = session.auth_url.clone();
        let state = reqwest::Url::parse(&url)
            .ok()
            .and_then(|u| {
                u.query_pairs()
                    .find(|(k, _)| k == "state")
                    .map(|(_, v)| v.into_owned())
            })
            .ok_or("the authorization URL has no state")?;
        let mut attempts = self.sign_ins.lock().expect("sign-in lock");
        // Attempts stay valid for a while: the user may finish an earlier
        // tab after starting another.
        attempts.retain(|_, a| a.started.elapsed() < SIGN_IN_TTL);
        attempts.insert(
            state.clone(),
            Arc::new(SignIn {
                server: server.to_owned(),
                started: std::time::Instant::now(),
                stage: StdMutex::new((Some(session), None)),
            }),
        );
        tracing::info!(server, state = %&state[..state.len().min(8)], "MCP sign-in started");
        Ok(url)
    }

    /// Complete a sign-in from the URL the browser was redirected to.
    /// Returns the server's name; tokens are saved, ready for `connect`.
    /// Safe to call more than once for the same redirect.
    pub async fn finish_sign_in(&self, callback_url: &str) -> Result<String, String> {
        let callback =
            AuthorizationCallback::from_redirect_url(callback_url).map_err(|e| e.to_string())?;
        let state = callback.csrf_token.clone();
        let attempt = self
            .sign_ins
            .lock()
            .expect("sign-in lock")
            .get(&state)
            .cloned()
            .ok_or("this sign-in link expired or was already used; start again")?;
        let mut outcome = {
            let mut stage = attempt.stage.lock().expect("sign-in stage lock");
            match stage.0.take() {
                Some(session) => {
                    let (tx, rx) = tokio::sync::watch::channel(None);
                    stage.1 = Some(rx.clone());
                    let server = attempt.server.clone();
                    tokio::spawn(async move {
                        let exchange = session.handle_callback_with_issuer(
                            &callback.code,
                            &callback.csrf_token,
                            callback.issuer.as_deref(),
                        );
                        let result = match tokio::time::timeout(EXCHANGE_TIMEOUT, exchange).await {
                            Ok(Ok(_)) => Ok(server.clone()),
                            Ok(Err(e)) => Err(format!("sign-in failed: {e}")),
                            Err(_) => {
                                Err("sign-in failed: the provider did not answer in time".into())
                            }
                        };
                        match &result {
                            Ok(_) => tracing::info!(server, "MCP sign-in completed"),
                            Err(e) => tracing::warn!(server, error = %e, "MCP sign-in failed"),
                        }
                        let _ = tx.send(Some(result));
                    });
                    rx
                }
                None => {
                    tracing::info!(server = %attempt.server, "MCP sign-in callback repeated; waiting for the first");
                    stage
                        .1
                        .clone()
                        .expect("a claimed sign-in has an outcome channel")
                }
            }
        };
        loop {
            if let Some(result) = outcome.borrow().clone() {
                return result;
            }
            if outcome.changed().await.is_err() {
                return Err("sign-in was interrupted; start again".into());
            }
        }
    }

    pub fn remove(&self, name: &str) -> Option<Arc<McpServer>> {
        self.servers.lock().expect("mcp lock").remove(name)
    }

    pub fn get(&self, name: &str) -> Option<Arc<McpServer>> {
        self.servers.lock().expect("mcp lock").get(name).cloned()
    }

    pub fn list(&self) -> Vec<McpServerInfo> {
        self.servers
            .lock()
            .expect("mcp lock")
            .values()
            .map(|s| s.info())
            .collect()
    }

    pub fn servers(&self) -> Vec<Arc<McpServer>> {
        self.servers
            .lock()
            .expect("mcp lock")
            .values()
            .cloned()
            .collect()
    }

    pub fn mark_settled(&self) {
        self.settled_flag
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.settled.notify_waiters();
    }

    /// Wait (up to `max`) for the initial connections to finish.
    pub async fn wait_settled(&self, max: Duration) {
        let notified = self.settled.notified();
        if self.settled_flag.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let _ = tokio::time::timeout(max, notified).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_provider_safe() {
        assert_eq!(
            tool_name("git hub", "create.issue"),
            "mcp__git_hub__create_issue"
        );
        assert!(tool_name("s", &"x".repeat(100)).len() <= 64);
        assert_eq!(tool_prefix("fs"), "mcp__fs__");
    }

    #[test]
    fn expands_env_vars() {
        // SAFETY: tests in this module do not read this variable concurrently.
        unsafe { std::env::set_var("GAI_MCP_TEST_TOKEN", "t0k") };
        assert_eq!(expand("Bearer ${GAI_MCP_TEST_TOKEN}!"), "Bearer t0k!");
        assert_eq!(expand("${GAI_MCP_SURELY_UNSET}x"), "x");
        assert_eq!(expand("no vars"), "no vars");
    }

    #[test]
    fn converts_mcp_content() {
        let out = convert_result(&json!({
            "content": [{"type": "text", "text": "hi"}, {"type": "image", "data": "AAA", "mimeType": "image/png"}],
            "isError": true
        }));
        assert!(out.is_error);
        assert_eq!(out.as_text(), "hi");
        assert!(matches!(out.content[1], ContentBlock::Image { .. }));
        let structured = convert_result(&json!({"content": [], "structuredContent": {"n": 1}}));
        assert!(structured.as_text().contains("\"n\": 1"));
    }

    #[test]
    fn config_needs_exactly_one_transport() {
        assert!(McpServerConfig::default().validate().is_err());
        assert!(
            McpServerConfig {
                command: Some("x".into()),
                url: Some("y".into()),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            McpServerConfig {
                url: Some("y".into()),
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
    }
}
