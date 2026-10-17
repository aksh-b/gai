//! The OpenAPI 3.1 document, built from a route table and the JSON Schemas
//! schemars derives for the API types. OpenAPI 3.1 uses JSON Schema 2020-12,
//! which is what schemars emits, so types are described once, in Rust.

use axum::Json;
use axum::response::Html;
use gai::agent::{AgentSpec, Decision, EventEnvelope, PendingApproval, RunOutcome};
use gai::llm::CatalogEntry;
use gai::store::{AgentVersion, ApiKey, NewSession, Run, Session, Step, StoredMessage, UsageTotal};
use schemars::generate::SchemaSettings;
use schemars::{JsonSchema, SchemaGenerator};
use serde_json::{Map, Value, json};

use crate::api::*;
use crate::knowledge::*;
use crate::media::*;
use crate::openai::ChatCompletionRequest;
use crate::triggers::*;
use crate::versions::*;
use crate::workflows::*;
use gai::memory::{Ingested, NewCollection, SearchHit};
use gai::orchestra::WorkflowSpec;
use gai::store::{BlockRevision, Collection, Document, MemoryBlock, NewBlock};
use gai::tools::mcp::McpServerInfo;

struct Op {
    method: &'static str,
    path: &'static str,
    summary: &'static str,
    tag: &'static str,
    body: Option<Value>,
    response: Option<Value>,
    /// Responds with Server-Sent Events.
    sse: bool,
    query: &'static [(&'static str, &'static str)],
}

fn op(method: &'static str, path: &'static str, tag: &'static str, summary: &'static str) -> Op {
    Op {
        method,
        path,
        summary,
        tag,
        body: None,
        response: None,
        sse: false,
        query: &[],
    }
}

impl Op {
    fn body(mut self, s: Value) -> Self {
        self.body = Some(s);
        self
    }
    fn ok(mut self, s: Value) -> Self {
        self.response = Some(s);
        self
    }
    fn sse(mut self) -> Self {
        self.sse = true;
        self
    }
    fn query(mut self, q: &'static [(&'static str, &'static str)]) -> Self {
        self.query = q;
        self
    }
}

fn s<T: JsonSchema>(g: &mut SchemaGenerator) -> Value {
    g.subschema_for::<T>().to_value()
}

fn list<T: JsonSchema>(g: &mut SchemaGenerator) -> Value {
    json!({"type": "array", "items": s::<T>(g)})
}

