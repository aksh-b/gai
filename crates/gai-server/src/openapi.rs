//! The OpenAPI 3.1 document, built from a route table and the JSON Schemas
//! schemars derives for the API types. OpenAPI 3.1 uses JSON Schema 2020-12,
//! which is what schemars emits, so types are described once, in Rust.

use axum::Json;
use axum::response::Html;
use gai::agent::{AgentSpec, Decision, EventEnvelope, PendingApproval, PersonaSpec, RunOutcome};
use gai::llm::CatalogEntry;
use gai::store::{
    AgentVersion, ApiKey, NewSession, PersonaVersion, Run, Session, Step, StoredMessage, UsageTotal,
};
use schemars::generate::SchemaSettings;
use schemars::{JsonSchema, SchemaGenerator};
use serde_json::{Map, Value, json};

use crate::api::*;
use crate::apps::*;
use crate::data::*;
use crate::evals::*;
use crate::governance::*;
use crate::knowledge::*;
use crate::marketplace::*;
use crate::media::*;
use crate::openai::ChatCompletionRequest;
use crate::optimizer::*;
use crate::orgs::*;
use crate::personas::*;
use crate::prompts::*;
use crate::recordings::*;
use crate::setup::*;
use crate::sso::AuthConfig;
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
        op("post", "/v1/collections/{name}/extract", "knowledge", "Read the same fields out of every document (or the ones named) into a table, with the default model or `model`; ?format=csv for a download")
            .body(s::<ExtractRequest>(&mut g))
            .ok(s::<Extracted>(&mut g))
            .query(&[("format", "csv for a download")]),
        op("get", "/v1/collections/{name}/graph", "knowledge", "The collection's knowledge graph: its most mentioned entities and the relations among them")
            .ok(s::<GraphView>(&mut g))
            .query(&[("q", "Entities whose name contains this"), ("limit", "Max entities (80)")]),
        op("post", "/v1/collections/{name}/graph", "knowledge", "Read the collection's unread chunks into its graph with a model: 202 in the background, ?wait=true for what was found")
            .body(s::<BuildGraph>(&mut g))
            .ok(s::<gai::memory::graph::GraphBuild>(&mut g))
            .query(&[("wait", "Answer when done")]),
        op("delete", "/v1/collections/{name}/graph", "knowledge", "Forget the collection's graph (the next build reads everything again)"),
        op("get", "/v1/collections/{name}/graph/nodes/{id}", "knowledge", "One entity: its relations and the passages that mention it")
            .ok(s::<GraphNodeView>(&mut g)),
        op("get", "/v1/connections", "data", "Database connections agents can query")
            .ok(list::<gai::store::DataConnection>(&mut g)),
        op("post", "/v1/connections", "data", "Save a connection (JSON, or {\"yaml\": \"...\"}): postgres, mysql, sqlite, bigquery or snowflake; passwords and tokens by environment variable or secret name. Reads its schema right away")
            .body(s::<gai::data::ConnectionSpec>(&mut g))
            .ok(s::<gai::store::DataConnection>(&mut g)),
        op("get", "/v1/connections/{name}", "data", "A connection, with its last schema").ok(s::<gai::store::DataConnection>(&mut g)),
        op("delete", "/v1/connections/{name}", "data", "Delete a connection").ok(s::<Done>(&mut g)),
        op("get", "/v1/connections/{name}/schema", "data", "Its tables and columns (kept from the last read; ?refresh=true reads them again)")
            .ok(s::<gai::data::Schema>(&mut g))
            .query(&[("refresh", "Read the schema from the database again")]),
        op("post", "/v1/connections/{name}/query", "data", "Run one read-only query (SELECT), or check it with dry_run")
            .body(s::<RunQuery>(&mut g))
            .ok(s::<QueryAnswer>(&mut g)),
        op("get", "/v1/dashboards", "data", "Dashboards: charts with their queries, and their last results")
            .ok(list::<gai::data::dashboards::DashboardView>(&mut g)),
        op("post", "/v1/dashboards", "data", "Save a dashboard (JSON, or {\"yaml\": \"...\"}) and refresh it; refresh_minutes keeps it up to date, panel alerts watch KPIs")
            .body(s::<gai::data::dashboards::DashboardSpec>(&mut g))
            .ok(s::<gai::data::dashboards::DashboardView>(&mut g)),
        op("get", "/v1/dashboards/{name}", "data", "A dashboard and its last results").ok(s::<gai::data::dashboards::DashboardView>(&mut g)),
        op("delete", "/v1/dashboards/{name}", "data", "Delete a dashboard").ok(s::<Done>(&mut g)),
        op("post", "/v1/dashboards/{name}/refresh", "data", "Run its queries now (and send alerts that start firing)")
            .ok(s::<gai::data::dashboards::DashboardResults>(&mut g)),
        op("get", "/v1/sessions/search", "sessions", "Find messages across conversations, best first, with the words marked [like this]")
            .ok(list::<gai::store::MessageHit>(&mut g))
            .query(&[("q", "Words to find"), ("agent", "One agent"), ("user", "One user id"), ("since", "RFC 3339 time"), ("until", "RFC 3339 time"), ("limit", "Max hits")]),
        op("get", "/v1/analytics/agents", "runs", "Per agent version: runs, success rate, steps, p50/p95 latency, cost, tokens and ratings (evaluations not counted)")
            .ok(list::<gai::store::AgentStats>(&mut g))
            .query(&[("agent", "One agent"), ("since", "RFC 3339 time"), ("until", "RFC 3339 time")]),
        op("get", "/v1/changes", "governance", "Who changed one thing and when (its audit entries, without details)")
            .ok(list::<Change>(&mut g))
            .query(&[("target", "e.g. agents/helper"), ("limit", "Max rows")]),
        op("get", "/v1/audit", "governance", "Changes made through the API, newest first: who, what, when and the answer's status")
            .ok(list::<gai::store::AuditEntry>(&mut g))
            .query(&[("actor", "API key name"), ("target", "Targets starting with this, e.g. agents/"), ("since", "RFC 3339 time"), ("until", "RFC 3339 time"), ("limit", "Max rows"), ("format", "csv for a download")]),
        op("delete", "/v1/users/{id}", "governance", "Delete everything kept about a user: conversations, runs, files, memory blocks, archival memory and ratings")
            .ok(s::<gai::store::Deleted>(&mut g)),
        op("post", "/v1/retention/sweep", "governance", "Delete conversations older than each agent's safety.retention_days now (gai serve also does this hourly)")
            .ok(s::<Swept>(&mut g)),
        op("get", "/v1/sources", "knowledge", "Sources synced into knowledge collections, with their last sync")
            .ok(list::<gai::store::SourceRecord>(&mut g)),
        op("post", "/v1/sources", "knowledge", "Save a source (JSON, or {\"yaml\": \"...\"}): a folder, website, GitHub repository, Confluence space or Notion workspace")
            .body(s::<gai::memory::sources::SourceSpec>(&mut g))
            .ok(s::<gai::store::SourceRecord>(&mut g)),
        op("get", "/v1/sources/{name}", "knowledge", "A source and its last sync")
            .ok(s::<gai::store::SourceRecord>(&mut g)),
        op("delete", "/v1/sources/{name}", "knowledge", "Delete a source (its documents stay in the collection)"),
        op("post", "/v1/sources/{name}/sync", "knowledge", "Sync now: only new, changed and removed items. 202 in the background, or ?wait=true for the counts (gai serve also syncs sources with every_minutes)")
            .ok(s::<gai::memory::sources::SyncStats>(&mut g))
            .query(&[("wait", "Answer when done, with the counts")]),
        op("post", "/v1/runs/{id}/feedback", "feedback", "Rate a reply: 1 good, -1 bad, with an optional comment and tags. Rating again replaces your rating")
            .body(s::<Rate>(&mut g))
            .ok(s::<gai::store::Feedback>(&mut g)),
        op("get", "/v1/runs/{id}/feedback", "feedback", "Ratings of one run")
            .ok(list::<gai::store::Feedback>(&mut g)),
        op("delete", "/v1/runs/{id}/feedback", "feedback", "Take back your rating of a run"),
        op("get", "/v1/feedback", "feedback", "Ratings, newest first")
            .ok(list::<gai::store::Feedback>(&mut g))
            .query(&[("agent", "One agent"), ("rating", "1 or -1"), ("limit", "Max rows")]),
        op("get", "/v1/feedback/summary", "feedback", "Good and bad counts per agent version")
            .ok(list::<gai::store::FeedbackSummary>(&mut g))
            .query(&[("agent", "One agent")]),
        op("get", "/v1/evals", "evals", "Eval sets")
            .ok(list::<gai::store::EvalSetSummary>(&mut g)),
        op("post", "/v1/evals", "evals", "Save an eval set (JSON, or {\"yaml\": \"...\"}) as a new version")
            .body(s::<gai::eval::EvalSet>(&mut g))
            .ok(s::<Saved>(&mut g))
            .query(&[("note", "Why it changed")]),
        op("get", "/v1/evals/{name}", "evals", "An eval set (name or name@v2)")
            .ok(s::<EvalSetView>(&mut g)),
        op("delete", "/v1/evals/{name}", "evals", "Delete an eval set and its versions"),
        op("get", "/v1/evals/{name}/versions", "evals", "Every version of an eval set")
            .ok(list::<gai::store::EvalSetVersion>(&mut g)),
        op("post", "/v1/evals/{name}/runs", "evals", "Evaluate an agent (and optionally several models) on the set, in the background")
            .body(s::<StartEval>(&mut g))
            .ok(s::<StartedEvals>(&mut g)),
        op("get", "/v1/evals/{name}/runs", "evals", "Evaluations of one set, newest first")
            .ok(list::<gai::store::EvalRun>(&mut g)),
        op("get", "/v1/eval-runs", "evals", "Evaluations, newest first")
            .ok(list::<gai::store::EvalRun>(&mut g))
            .query(&[("set", "One eval set"), ("limit", "Max rows")]),
        op("get", "/v1/eval-runs/{id}", "evals", "An evaluation with its per-case results (partial while it runs)")
            .ok(s::<gai::eval::EvalReport>(&mut g)),
        op("delete", "/v1/eval-runs/{id}", "evals", "Delete an evaluation"),
        op("get", "/v1/eval-runs/compare", "evals", "Two evaluations case by case, with a pass/fail verdict")
            .ok(s::<CompareView>(&mut g))
            .query(&[("baseline", "Evaluation id"), ("candidate", "Evaluation id"), ("max_drop", "Largest score drop that passes (0.05)")]),
        op(
            "post",
            "/v1/bundles/export",
            "bundles",
            "Write a .gaibundle (zip) with the chosen agents, workflows, personas, skills, MCP servers and knowledge, plus what they use",
        )
        .body(s::<gai::bundle::ExportOptions>(&mut g)),
        op(
            "post",
            "/v1/bundles/inspect",
            "bundles",
            "Read a .gaibundle (the request body) without importing it: its contents and what this server is missing",
        )
        .ok(s::<gai::bundle::ImportReport>(&mut g)),
        op(
            "post",
            "/v1/bundles/import",
            "bundles",
            "Import a .gaibundle (the request body). Existing items get a new version unless skip_existing",
        )
        .ok(s::<gai::bundle::ImportReport>(&mut g))
        .query(&[("skip_existing", "Leave items that already exist alone")]),
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
        op("get", "/v1/recordings", "media", "Recordings (meetings, calls, videos), newest first, without their full results")
            .ok(list::<gai::store::Recording>(&mut g))
            .query(&[("limit", "Max rows")]),
        op("post", "/v1/recordings", "media", "Process an uploaded recording in the background: a meeting gets speakers, notes, decisions and action items; audio speakers, language and sentiment; a video a summary in chapters. Poll GET /v1/recordings/{id}")
            .body(s::<NewRecordingBody>(&mut g))
            .ok(s::<gai::store::Recording>(&mut g)),
        op("get", "/v1/recordings/templates", "media", "The built-in meeting notes templates")
            .ok(list::<NotesTemplateInfo>(&mut g)),
        op("get", "/v1/recordings/{id}", "media", "A recording and, once done, its result (a RecordingResult)")
            .ok(s::<gai::store::Recording>(&mut g)),
        op("delete", "/v1/recordings/{id}", "media", "Delete a recording (its blob stays)"),
        op("get", "/v1/voice", "media", "A voice chat over a WebSocket: send {\"type\": \"start\", \"agent\": ...}, then audio frames and {\"type\": \"commit\"} per utterance, or {\"type\": \"text\"}; get transcript, delta, audio (per sentence) and reply_done back; {\"type\": \"interrupt\"} stops a reply. Agents with media.realtime are relayed to the provider's realtime session (24 kHz PCM16 in and out)"),
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
        op("get", "/v1/auth/config", "users", "How people sign in: OIDC, a proxy, or keys only")
            .ok(s::<AuthConfig>(&mut g)),
        op("get", "/v1/auth/login", "users", "Start an OIDC sign-in (redirects to the identity provider)")
            .query(&[("return_to", "A path on this server to land on afterwards")]),
        op("get", "/v1/auth/callback", "users", "Where the identity provider sends the browser back; sets the gai_session cookie"),
        op("post", "/v1/auth/logout", "users", "Sign this browser out")
            .ok(s::<Done>(&mut g)),
        op("get", "/v1/orgs", "users", "Organizations you work in (server admins: all of them)")
            .ok(list::<OrgView>(&mut g)),
        op("post", "/v1/orgs", "users", "Create an organization, with a database and files of its own (admins of the default organization)")
            .body(s::<NewOrg>(&mut g))
            .ok(s::<gai::store::Org>(&mut g)),
        op("get", "/v1/orgs/{org}", "users", "An organization").ok(s::<gai::store::Org>(&mut g)),
        op("patch", "/v1/orgs/{org}", "users", "Rename an organization or change its branding (owners); limits are set by server admins")
            .body(s::<OrgPatch>(&mut g))
            .ok(s::<gai::store::Org>(&mut g)),
        op("delete", "/v1/orgs/{org}", "users", "Delete an organization's members, teams and keys; its data stays on disk")
            .ok(s::<Done>(&mut g)),
        op("get", "/v1/members", "users", "Members of this organization and their roles")
            .ok(list::<gai::store::Membership>(&mut g)),
        op("put", "/v1/members/{who}", "users", "Add someone by email (they join at their first sign-in) or change their role")
            .body(s::<SetMember>(&mut g))
            .ok(s::<gai::store::Membership>(&mut g)),
        op("delete", "/v1/members/{who}", "users", "Take someone out of this organization").ok(s::<Done>(&mut g)),
        op("get", "/v1/teams", "users", "Teams and their members").ok(list::<gai::store::Team>(&mut g)),
        op("post", "/v1/teams", "users", "Create a team")
            .body(s::<NewTeam>(&mut g))
            .ok(s::<gai::store::Team>(&mut g)),
        op("delete", "/v1/teams/{team}", "users", "Delete a team: what it owned goes back to the organization, its keys are revoked")
            .ok(s::<Done>(&mut g)),
        op("put", "/v1/teams/{team}/members/{who}", "users", "Put a member in a team").ok(s::<gai::store::Team>(&mut g)),
        op("delete", "/v1/teams/{team}/members/{who}", "users", "Take a member out of a team").ok(s::<gai::store::Team>(&mut g)),
        op("get", "/v1/owners", "users", "Which team owns which agent, workflow, collection and so on")
            .ok(list::<Owner>(&mut g)),
        op("put", "/v1/owners/{kind}/{name}", "users", "Give a resource to a team (only its builders change it), or back to everyone with team: null")
            .body(s::<SetOwner>(&mut g)),
        op("get", "/v1/secrets", "users", "Names of this organization's secrets (sources read them by name)")
            .ok(list::<String>(&mut g)),
        op("put", "/v1/secrets/{name}", "users", "Save a secret").body(s::<SecretValue>(&mut g)).ok(s::<Done>(&mut g)),
        op("delete", "/v1/secrets/{name}", "users", "Delete a secret").ok(s::<Done>(&mut g)),
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
        op("get", "/v1/apps", "apps", "Apps: agents and workflows published as a form and an endpoint")
            .ok(list::<AppView>(&mut g)),
        op("post", "/v1/apps", "apps", "Create or replace an app")
            .body(s::<AppSpec>(&mut g))
            .ok(s::<AppView>(&mut g)),
        op("get", "/v1/apps/{name}", "apps", "An app").ok(s::<AppView>(&mut g)),
        op("delete", "/v1/apps/{name}", "apps", "Delete an app (its runs are kept)"),
        op("post", "/v1/apps/{name}/key", "apps", "Make the app's own key, which runs this app and nothing else; replaces the old one. Shown once")
            .ok(s::<AppKey>(&mut g)),
        op("delete", "/v1/apps/{name}/key", "apps", "Remove the app's key"),
        op("get", "/v1/apps/{name}/form", "apps", "What the app's page shows: title, fields and button. Takes the app's key, or none for a public app")
            .ok(s::<AppForm>(&mut g)),
        op("get", "/v1/apps/{name}/openapi.json", "apps", "An OpenAPI document for this app's endpoint alone, with its input and output schemas"),
        op("post", "/v1/apps/{name}/run", "apps", "Run the app: the body is its inputs as a JSON object. mode=background answers at once with a run id")
            .query(&[("mode", "wait (default) or background")])
            .body(json!({"type": "object"}))
            .ok(json!({"oneOf": [s::<AppResult>(&mut g), s::<RunIds>(&mut g)]})),
        op("get", "/v1/apps/{name}/runs/{id}", "apps", "A run started through the app, with its result once done")
            .ok(s::<AppResult>(&mut g)),
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
        op(
            "get",
            "/v1/setup/status",
            "setup",
            "What first-run setup still needs (public)",
        )
        .ok(s::<SetupStatus>(&mut g)),
        op(
            "post",
            "/v1/setup/claim",
            "setup",
            "Get an admin key for the dashboard on this computer (local servers only)",
        )
        .ok(s::<Claimed>(&mut g)),
        op("get", "/v1/settings", "setup", "Runtime settings").ok(s::<SettingsView>(&mut g)),
        op("put", "/v1/settings", "setup", "Change runtime settings")
            .body(s::<Settings>(&mut g))
            .ok(s::<SettingsView>(&mut g)),
        op(
            "put",
            "/v1/providers/{name}/key",
            "setup",
            "Save an API key for a provider",
        )
        .body(s::<ProviderKey>(&mut g))
        .ok(s::<ProviderKeyView>(&mut g)),
        op(
            "delete",
            "/v1/providers/{name}/key",
            "setup",
            "Forget a provider key saved from the dashboard",
        )
        .ok(s::<ProviderKeyView>(&mut g)),
        op(
            "post",
            "/v1/providers/{name}/test",
            "setup",
            "Try a provider with one tiny request",
        )
        .body(s::<TestRequest>(&mut g))
        .ok(s::<TestResult>(&mut g)),
        op(
            "get",
            "/v1/providers/ollama/library",
            "setup",
            "Recommended Ollama models and what is installed",
        )
        .ok(s::<OllamaLibrary>(&mut g)),
        op(
            "post",
            "/v1/providers/ollama/pull",
            "setup",
            "Start downloading a model into Ollama",
        )
        .body(s::<PullRequest>(&mut g))
        .ok(s::<PullProgress>(&mut g)),
        op(
            "get",
            "/v1/providers/ollama/pulls",
            "setup",
            "Progress of Ollama downloads",
        )
        .ok(list::<PullProgress>(&mut g)),
        op("get", "/v1/templates", "setup", "Starter agents and workflows")
            .ok(list::<TemplateView>(&mut g)),
        op(
            "post",
            "/v1/templates/{id}/install",
            "setup",
            "Save a template (and the agents it uses)",
        )
        .ok(s::<Installed>(&mut g)),
        op("get", "/v1/personas", "personas", "Stored personas").ok(list::<PersonaEntry>(&mut g)),
        op(
            "post",
            "/v1/personas",
            "personas",
            "Save a persona as a new version",
        )
        .body(s::<PersonaSpec>(&mut g))
        .ok(s::<Saved>(&mut g))
        .query(&[("note", "Version note")]),
        op(
            "post",
            "/v1/personas/render",
            "personas",
            "A persona as it goes into the system prompt",
        )
        .body(s::<PersonaSpec>(&mut g))
        .ok(s::<RenderedPersona>(&mut g)),
        op(
            "get",
            "/v1/personas/defaults",
            "personas",
            "The persona runs use when their agent names none",
        )
        .ok(s::<PersonaDefaultsView>(&mut g)),
        op(
            "put",
            "/v1/personas/defaults",
            "personas",
            "Choose the default persona and where it goes in the prompt",
        )
        .body(s::<PutPersonaDefaults>(&mut g))
        .ok(s::<PersonaDefaultsView>(&mut g)),
        op(
            "get",
            "/v1/personas/{name}",
            "personas",
            "A persona (latest, or ?version=)",
        )
        .ok(s::<PersonaView>(&mut g))
        .query(&[("version", "Version number")]),
        op(
            "delete",
            "/v1/personas/{name}",
            "personas",
            "Delete a persona that nothing uses",
        ),
        op(
            "get",
            "/v1/personas/{name}/versions",
            "personas",
            "A persona's versions",
        )
        .ok(list::<PersonaVersion>(&mut g)),
        op(
            "get",
            "/v1/personas/{name}/usage",
            "personas",
            "Agents that speak as this persona by default",
        )
        .ok(json!({"type": "array", "items": {"type": "string"}})),
        op(
            "get",
            "/v1/personas/{name}/diff",
            "personas",
            "Diff two versions of a persona",
        )
        .ok(s::<VersionDiff>(&mut g))
        .query(&[("from", "Older version"), ("to", "Newer version")]),
        op(
            "post",
            "/v1/personas/{name}/rollback",
            "personas",
            "Save an old version again as the newest",
        )
        .body(s::<Rollback>(&mut g))
        .ok(s::<Saved>(&mut g)),
        op(
            "post",
            "/v1/personas/{name}/labels/{label}",
            "personas",
            "Point a label at a version",
        )
        .body(s::<LabelBody>(&mut g))
        .ok(s::<Done>(&mut g)),
        op(
            "delete",
            "/v1/personas/{name}/labels/{label}",
            "personas",
            "Remove a label",
        )
        .ok(s::<Done>(&mut g)),
        op("get", "/v1/prompts", "prompts", "Prompt templates, with the agents that use them")
            .ok(list::<PromptEntry>(&mut g)),
        op("post", "/v1/prompts", "prompts", "Save a prompt template as its next version")
            .body(s::<gai::agent::PromptSpec>(&mut g))
            .ok(s::<Saved>(&mut g))
            .query(&[("note", "Version note")]),
        op("post", "/v1/prompts/render", "prompts", "A prompt's text with these variables (and its defaults)")
            .body(s::<RenderPrompt>(&mut g))
            .ok(s::<RenderedPrompt>(&mut g)),
        op("get", "/v1/prompts/{name}", "prompts", "A prompt template (latest, or ?version=)")
            .ok(s::<PromptView>(&mut g))
            .query(&[("version", "Version number")]),
        op("delete", "/v1/prompts/{name}", "prompts", "Delete a prompt template no agent uses"),
        op("get", "/v1/prompts/{name}/versions", "prompts", "A prompt's versions, newest first")
            .ok(list::<gai::store::LibraryVersion>(&mut g)),
        op("get", "/v1/prompts/{name}/diff", "prompts", "What changed between two versions")
            .ok(s::<VersionDiff>(&mut g))
            .query(&[("from", "Version (default: the one before to)"), ("to", "Version (default: latest)")]),
        op("post", "/v1/prompts/{name}/rollback", "prompts", "Save an old version as the newest")
            .body(s::<Rollback>(&mut g))
            .ok(s::<Saved>(&mut g)),
        op("post", "/v1/prompts/{name}/labels/{label}", "prompts", "Point a label at a version (agents use name@label)")
            .body(s::<LabelBody>(&mut g))
            .ok(s::<Done>(&mut g)),
        op("delete", "/v1/prompts/{name}/labels/{label}", "prompts", "Remove a label")
            .ok(s::<Done>(&mut g)),
        op("post", "/v1/evals/{name}/optimize", "evals", "Rewrite the agent's instructions (or its prompt template) to raise its score on this set, in the background; follow GET /v1/optimizations/{id}")
            .body(s::<StartOptimize>(&mut g))
            .ok(s::<Started>(&mut g)),
        op("get", "/v1/optimizations", "evals", "Prompt optimizations, newest first")
            .ok(list::<Optimization>(&mut g))
            .query(&[("limit", "Max rows")]),
        op("get", "/v1/optimizations/{id}", "evals", "An optimization and what it found so far")
            .ok(s::<Optimization>(&mut g)),
        op("post", "/v1/optimizations/{id}/apply", "evals", "Save the best instructions as the agent's (or prompt template's) next version")
            .body(s::<Apply>(&mut g))
            .ok(s::<Saved>(&mut g)),
        op("get", "/v1/marketplace/listings", "marketplace", "What this server publishes (other servers browse it as a registry)")
            .ok(list::<Listing>(&mut g)),
        op("post", "/v1/marketplace/listings", "marketplace", "Export a bundle from this server and publish it as a listing's next version")
            .body(s::<Publish>(&mut g))
            .ok(s::<Listing>(&mut g)),
        op("get", "/v1/marketplace/listings/{name}", "marketplace", "A listing with its versions and reviews")
            .ok(s::<ListingDetail>(&mut g)),
        op("delete", "/v1/marketplace/listings/{name}", "marketplace", "Unpublish a listing and its bundles"),
        op("get", "/v1/marketplace/listings/{name}/bundle", "marketplace", "A listing's bundle (a zip; latest, or ?version=)")
            .query(&[("version", "Version number")]),
        op("post", "/v1/marketplace/listings/{name}/ratings", "marketplace", "Rate a listing 1 to 5; rating again replaces your rating")
            .body(s::<RateListing>(&mut g))
            .ok(s::<Done>(&mut g)),
        op("get", "/v1/marketplace/registries", "marketplace", "Other registries (gai servers) to browse and install from")
            .ok(list::<RegistryView>(&mut g)),
        op("put", "/v1/marketplace/registries", "marketplace", "Replace the list of registries; one sent without a token keeps its saved token")
            .body(json!({"type": "array", "items": s::<Registry>(&mut g)}))
            .ok(list::<RegistryView>(&mut g)),
        op("get", "/v1/marketplace/browse", "marketplace", "Listings of this server or of a registry")
            .ok(list::<Listing>(&mut g))
            .query(&[("registry", "A registry's name; this server without one"), ("q", "Words to look for")]),
        op("get", "/v1/marketplace/browse/{registry}/{name}", "marketplace", "A listing of this server (registry local) or another registry")
            .ok(s::<ListingDetail>(&mut g)),
        op("post", "/v1/marketplace/browse/{registry}/{name}/ratings", "marketplace", "Rate a listing here or in another registry")
            .body(s::<RateListing>(&mut g))
            .ok(s::<Done>(&mut g)),
        op("post", "/v1/marketplace/install", "marketplace", "Install a listing's bundle, or with dry_run see what it holds and needs")
            .body(s::<Install>(&mut g))
            .ok(s::<gai::bundle::ImportReport>(&mut g)),
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
    // What untyped fields hold, for the dashboard: a recording's result.
    let _ = s::<gai::media::RecordingResult>(&mut g);
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
        assert!(schemas.contains_key("PersonaSpec"));
        assert!(doc["paths"]["/v1/personas/{name}/rollback"]["post"].is_object());
    }
}
