//! gai as an MCP server: stored agents and workflows become tools, and
//! knowledge collections become resources, for any MCP client (Claude
//! Desktop, Claude Code, Cursor...). Served over streamable HTTP at `/mcp`
//! (behind the usual API-key auth) and over stdio by `gai mcp serve`.

use std::sync::Arc;
use std::time::Duration;

use gai::agent::{RunOutcome, RunRequest, Runtime};
use gai::memory::{SearchMode, format_hits};
use gai::orchestra::{WorkflowRequest, WorkflowSpec, Workflows};
use gai::store::RunStatus;
use gai::{Gai, McpServeSettings};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    JsonObject, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde_json::{Map, Value, json};

use crate::auth::Principal;

const AGENT_PREFIX: &str = "agent_";
const WORKFLOW_PREFIX: &str = "workflow_";
const SEARCH_TOOL: &str = "search_knowledge";
const URI_PREFIX: &str = "gai://collections/";

/// The MCP face of a gai instance.
#[derive(Clone)]
pub struct GaiMcp {
    gai: Arc<Gai>,
    rt: Runtime,
    wf: Workflows,
}

/// A tool name made of the characters MCP clients accept.
fn tool_name(prefix: &str, name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{prefix}{clean}").chars().take(64).collect()
}

fn schema(v: Value) -> Arc<JsonObject> {
    Arc::new(v.as_object().cloned().unwrap_or_default())
}

fn internal(e: impl std::fmt::Display) -> McpError {
    McpError::internal_error(e.to_string(), None)
}

impl GaiMcp {
    pub fn new(gai: Arc<Gai>, rt: Runtime, wf: Workflows) -> Self {
        Self { gai, rt, wf }
    }

    fn settings(&self) -> &McpServeSettings {
        &self.gai.config().server.mcp
    }

    /// Stored agents on offer, by name.
    async fn agents(&self) -> Result<Vec<(String, Option<String>)>, McpError> {
        let mut out = Vec::new();
        for a in self.rt.store().list_agents().await.map_err(internal)? {
            if McpServeSettings::matches(&self.settings().agents, &a.name) {
                let desc = self
                    .rt
                    .load_agent(&a.name, None)
                    .await
                    .ok()
                    .and_then(|(s, _)| s.description);
                out.push((a.name, desc));
            }
        }
        Ok(out)
    }

    /// Workflows on offer (stored, then files), with their specs.
    async fn workflows(&self) -> Result<Vec<WorkflowSpec>, McpError> {
        let mut names: Vec<String> = self
            .rt
            .store()
            .list_workflows()
            .await
            .map_err(internal)?
            .into_iter()
            .map(|w| w.name)
            .collect();
        for (n, _) in self.wf.files() {
            if !names.contains(&n) {
                names.push(n);
            }
        }
        let mut out = Vec::new();
        for n in names {
            if McpServeSettings::matches(&self.settings().workflows, &n)
                && let Ok((spec, _)) = self.wf.resolve(&n).await
            {
                out.push(spec);
            }
        }
        Ok(out)
    }

    /// Knowledge collections on offer (never agents' memory).
    async fn collections(&self) -> Result<Vec<gai::store::Collection>, McpError> {
        Ok(self
            .rt
            .store()
            .list_collections()
            .await
            .map_err(internal)?
            .into_iter()
            .filter(|c| {
                !c.name.starts_with("memory:")
                    && McpServeSettings::matches(&self.settings().collections, &c.name)
            })
            .collect())
    }