pub fn document() -> Value {
    let mut g = SchemaSettings::draft2020_12()
        .with(|st| st.definitions_path = "#/components/schemas/".into())
        .into_generator();
    let ops = vec![
        op(
            "get",
            "/v1/health",
            "system",
            "Server status and whether auth is open",
        )
        .ok(s::<Health>(&mut g)),
        op("get", "/v1/me", "system", "The calling key").ok(s::<Me>(&mut g)),
        op("get", "/v1/models", "models", "Model catalog").ok(list::<CatalogEntry>(&mut g)),
        op(
            "get",
            "/v1/providers",
            "models",
            "Configured providers and key status",
        )
        .ok(list::<ProviderView>(&mut g)),
        op("get", "/v1/tools", "tools", "Tools agents can use").ok(list::<ToolView>(&mut g)),
        op("get", "/v1/agents", "agents", "Stored agents").ok(list::<AgentEntry>(&mut g)),
        op(
            "post",
            "/v1/agents",
            "agents",
            "Save an agent as a new version",
        )
        .body(s::<AgentSpec>(&mut g))
        .ok(s::<Saved>(&mut g))
        .query(&[("note", "Version note")]),
        op(
            "get",
            "/v1/agents/default",
            "agents",
            "The built-in assistant",
        )
        .ok(s::<AgentSpec>(&mut g)),
        op(
            "get",
            "/v1/agents/{name}",
            "agents",
            "An agent (latest, or ?version=)",
        )
        .ok(s::<AgentView>(&mut g))
        .query(&[("version", "Version number")]),
        op(
            "delete",
            "/v1/agents/{name}",
            "agents",
            "Delete an agent and its versions",
        ),
        op(
            "get",
            "/v1/agents/{name}/versions",
            "agents",
            "An agent's versions",
        )
        .ok(list::<AgentVersion>(&mut g)),
        op(
            "post",
            "/v1/agents/{name}/labels/{label}",
            "agents",
            "Point a label at a version",
        )
        .body(s::<LabelBody>(&mut g))
        .ok(s::<Done>(&mut g)),
        op(
            "delete",
            "/v1/agents/{name}/labels/{label}",
            "agents",
            "Remove a label",
        )
        .ok(s::<Done>(&mut g)),
        op("get", "/v1/sessions", "sessions", "Recent sessions")
            .ok(list::<Session>(&mut g))
            .query(&[("agent", "Filter by agent"), ("limit", "Max rows")]),
        op("post", "/v1/sessions", "sessions", "Create a session")
            .body(s::<NewSession>(&mut g))
            .ok(s::<Session>(&mut g)),
        op("get", "/v1/sessions/{id}", "sessions", "A session").ok(s::<Session>(&mut g)),
        op(
            "delete",
            "/v1/sessions/{id}",
            "sessions",
            "Delete a session and its runs",
        ),
        op(
            "get",
            "/v1/sessions/{id}/messages",
            "sessions",
            "A session's transcript",
        )
        .ok(list::<StoredMessage>(&mut g)),
        op(
            "post",
            "/v1/runs",
            "runs",
            "Start an agent run (mode: wait, stream or background)",
        )
        .body(s::<CreateRun>(&mut g))
        .ok(json!({"oneOf": [s::<RunOutcome>(&mut g), s::<RunIds>(&mut g)]})),
        op("get", "/v1/runs", "runs", "Recent runs")
            .ok(list::<Run>(&mut g))
            .query(&[
                ("session_id", "Filter by session"),
                ("agent", "Filter by agent"),
                ("status", "Filter by status"),
                ("fork_of", "Only forks (replays) of this run"),
                ("limit", "Max rows"),
            ]),
        op("get", "/v1/triggers", "triggers", "Triggers (schedules, webhooks, file watchers)")
            .ok(list::<TriggerView>(&mut g)),
        op("post", "/v1/triggers", "triggers", "Create a trigger")
            .body(s::<gai::triggers::TriggerSpec>(&mut g))
            .ok(s::<CreatedTrigger>(&mut g)),
        op(
            "post",
            "/v1/triggers/preview",
            "triggers",
            "Check an unsaved trigger: errors, the schedule in words, its next times",
        )
        .ok(s::<TriggerPreview>(&mut g)),
        op("get", "/v1/triggers/{id}", "triggers", "A trigger (by id or name)")
            .ok(s::<TriggerView>(&mut g)),
        op("put", "/v1/triggers/{id}", "triggers", "Replace a trigger's definition")
            .body(s::<gai::triggers::TriggerSpec>(&mut g))
            .ok(s::<TriggerView>(&mut g)),
        op("delete", "/v1/triggers/{id}", "triggers", "Delete a trigger"),
        op("post", "/v1/triggers/{id}/enable", "triggers", "Turn a trigger on")
            .ok(s::<TriggerView>(&mut g)),
        op("post", "/v1/triggers/{id}/disable", "triggers", "Turn a trigger off")
            .ok(s::<TriggerView>(&mut g)),
        op(
            "post",
            "/v1/triggers/{id}/fire",
            "triggers",
            "Fire now; the body (any JSON) is the payload",
        )
        .ok(s::<gai::triggers::Fired>(&mut g)),
        op("post", "/v1/triggers/{id}/secret", "triggers", "A webhook trigger's signing secret")
            .ok(s::<TriggerSecret>(&mut g)),
        op("get", "/v1/triggers/{id}/events", "triggers", "What a trigger did, newest first")
            .ok(list::<gai::store::TriggerEvent>(&mut g))
            .query(&[("limit", "Max rows")]),
        op(
            "post",
            "/v1/webhooks/{id}",
            "triggers",
            "Receive a webhook (no API key: sign the body with the trigger's secret, X-Gai-Signature: sha256=<hex>, or send X-Gai-Token)",
        )
        .ok(s::<gai::triggers::Fired>(&mut g)),
        op("get", "/v1/media", "media", "Default media models from [media]")
            .ok(s::<MediaDefaults>(&mut g)),
        op("get", "/v1/blobs", "media", "Stored media, newest first")
            .ok(list::<gai::store::Blob>(&mut g))
            .query(&[("run_id", "Only what this run made"), ("limit", "Max rows")]),
        op(
            "post",
            "/v1/blobs",
            "media",
            "Store the request body (any content-type) as a blob",
        )
        .ok(s::<gai::store::Blob>(&mut g))
        .query(&[("name", "File name")]),
        op("get", "/v1/blobs/{id}", "media", "A blob's bytes"),
        op("get", "/v1/blobs/{id}/info", "media", "A blob's metadata")
            .ok(s::<gai::store::Blob>(&mut g)),
        op("delete", "/v1/blobs/{id}", "media", "Delete a blob"),
        op(
            "post",
            "/v1/audio/transcriptions",
            "media",
            "Speech to text, from a blob or base64 audio",
        )
        .body(s::<TranscribeBody>(&mut g))
        .ok(s::<TranscribeResult>(&mut g)),
        op(
            "post",
            "/v1/audio/speech",
            "media",
            "Text to speech; the audio is stored as a blob",
        )
        .body(s::<SpeakBody>(&mut g))
        .ok(s::<MediaResult>(&mut g)),
        op(
            "post",
            "/v1/images/generations",
            "media",
            "Generate images; they are stored as blobs",
        )
        .body(s::<ImagesBody>(&mut g))
        .ok(s::<MediaResult>(&mut g)),
        op(
            "post",
            "/v1/runs/{id}/replay",
            "runs",
            "Fork a run: replay steps before from_step from the recording, run the rest live (mode: wait, stream or background)",
        )
        .body(s::<ReplayRun>(&mut g))
        .ok(json!({"oneOf": [s::<RunOutcome>(&mut g), s::<RunIds>(&mut g)]})),
        op(
            "get",
            "/v1/agents/{name}/diff",
            "agents",
            "Diff two versions of an agent (default: latest against the one before)",
        )
        .ok(s::<VersionDiff>(&mut g))
        .query(&[("from", "Older version"), ("to", "Newer version")]),
        op(
            "post",
            "/v1/agents/{name}/rollback",
            "agents",
            "Save an old version again as the newest",
        )
        .body(s::<Rollback>(&mut g))
        .ok(s::<Saved>(&mut g)),
        op(
            "get",
            "/v1/workflows/{name}/diff",
            "workflows",
            "Diff two versions of a workflow",
        )
        .ok(s::<VersionDiff>(&mut g))
        .query(&[("from", "Older version"), ("to", "Newer version")]),
        op(
            "post",
            "/v1/workflows/{name}/rollback",
            "workflows",
            "Save an old version again as the newest",
        )
        .body(s::<Rollback>(&mut g))
        .ok(s::<Saved>(&mut g)),
        op(
            "post",
            "/v1/workflows/{name}/labels/{label}",
            "workflows",
            "Point a label at a version",
        )
        .body(s::<LabelBody>(&mut g))
        .ok(s::<Done>(&mut g)),
        op(
            "delete",
            "/v1/workflows/{name}/labels/{label}",
            "workflows",
            "Remove a label",
        )
        .ok(s::<Done>(&mut g)),
        op("get", "/v1/runs/{id}", "runs", "A run").ok(s::<RunView>(&mut g)),
        op("get", "/v1/runs/{id}/steps", "runs", "A run's step journal").ok(list::<Step>(&mut g)),
        op(
            "get",
            "/v1/runs/{id}/trace",
            "runs",
            "A run with its steps and child runs",
        )
        .ok(s::<Trace>(&mut g)),
        op(
            "get",
            "/v1/runs/{id}/events",
            "runs",
            "Stored then live run events",
        )
        .ok(s::<EventEnvelope>(&mut g))
        .sse()
        .query(&[("after", "Only events after this seq")]),
        op("post", "/v1/runs/{id}/cancel", "runs", "Stop a run").ok(s::<Done>(&mut g)),
        op(
            "post",
            "/v1/runs/{id}/approvals/{call_id}",
            "runs",
            "Approve or decline a tool call",
        )
        .body(s::<Decision>(&mut g))
        .ok(s::<Done>(&mut g)),
        op(
            "get",
            "/v1/approvals",
            "runs",
            "Tool calls waiting for approval",
        )
        .ok(list::<PendingApproval>(&mut g)),
        op("get", "/v1/usage", "usage", "Tokens and cost by model")
            .ok(list::<UsageTotal>(&mut g))
            .query(&[("since", "RFC 3339 time")]),
        op("get", "/v1/keys", "keys", "API keys").ok(list::<ApiKey>(&mut g)),
        op(
            "post",
            "/v1/runs/{id}/resume",
            "runs",
            "Resume an unfinished run",
        )
        .ok(s::<RunIds>(&mut g)),
        op(
            "get",
            "/v1/mcp",
            "mcp",
            "MCP servers, their status and tools",
        )
        .ok(list::<McpServerInfo>(&mut g)),
        op(
            "post",
            "/v1/mcp",
            "mcp",
            "Add or replace an MCP server and connect it",
        )
        .body(s::<AddMcpServer>(&mut g))
        .ok(s::<McpServerInfo>(&mut g)),
        op(
            "delete",
            "/v1/mcp/{name}",
            "mcp",
            "Remove an MCP server added through the API",
        ),
        op(
            "post",
            "/v1/mcp/{name}/reconnect",
            "mcp",
            "Reconnect an MCP server",
        )
        .ok(s::<McpServerInfo>(&mut g)),
        op(
            "post",
            "/v1/mcp/{name}/sign-in",
            "mcp",
            "Start an OAuth sign-in; open the returned URL in a browser",
        )
        .body(s::<SignInRequest>(&mut g))
        .ok(s::<SignInStarted>(&mut g)),
        op(
            "delete",
            "/v1/mcp/{name}/sign-in",
            "mcp",
            "Forget a server's sign-in",
        )
        .ok(s::<McpServerInfo>(&mut g)),
        op(
            "get",
            "/v1/mcp/oauth/callback",
            "mcp",
            "OAuth redirect target (browser only)",
        ),
        op("get", "/v1/hooks", "system", "Active hooks")
            .ok(json!({"type": "array", "items": {"type": "string"}})),
        op(
            "post",
            "/v1/keys",
            "keys",
            "Create an API key (the secret is returned once)",
        )
        .body(s::<CreateKey>(&mut g))
        .ok(s::<CreatedKey>(&mut g)),
        op("delete", "/v1/keys/{id}", "keys", "Revoke an API key"),
        op(
            "get",
            "/v1/collections",
            "knowledge",
            "Knowledge collections",
        )
        .ok(list::<Collection>(&mut g))
        .query(&[("all", "Include archival memory collections (true/false)")]),
        op(
            "post",
            "/v1/collections",
            "knowledge",
            "Create a collection",
        )
        .body(s::<NewCollection>(&mut g))
        .ok(s::<Collection>(&mut g)),
        op("get", "/v1/collections/{name}", "knowledge", "A collection")
            .ok(s::<Collection>(&mut g)),
        op(
            "delete",
            "/v1/collections/{name}",
            "knowledge",
            "Delete a collection and its documents",
        ),
        op(
            "get",
            "/v1/collections/{name}/documents",
            "knowledge",
            "Documents in a collection",
        )
        .ok(list::<Document>(&mut g))
        .query(&[("limit", "Max rows"), ("offset", "Rows to skip")]),
        op(
            "post",
            "/v1/collections/{name}/documents",
            "knowledge",
            "Add a document from text, a base64 file or a URL",
        )
        .body(s::<AddDocument>(&mut g))
        .ok(s::<Ingested>(&mut g)),
        op(
            "get",
            "/v1/collections/{name}/documents/{id}",
            "knowledge",
            "A document and its chunks",
        )
        .ok(s::<DocumentView>(&mut g)),
        op(
            "delete",
            "/v1/collections/{name}/documents/{id}",
            "knowledge",
            "Delete a document",
        ),
        op(
            "post",
            "/v1/collections/{name}/search",
            "knowledge",
            "Search a collection (hybrid, vector or text)",
        )
        .body(s::<SearchRequest>(&mut g))
        .ok(list::<SearchHit>(&mut g)),
        op("get", "/v1/memory/blocks", "memory", "Memory blocks")
            .ok(list::<MemoryBlock>(&mut g))
            .query(&[("scope", "agent:<name>, user:<id> or shared:<name>")]),
        op(
            "post",
            "/v1/memory/blocks",
            "memory",
            "Create a memory block",
        )
        .body(s::<NewBlock>(&mut g))
        .ok(s::<MemoryBlock>(&mut g)),
        op("get", "/v1/memory/blocks/{id}", "memory", "A memory block")
            .ok(s::<MemoryBlock>(&mut g)),
        op(
            "patch",
            "/v1/memory/blocks/{id}",
            "memory",
            "Edit a block's value or settings",
        )
        .body(s::<EditBlock>(&mut g))
        .ok(s::<MemoryBlock>(&mut g)),
        op(
            "delete",
            "/v1/memory/blocks/{id}",
            "memory",
            "Delete a memory block",
        ),
        op(
            "get",
            "/v1/memory/blocks/{id}/history",
            "memory",
            "Every value a block has had, newest first",
        )
        .ok(list::<BlockRevision>(&mut g)),
        op(
            "get",
            "/v1/workflows",
            "workflows",
            "Stored workflows and workflow files",
        )
        .ok(list::<WorkflowEntry>(&mut g)),
        op(
            "post",
            "/v1/workflows",
            "workflows",
            "Save a workflow as a new version",
        )
        .body(s::<WorkflowSpec>(&mut g))
        .ok(s::<Saved>(&mut g))
        .query(&[("note", "Version note")]),
        op(
            "post",
            "/v1/workflows/validate",
            "workflows",
            "Check a workflow and return its graph",
        )
        .body(s::<WorkflowSpec>(&mut g))
        .ok(s::<Validation>(&mut g)),
        op(
            "post",
            "/v1/workflows/run",
            "workflows",
            "Run an inline workflow (mode: wait, stream or background)",
        )
        .body(s::<RunInlineWorkflow>(&mut g))
        .ok(json!({"oneOf": [s::<RunOutcome>(&mut g), s::<RunIds>(&mut g)]})),
        op(
            "get",
            "/v1/workflows/{name}",
            "workflows",
            "A workflow (stored latest, ?version=, or file) with its graph",
        )
        .ok(s::<WorkflowView>(&mut g))
        .query(&[("version", "Version number")]),
        op(
            "delete",
            "/v1/workflows/{name}",
            "workflows",
            "Delete a stored workflow and its versions",
        ),
        op(
            "get",
            "/v1/workflows/{name}/versions",
            "workflows",
            "A workflow's versions",
        )
        .ok(list::<gai::store::WorkflowVersion>(&mut g)),
        op(
            "post",
            "/v1/workflows/{name}/runs",
            "workflows",
            "Run a workflow (mode: wait, stream or background)",
        )
        .body(s::<RunWorkflow>(&mut g))
        .ok(json!({"oneOf": [s::<RunOutcome>(&mut g), s::<RunIds>(&mut g)]})),
        op("get", "/v1/skills", "skills", "Agent Skills").ok(list::<SkillView>(&mut g)),
        op(
            "post",
            "/v1/skills",
            "skills",
            "Upload a skill (SKILL.md plus text files)",
        )
        .body(s::<SaveSkill>(&mut g))
        .ok(s::<SavedSkill>(&mut g)),
        op(
            "get",
            "/v1/skills/{name}",
            "skills",
            "A skill with its instructions",
        )
        .ok(s::<SkillDetail>(&mut g)),
        op(
            "delete",
            "/v1/skills/{name}",
            "skills",
            "Delete an uploaded skill",
        ),
        op(
            "post",
            "/openai/v1/chat/completions",
            "openai",
            "OpenAI-compatible chat completions (`provider/model` or `agent:<name>`)",
        )
        .body(s::<ChatCompletionRequest>(&mut g))
        .ok(json!({"type": "object"})),
        op(
            "get",
            "/openai/v1/models",
            "openai",
            "OpenAI-compatible model list",
        )
        .ok(json!({"type": "object"})),
    ];

    let mut paths = Map::new();
    for o in ops {
        let mut params: Vec<Value> = o
            .path
            .split('/')
            .filter_map(|seg| seg.strip_prefix('{')?.strip_suffix('}'))
            .map(|name| json!({"name": name, "in": "path", "required": true, "schema": {"type": "string"}}))
            .collect();
        params.extend(o.query.iter().map(|(name, d)| json!({"name": name, "in": "query", "description": d, "schema": {"type": "string"}})));
        let content_type = if o.sse {
            "text/event-stream"
        } else {
            "application/json"
        };
        let mut operation = json!({
            "summary": o.summary,
            "tags": [o.tag],
            "operationId": format!("{}_{}", o.method, o.path.trim_start_matches('/').replace(['/', '{', '}'], "_")),
            "parameters": params,
            "responses": {
                "200": match o.response {
                    Some(schema) => json!({"description": "OK", "content": {content_type: {"schema": schema}}}),
                    None => json!({"description": "Done"}),
                },
                "default": {"description": "Error", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Error"}}}}
            }
        });
        if let Some(body) = o.body {
            operation["requestBody"] =
                json!({"required": true, "content": {"application/json": {"schema": body}}});
        }
        paths.entry(o.path.to_owned()).or_insert_with(|| json!({}))[o.method] = operation;
    }
    let mut schemas = g.take_definitions(true);
    schemas.insert(
        "Error".into(),
        json!({"type": "object", "properties": {"error": {"type": "object", "properties": {"message": {"type": "string"}}, "required": ["message"]}}}),
    );
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "gai",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "The gai agent platform API. Authenticate with `Authorization: Bearer <api key>`."
        },
        "servers": [{"url": "/"}],
        "security": [{"apiKey": []}],
        "paths": paths,
        "components": {
            "schemas": schemas,
            "securitySchemes": {"apiKey": {"type": "http", "scheme": "bearer"}}
        }
    })
}

pub async fn spec() -> Json<Value> {
    Json(document())
}

pub async fn docs() -> Html<&'static str> {
    Html(
        r#"<!doctype html>
<html><head><meta charset="utf-8"><title>gai API</title>
<meta name="viewport" content="width=device-width, initial-scale=1"></head>
<body><script id="api-reference" data-url="/openapi.json"></script>
<script src="https://cdn.jsdelivr.net/npm/@scalar/api-reference@1.25.0"></script></body></html>"#,
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn document_resolves_every_ref() {
        let doc = super::document();
        let text = doc.to_string();
        let schemas = doc["components"]["schemas"].as_object().unwrap();
        for part in text.split("\"$ref\":\"#/components/schemas/").skip(1) {
            let name = &part[..part.find('"').unwrap()];
            assert!(schemas.contains_key(name), "dangling $ref {name}");
        }
        assert!(doc["paths"]["/v1/runs"]["post"]["requestBody"].is_object());
        assert!(schemas.contains_key("AgentSpec"));
    }
}