    pub async fn tools(&self) -> Result<Vec<Tool>, McpError> {
        let mut tools = Vec::new();
        for (name, desc) in self.agents().await? {
            tools.push(Tool::new(
                tool_name(AGENT_PREFIX, &name),
                format!(
                    "Give the gai agent {name:?} a task and get its answer. {}Pass session_id from an earlier answer to continue that conversation.",
                    desc.map(|d| format!("{d} ")).unwrap_or_default()
                ),
                schema(json!({
                    "type": "object",
                    "properties": {
                        "task": {"type": "string", "description": "What the agent should do, with any context it needs."},
                        "session_id": {"type": "string", "description": "Continue this conversation (from an earlier result)."}
                    },
                    "required": ["task"]
                })),
            ));
        }
        for spec in self.workflows().await? {
            let mut props = Map::new();
            let mut required = Vec::new();
            for (k, i) in &spec.inputs {
                let mut p = json!({});
                if let Some(d) = &i.description {
                    p["description"] = json!(d);
                }
                match &i.default {
                    Some(d) => p["default"] = d.clone(),
                    None => required.push(k.clone()),
                }
                props.insert(k.clone(), p);
            }
            tools.push(Tool::new(
                tool_name(WORKFLOW_PREFIX, &spec.name),
                format!(
                    "Run the gai workflow {:?} and get its output. {}",
                    spec.name,
                    spec.description.clone().unwrap_or_default()
                ),
                schema(json!({"type": "object", "properties": props, "required": required})),
            ));
        }
        let collections = self.collections().await?;
        if !collections.is_empty() {
            let names: Vec<&str> = collections.iter().map(|c| c.name.as_str()).collect();
            tools.push(Tool::new(
                SEARCH_TOOL,
                format!(
                    "Search gai's knowledge collections ({}) by meaning and keywords; returns the best matching passages with their sources.",
                    names.join(", ")
                ),
                schema(json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"},
                        "collection": {"type": "string", "enum": names, "description": "Only this collection (default: all)."},
                        "k": {"type": "integer", "minimum": 1, "maximum": 20, "description": "How many passages (default 6)."}
                    },
                    "required": ["query"]
                })),
            ));
        }
        Ok(tools)
    }

    /// Run an agent or workflow tool, or search, as `principal`.
    pub async fn call(
        &self,
        name: &str,
        args: JsonObject,
        principal: Option<&Principal>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<CallToolResult, McpError> {
        if let Some(p) = principal {
            p.check_budget()
                .map_err(|e| McpError::invalid_request(e.message().to_owned(), None))?;
        }
        let mut meta = Map::new();
        meta.insert("source".into(), json!("mcp"));
        if let Some(id) = principal.and_then(Principal::key_id) {
            meta.insert("key_id".into(), json!(id));
        }
        let timeout = Duration::from_secs(self.settings().call_timeout_secs.max(1));

        if name == SEARCH_TOOL {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let mut names: Vec<String> = self
                .collections()
                .await?
                .into_iter()
                .map(|c| c.name)
                .collect();
            if let Some(c) = args.get("collection").and_then(Value::as_str) {
                if !names.iter().any(|n| n == c) {
                    return Err(McpError::invalid_params(
                        format!("no collection {c:?}"),
                        None,
                    ));
                }
                names = vec![c.to_owned()];
            }
            let k = args
                .get("k")
                .and_then(Value::as_u64)
                .unwrap_or(6)
                .clamp(1, 20) as usize;
            return match self
                .rt
                .knowledge()
                .search(&names, query, k, SearchMode::Hybrid)
                .await
            {
                Ok(hits) => Ok(CallToolResult::success(vec![ContentBlock::text(
                    format_hits(&hits),
                )])),
                Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(
                    e.to_string(),
                )])),
            };
        }

        if name.starts_with(AGENT_PREFIX) {
            let agent = self
                .agents()
                .await?
                .into_iter()
                .map(|(n, _)| n)
                .find(|n| tool_name(AGENT_PREFIX, n) == name)
                .ok_or_else(|| McpError::invalid_params(format!("no tool {name}"), None))?;
            let task = args
                .get("task")
                .and_then(Value::as_str)
                .filter(|t| !t.trim().is_empty())
                .ok_or_else(|| McpError::invalid_params("task is required", None))?;
            let (spec, version) = self.rt.resolve_agent(&agent).await.map_err(internal)?;
            let mut req = RunRequest::new(spec, task);
            req.agent_version = version;
            req.session_id = args
                .get("session_id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            req.auto_approve = self.settings().auto_approve;
            req.metadata = meta;
            let handle = match self.rt.start(req).await {
                Ok(h) => h,
                Err(e) => {
                    return Ok(CallToolResult::error(vec![ContentBlock::text(
                        e.to_string(),
                    )]));
                }
            };
            let (run_id, session_id) = (handle.run_id.clone(), handle.session_id.clone());
            tokio::select! {
                out = handle.wait() => Ok(result(out)),
                _ = tokio::time::sleep(timeout) => Ok(still_running(&run_id, &session_id)),
                _ = cancel.cancelled() => {
                    self.rt.cancel(&run_id);
                    Ok(CallToolResult::error(vec![ContentBlock::text("cancelled")]))
                }
            }
        } else if name.starts_with(WORKFLOW_PREFIX) {
            let spec = self
                .workflows()
                .await?
                .into_iter()
                .find(|s| tool_name(WORKFLOW_PREFIX, &s.name) == name)
                .ok_or_else(|| McpError::invalid_params(format!("no tool {name}"), None))?;
            let (_, version) = self.wf.resolve(&spec.name).await.map_err(internal)?;
            let mut req = WorkflowRequest::new(spec, args);
            req.version = version;
            req.auto_approve = self.settings().auto_approve;
            req.metadata = meta;
            let handle = match self.wf.start(req).await {
                Ok(h) => h,
                Err(e) => {
                    return Ok(CallToolResult::error(vec![ContentBlock::text(
                        e.to_string(),
                    )]));
                }
            };
            let (run_id, session_id) = (handle.run_id.clone(), handle.session_id.clone());
            tokio::select! {
                out = handle.wait() => Ok(result(out)),
                _ = tokio::time::sleep(timeout) => Ok(still_running(&run_id, &session_id)),
                _ = cancel.cancelled() => {
                    self.rt.cancel(&run_id);
                    Ok(CallToolResult::error(vec![ContentBlock::text("cancelled")]))
                }
            }
        } else {
            Err(McpError::invalid_params(format!("no tool {name}"), None))
        }
    }

    pub async fn resources(&self) -> Result<Vec<Resource>, McpError> {
        let mut out = Vec::new();
        for c in self.collections().await? {
            out.push(
                Resource::new(format!("{URI_PREFIX}{}", c.name), c.name.clone())
                    .with_title(format!("Collection {}", c.name))
                    .with_description(c.description.clone().unwrap_or_else(|| {
                        format!("{} documents in gai's knowledge base", c.documents)
                    }))
                    .with_mime_type("text/markdown"),
            );
            let docs = self
                .rt
                .store()
                .list_documents(&c.name, 200, 0)
                .await
                .map_err(internal)?;
            for d in docs {
                out.push(
                    Resource::new(
                        format!("{URI_PREFIX}{}/documents/{}", c.name, d.id),
                        d.title.clone().unwrap_or_else(|| d.source.clone()),
                    )
                    .with_description(format!("{} (in {})", d.source, c.name))
                    .with_mime_type("text/plain")
                    .with_size(d.chars),
                );
            }
        }
        Ok(out)
    }

    pub async fn read(&self, uri: &str) -> Result<ReadResourceResult, McpError> {
        let not_found = || McpError::resource_not_found(format!("no resource {uri}"), None);
        let rest = uri.strip_prefix(URI_PREFIX).ok_or_else(not_found)?;
        let collections = self.collections().await?;
        let store = self.rt.store();
        match rest.split_once("/documents/") {
            None => {
                let c = collections
                    .iter()
                    .find(|c| c.name == rest)
                    .ok_or_else(not_found)?;
                let docs = store
                    .list_documents(&c.name, 500, 0)
                    .await
                    .map_err(internal)?;
                let mut md = format!("# {}\n\n", c.name);
                if let Some(d) = &c.description {
                    md.push_str(&format!("{d}\n\n"));
                }
                md.push_str(&format!(
                    "{} documents, {} chunks.\n\n",
                    c.documents, c.chunks
                ));
                for d in docs {
                    md.push_str(&format!(
                        "- [{}]({URI_PREFIX}{}/documents/{}) — {}\n",
                        d.title.as_deref().unwrap_or(&d.source),
                        c.name,
                        d.id,
                        d.source
                    ));
                }
                Ok(ReadResourceResult::new(vec![
                    ResourceContents::text(md, uri).with_mime_type("text/markdown"),
                ]))
            }
            Some((collection, id)) => {
                if !collections.iter().any(|c| c.name == collection) {
                    return Err(not_found());
                }
                let doc = store
                    .get_document(id)
                    .await
                    .map_err(internal)?
                    .ok_or_else(not_found)?;
                if doc.collection != collection {
                    return Err(not_found());
                }
                let text = store
                    .document_chunks(id)
                    .await
                    .map_err(internal)?
                    .into_iter()
                    .map(|c| c.text)
                    .collect::<Vec<_>>()
                    .join("\n\n");
                Ok(ReadResourceResult::new(vec![
                    ResourceContents::text(text, uri).with_mime_type("text/plain"),
                ]))
            }
        }
    }
}

/// A finished run as a tool result.
fn result(out: RunOutcome) -> CallToolResult {
    let text = if out.text.is_empty() {
        out.output
            .as_ref()
            .map(|o| match o {
                Value::String(s) => s.clone(),
                v => serde_json::to_string_pretty(v).unwrap_or_default(),
            })
            .unwrap_or_default()
    } else {
        out.text.clone()
    };
    let structured = json!({
        "run_id": out.run_id,
        "session_id": out.session_id,
        "status": out.status.as_str(),
        "output": out.output,
        "cost_usd": out.cost,
    });
    let mut r = if out.status == RunStatus::Completed {
        CallToolResult::success(vec![ContentBlock::text(text)])
    } else {
        CallToolResult::error(vec![ContentBlock::text(format!(
            "The run ended {}{}{}",
            out.status.as_str().replace('_', " "),
            out.error.map(|e| format!(": {e}")).unwrap_or_default(),
            if text.is_empty() {
                String::new()
            } else {
                format!("\n\nLast output:\n{text}")
            }
        ))])
    };
    r.structured_content = Some(structured);
    r
}

fn still_running(run_id: &str, session_id: &str) -> CallToolResult {
    let mut r = CallToolResult::success(vec![ContentBlock::text(format!(
        "Still running (it may be waiting for approval in the gai dashboard). Run {run_id}, session {session_id}."
    ))]);
    r.structured_content =
        Some(json!({"run_id": run_id, "session_id": session_id, "status": "running"}));
    r
}

/// The caller's API key, when the request came over HTTP with one.
fn principal(ctx: &RequestContext<RoleServer>) -> Option<Principal> {
    ctx.extensions
        .get::<axum::http::request::Parts>()
        .and_then(|p| p.extensions.get::<Principal>().cloned())
}

impl ServerHandler for GaiMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new("gai", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "gai runs AI agents and workflows. Each agent_* tool hands a task to a gai agent (which has its own tools, memory and model) and returns its answer; workflow_* tools run multi-step workflows; search_knowledge searches gai's document collections, which are also listed as resources.",
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(self.tools().await?))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let p = principal(&context);
        self.call(
            &request.name,
            request.arguments.unwrap_or_default(),
            p.as_ref(),
            context.ct.clone(),
        )
        .await
        .map(Into::into)
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult::with_all_items(self.resources().await?))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        self.read(&request.uri).await.map(Into::into)
    }
}

/// The `/mcp` endpoint.
pub fn http_service(
    mcp: GaiMcp,
    settings: &McpServeSettings,
) -> rmcp::transport::streamable_http_server::StreamableHttpService<
    GaiMcp,
    rmcp::transport::streamable_http_server::session::local::LocalSessionManager,
> {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    };
    let mut config = StreamableHttpServerConfig::default();
    if settings.allowed_hosts.iter().any(|h| h == "*") {
        config = config.disable_allowed_hosts();
    } else if !settings.allowed_hosts.is_empty() {
        let mut hosts = config.allowed_hosts.clone();
        hosts.extend(settings.allowed_hosts.iter().cloned());
        config = config.with_allowed_hosts(hosts);
    }
    StreamableHttpService::new(
        move || Ok(mcp.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    )
}

/// Serve MCP over stdin and stdout until the client hangs up.
pub async fn serve_stdio(gai: Arc<Gai>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use rmcp::ServiceExt;
    let rt = gai.runtime().await?.clone();
    let wf = gai.workflows().await?.clone();
    rt.ensure_mcp();
    // Pick up runs a previous process left behind, as `gai serve` does.
    rt.recover().await;
    let running = GaiMcp::new(gai, rt, wf)
        .serve(rmcp::transport::stdio())
        .await?;
    running.waiting().await?;
    Ok(())
}
