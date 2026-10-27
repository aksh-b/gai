use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use gai_core::{Message, Usage};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentVersion {
    pub name: String,
    pub version: u32,
    /// The agent spec as JSON. The agent crate owns its shape.
    pub spec: Value,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentSummary {
    pub name: String,
    pub latest_version: u32,
    pub labels: BTreeMap<String, u32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NewSession {
    pub agent: Option<String>,
    pub title: Option<String>,
    pub user_id: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Session {
    pub id: String,
    pub agent: Option<String>,
    pub title: Option<String>,
    pub user_id: Option<String>,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Charts with their queries. The data crate owns the spec's shape.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DashboardRecord {
    pub name: String,
    pub spec: Value,
    /// What the last refresh found.
    pub results: Option<Value>,
    pub refreshed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// An agent or workflow published as an app. The server owns the spec's
/// shape; only a hash of the app's key is kept.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AppRecord {
    pub name: String,
    pub spec: Value,
    #[serde(skip)]
    pub key_hash: Option<String>,
    /// The start of the app's key, safe to show.
    pub key_prefix: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A meeting, call or video, and what processing it made: the media crate
/// owns `options` and `result`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Recording {
    pub id: String,
    /// `meeting`, `video` or `audio`.
    pub kind: String,
    pub title: String,
    /// The stored media.
    pub blob_id: Option<String>,
    /// `processing`, `done` or `failed`.
    pub status: String,
    pub options: Value,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Default)]
pub struct NewRecording {
    pub kind: String,
    pub title: String,
    pub blob_id: Option<String>,
    pub options: Value,
}

/// A database agents can query. The data crate owns the spec's shape.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DataConnection {
    pub name: String,
    /// `postgres`, `mysql`, `sqlite`, `bigquery` or `snowflake`.
    pub kind: String,
    pub spec: Value,
    /// Tables and columns, as last read from the database.
    pub schema: Option<Value>,
    pub schema_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// An entity a model found in chunks, with the chunks that mention it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NewEntity {
    pub name: String,
    /// `person`, `organization`, `place`, `product`, `event`, `concept`,
    /// `date`, `amount` or `other`.
    pub kind: String,
    pub description: Option<String>,
    pub chunk_ids: Vec<i64>,
}

/// `source` —relation→ `target`, as one chunk says.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NewRelation {
    pub source: String,
    pub target: String,
    pub relation: String,
    pub chunk_id: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "Entity")]
pub struct GraphNode {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub description: Option<String>,
    /// Chunks that mention it.
    pub mentions: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "EntityRelation")]
pub struct GraphEdge {
    pub source: i64,
    pub target: i64,
    pub relation: String,
    /// Chunks that say so.
    pub weight: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GraphStats {
    pub nodes: u64,
    pub edges: u64,
    /// Chunks read into the graph, and still to read.
    pub chunks_done: u64,
    pub chunks_pending: u64,
}

/// Which runs [`RunStore::agent_stats`] counts.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StatsFilter {
    pub agent: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
}

/// How one agent version did. Evaluation runs aren't counted.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentStats {
    pub agent: String,
    pub agent_version: Option<u32>,
    pub runs: u64,
    pub completed: u64,
    pub failed: u64,
    /// Cancelled, or stopped by a budget.
    pub stopped: u64,
    /// Queued, running or waiting for approval.
    pub open: u64,
    /// Completed out of finished runs, 0 to 1.
    pub success_rate: f64,
    pub avg_steps: f64,
    pub p50_latency_ms: u64,
    pub p95_latency_ms: u64,
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Thumbs up and down.
    pub good: u64,
    pub bad: u64,
    pub last_run_at: Option<DateTime<Utc>>,
}

/// One run, as [`AgentStats`] needs it.
pub(crate) struct StatRow {
    pub agent: String,
    pub agent_version: Option<u32>,
    pub status: String,
    pub steps: i64,
    pub cost: f64,
    pub usage: Usage,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub good: i64,
    pub bad: i64,
}

/// Group runs by agent version, busiest first.
pub(crate) fn aggregate(rows: Vec<StatRow>) -> Vec<AgentStats> {
    // Per agent version: the counts, latencies and total steps.
    type Group = (AgentStats, Vec<u64>, i64);
    let mut groups: BTreeMap<(String, Option<u32>), Group> = BTreeMap::new();
    for r in rows {
        let (s, latencies, steps) = groups
            .entry((r.agent.clone(), r.agent_version))
            .or_insert_with(|| {
                (
                    AgentStats {
                        agent: r.agent.clone(),
                        agent_version: r.agent_version,
                        ..Default::default()
                    },
                    Vec::new(),
                    0,
                )
            });
        s.runs += 1;
        match r.status.as_str() {
            "completed" => s.completed += 1,
            "failed" => s.failed += 1,
            "cancelled" | "budget_exceeded" => s.stopped += 1,
            _ => s.open += 1,
        }
        *steps += r.steps;
        s.cost_usd += r.cost;
        s.input_tokens += r.usage.input_tokens;
        s.output_tokens += r.usage.output_tokens;
        s.good += r.good.max(0) as u64;
        s.bad += r.bad.max(0) as u64;
        if let Some(f) = r.finished_at {
            latencies.push((f - r.created_at).num_milliseconds().max(0) as u64);
        }
        if s.last_run_at.is_none_or(|t| r.created_at > t) {
            s.last_run_at = Some(r.created_at);
        }
    }
    let mut out: Vec<AgentStats> = groups
        .into_values()
        .map(|(mut s, mut lat, steps)| {
            let finished = s.completed + s.failed + s.stopped;
            if finished > 0 {
                s.success_rate = s.completed as f64 / finished as f64;
            }
            if s.runs > 0 {
                s.avg_steps = steps as f64 / s.runs as f64;
            }
            lat.sort_unstable();
            let pct = |p: f64| -> u64 {
                if lat.is_empty() {
                    return 0;
                }
                lat[((lat.len() - 1) as f64 * p).round() as usize]
            };
            s.p50_latency_ms = pct(0.5);
            s.p95_latency_ms = pct(0.95);
            s
        })
        .collect();
    out.sort_by(|a, b| {
        b.runs
            .cmp(&a.runs)
            .then(a.agent.cmp(&b.agent))
            .then(b.agent_version.cmp(&a.agent_version))
    });
    out
}

/// What to look for in conversations.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MessageSearch {
    /// Words to find (all of them; the last may be the start of a word).
    pub text: String,
    /// Only conversations with this agent.
    pub agent: Option<String>,
    /// Only this user's conversations.
    pub user_id: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub limit: u32,
}

/// A message that matched, with the words marked `[like this]`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MessageHit {
    pub session_id: String,
    pub seq: i64,
    pub run_id: Option<String>,
    pub role: String,
    pub snippet: String,
    pub created_at: DateTime<Utc>,
    pub session_title: Option<String>,
    pub agent: Option<String>,
    pub user_id: Option<String>,
    /// Higher is a better match.
    pub score: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StoredMessage {
    pub seq: i64,
    pub run_id: Option<String>,
    pub message: Message,
    pub created_at: DateTime<Utc>,
}

/// What a model should see of a session: the latest compaction summary, if
/// any, then every message after it.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct History {
    pub summary: Option<String>,
    pub messages: Vec<StoredMessage>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Queued,
    Running,
    AwaitingApproval,
    Completed,
    Failed,
    Cancelled,
    BudgetExceeded,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::AwaitingApproval => "awaiting_approval",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::BudgetExceeded => "budget_exceeded",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        serde_json::from_value(Value::String(s.to_owned())).ok()
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::BudgetExceeded
        )
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NewRun {
    pub id: Option<String>,
    pub session_id: String,
    pub agent: String,
    pub agent_version: Option<u32>,
    pub parent_run_id: Option<String>,
    pub input: Value,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Run {
    pub id: String,
    pub session_id: String,
    pub agent: String,
    pub agent_version: Option<u32>,
    pub parent_run_id: Option<String>,
    pub status: RunStatus,
    pub input: Value,
    pub output: Option<Value>,
    pub error: Option<String>,
    pub usage: Usage,
    pub cost: f64,
    pub steps: u32,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// Fields to change on a run; `None` leaves a field alone.
#[derive(Clone, Debug, Default)]
pub struct RunUpdate {
    pub status: Option<RunStatus>,
    pub output: Option<Value>,
    pub error: Option<String>,
    pub usage: Option<Usage>,
    pub cost: Option<f64>,
    pub steps: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct RunFilter {
    pub session_id: Option<String>,
    pub agent: Option<String>,
    pub status: Option<RunStatus>,
    pub parent_run_id: Option<String>,
    /// Runs replayed from this run (`metadata.fork.run_id`).
    pub fork_of: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Model,
    Tool,
    Approval,
    Compaction,
    /// A workflow node.
    Node,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Started,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Step {
    pub run_id: String,
    pub seq: u32,
    pub kind: StepKind,
    pub name: Option<String>,
    pub status: StepStatus,
    pub input: Option<Value>,
    pub output: Option<Value>,
    pub usage: Option<Usage>,
    pub cost: Option<f64>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StoredEvent {
    pub seq: u64,
    pub event: Value,
    pub created_at: DateTime<Utc>,
}

/// What an API key may do. Each implies the ones after it: `admin`,
/// `build` (change agents, workflows, knowledge and the like), `run`,
/// `read`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Admin,
    Build,
    Run,
    Read,
}

impl Scope {
    fn rank(self) -> u8 {
        match self {
            Scope::Read => 0,
            Scope::Run => 1,
            Scope::Build => 2,
            Scope::Admin => 3,
        }
    }

    pub fn allows(granted: &[Scope], needed: Scope) -> bool {
        granted.iter().any(|g| g.rank() >= needed.rank())
    }

    /// The widest of `granted`, or `read` when empty.
    pub fn highest(granted: &[Scope]) -> Scope {
        granted
            .iter()
            .copied()
            .max_by_key(|s| s.rank())
            .unwrap_or(Scope::Read)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ApiKey {
    pub id: String,
    pub name: String,
    /// The first characters of the key, to tell keys apart.
    pub prefix: String,
    pub scopes: Vec<Scope>,
    pub budget_usd: Option<f64>,
    pub spent_usd: f64,
    pub org_id: String,
    pub team_id: Option<String>,
    pub user_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UsageTotal {
    pub model: String,
    pub calls: u64,
    pub usage: Usage,
    pub cost: f64,
}

/// A named, size-limited piece of text an agent always sees and can edit.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MemoryBlock {
    pub id: String,
    /// `agent:<name>`, `user:<id>` or `shared:<name>`.
    pub scope: String,
    pub label: String,
    /// Tells the agent what the block is for.
    pub description: Option<String>,
    pub value: String,
    /// Most characters the value may hold.
    pub limit: u32,
    pub read_only: bool,
    pub version: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NewBlock {
    pub scope: String,
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub value: String,
    pub limit: u32,
    #[serde(default)]
    pub read_only: bool,
}

/// One past value of a block.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BlockRevision {
    pub version: u32,
    pub value: String,
    /// `create`, `append`, `replace`, `rethink` or `edit`.
    pub op: String,
    /// The run that made the change, or `api`.
    pub actor: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Changes to a block's settings; `None` leaves a field alone.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BlockSettings {
    pub description: Option<String>,
    pub limit: Option<u32>,
    pub read_only: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Collection {
    pub name: String,
    pub description: Option<String>,
    /// Model every chunk (and every query) is embedded with.
    pub embedding_model: String,
    pub dimensions: Option<u32>,
    pub chunk_tokens: u32,
    pub chunk_overlap: u32,
    pub metadata: Value,
    /// Filled in by listings.
    #[serde(default)]
    pub documents: u64,
    #[serde(default)]
    pub chunks: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Document {
    pub id: String,
    pub collection: String,
    /// Where the text came from (a path, URL or caller-chosen key). Adding
    /// a document with the same source replaces the old one.
    pub source: String,
    pub title: Option<String>,
    pub content_hash: String,
    pub chars: u64,
    pub chunks: u32,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Default)]
pub struct NewDocument {
    pub collection: String,
    pub source: String,
    pub title: Option<String>,
    pub content_hash: String,
    pub chars: u64,
    pub metadata: Value,
}

#[derive(Clone, Debug, Default)]
pub struct NewChunk {
    pub text: String,
    pub embedding: Option<Vec<f32>>,
    pub metadata: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Chunk {
    pub id: i64,
    pub document_id: String,
    pub collection: String,
    pub seq: u32,
    pub text: String,
    pub metadata: Value,
}

/// A chunk found by a search, with its score in that search (cosine
/// similarity for vectors, negated BM25 for text: higher is better).
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChunkHit {
    pub chunk: Chunk,
    pub score: f32,
    pub source: String,
    pub title: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StoredSkill {
    pub name: String,
    pub description: String,
    pub version: u32,
    /// The whole SKILL.md, frontmatter included.
    pub body: String,
    /// Bundled files by relative path.
    pub files: BTreeMap<String, String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A stored workflow version.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkflowVersion {
    pub name: String,
    pub version: u32,
    /// The workflow spec as JSON. The orchestra crate owns its shape.
    pub spec: Value,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorkflowSummary {
    pub name: String,
    pub latest_version: u32,
    pub description: Option<String>,
    pub labels: BTreeMap<String, u32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A stored persona version.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PersonaVersion {
    pub name: String,
    pub version: u32,
    /// The persona spec as JSON. The agent crate owns its shape.
    pub spec: Value,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PersonaSummary {
    pub name: String,
    pub latest_version: u32,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub avatar: Option<String>,
    pub labels: BTreeMap<String, u32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Stored media: everything but the bytes.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Blob {
    pub id: String,
    pub media_type: String,
    pub size: u64,
    pub sha256: String,
    /// A file name, when it came with one.
    pub name: Option<String>,
    /// The run that made it.
    pub run_id: Option<String>,
    /// What made it: `upload`, `image_gen`, `speak`, `api`...
    pub source: Option<String>,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Default)]
pub struct NewBlob {
    pub media_type: String,
    pub name: Option<String>,
    pub run_id: Option<String>,
    pub source: Option<String>,
    pub metadata: Value,
    pub data: Vec<u8>,
}

/// A stored trigger. The triggers crate owns the shape of `spec`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TriggerRecord {
    pub id: String,
    pub name: String,
    /// `cron`, `webhook` or `file`.
    pub kind: String,
    pub enabled: bool,
    pub spec: Value,
    /// For webhooks: the key requests are signed with.
    #[serde(skip_serializing)]
    pub secret: Option<String>,
    /// For schedules: when it fires next.
    pub next_fire_at: Option<DateTime<Utc>>,
    pub last_fired_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One firing of a trigger.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TriggerEvent {
    pub id: i64,
    pub trigger_id: String,
    pub fired_at: DateTime<Utc>,
    /// `schedule`, `missed`, `webhook`, `file` or `manual`.
    pub reason: String,
    /// What came with it: the webhook body, the changed file...
    pub payload: Option<Value>,
    pub run_id: Option<String>,
    /// `started`, `skipped` or `failed`.
    pub status: String,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct NewTriggerEvent {
    pub trigger_id: String,
    pub reason: String,
    pub payload: Option<Value>,
    pub run_id: Option<String>,
    pub status: String,
    pub error: Option<String>,
}

/// A person's rating of a reply (one run).
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Feedback {
    pub id: String,
    pub run_id: String,
    pub session_id: String,
    pub agent: String,
    pub agent_version: Option<u32>,
    /// Who rated it (empty when unknown).
    pub user_id: String,
    /// 1 for good, -1 for bad.
    pub rating: i8,
    pub comment: Option<String>,
    /// Short reasons, e.g. `wrong`, `too long`, `unsafe`.
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Ratings counted for an agent (or one of its versions).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FeedbackSummary {
    pub agent: String,
    pub agent_version: Option<u32>,
    pub good: u64,
    pub bad: u64,
}

#[derive(Clone, Debug, Default)]
pub struct FeedbackFilter {
    pub agent: Option<String>,
    pub run_id: Option<String>,
    /// Only good (1) or bad (-1).
    pub rating: Option<i8>,
    pub limit: u32,
}

/// A stored eval set version.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EvalSetVersion {
    pub name: String,
    pub version: u32,
    /// The eval set spec as JSON. The eval crate owns its shape.
    pub spec: Value,
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EvalSetSummary {
    pub name: String,
    pub latest_version: u32,
    pub description: Option<String>,
    pub cases: u32,
    pub labels: BTreeMap<String, u32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One evaluation of an eval set against a target.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EvalRun {
    pub id: String,
    pub eval_set: String,
    pub eval_version: u32,
    /// What was evaluated: `{agent, agent_version, model}`.
    pub target: Value,
    /// `running`, `completed` or `failed`.
    pub status: String,
    /// Totals once finished: score, passed, total, cost, latency.
    pub summary: Value,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// How one case did in an evaluation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EvalResult {
    pub eval_run_id: String,
    pub case_id: String,
    pub run_id: Option<String>,
    pub output: Option<String>,
    /// Each scorer's result: `{scorer, score, passed, reason}`.
    pub scores: Value,
    /// 0 to 1.
    pub score: f64,
    pub passed: bool,
    pub cost: f64,
    pub latency_ms: u64,
    pub error: Option<String>,
}

/// One change recorded in the audit log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AuditEntry {
    pub id: i64,
    pub at: DateTime<Utc>,
    /// The API key's name, or `anonymous`.
    pub actor: String,
    pub actor_id: String,
    /// The request, e.g. `POST /v1/agents`.
    pub action: String,
    /// What it touched, e.g. `agents/researcher`.
    pub target: String,
    /// The HTTP status of the answer.
    pub status: u16,
    /// Extra facts, e.g. an approval's decision.
    pub detail: Value,
}

#[derive(Clone, Debug, Default)]
pub struct NewAudit {
    pub actor: String,
    pub actor_id: String,
    pub action: String,
    pub target: String,
    pub status: u16,
    pub detail: Value,
}

#[derive(Clone, Debug, Default)]
pub struct AuditFilter {
    pub actor: Option<String>,
    /// Targets starting with this, e.g. `agents/`.
    pub target: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub limit: u32,
}

/// What a deletion removed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Deleted {
    pub sessions: u64,
    pub runs: u64,
    pub blobs: u64,
    pub memory_blocks: u64,
    pub feedback: u64,
}

/// An outside source synced into a knowledge collection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SourceRecord {
    pub name: String,
    pub collection: String,
    /// `folder`, `web`, `github`, `confluence` or `notion`.
    pub kind: String,
    /// The source spec as JSON. The memory crate owns its shape.
    pub spec: Value,
    /// Each item seen and its version, for incremental syncs.
    #[serde(skip_serializing)]
    pub state: Value,
    /// `new`, `syncing`, `ok` or `failed`.
    pub status: String,
    pub error: Option<String>,
    /// The last sync's counts: added, updated, removed, unchanged, failed.
    pub stats: Value,
    pub last_sync_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A new API key. The plaintext never reaches the store, only its hash.
#[derive(Clone, Debug)]
pub struct NewKey {
    pub name: String,
    pub prefix: String,
    pub hash: String,
    pub scopes: Vec<Scope>,
    pub budget_usd: Option<f64>,
    /// The organization the key acts in.
    pub org_id: String,
    /// A team key may only change resources its team owns (or no team does).
    pub team_id: Option<String>,
    /// The user who created it, when a signed-in user did.
    pub user_id: Option<String>,
}

impl NewKey {
    /// A key in the default organization.
    pub fn new(
        name: &str,
        prefix: &str,
        hash: &str,
        scopes: &[Scope],
        budget_usd: Option<f64>,
    ) -> Self {
        Self {
            name: name.to_owned(),
            prefix: prefix.to_owned(),
            hash: hash.to_owned(),
            scopes: scopes.to_vec(),
            budget_usd,
            org_id: DEFAULT_ORG.into(),
            team_id: None,
            user_id: None,
        }
    }
}

/// The organization every server starts with. Its data lives in the main
/// database; other organizations each get a database of their own.
pub const DEFAULT_ORG: &str = "default";

/// What a member of an organization may do.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Reads everything in the organization.
    Viewer,
    /// Also chats with and runs agents and workflows.
    Member,
    /// Also builds: agents, personas, workflows, skills, knowledge,
    /// triggers, sources and evals.
    Builder,
    /// Also keys, MCP servers, providers, settings, members and teams,
    /// the audit log and data deletion.
    Admin,
    /// Also renames or deletes the organization and names other owners.
    Owner,
}

impl Role {
    pub fn scope(self) -> Scope {
        match self {
            Role::Viewer => Scope::Read,
            Role::Member => Scope::Run,
            Role::Builder => Scope::Build,
            Role::Admin | Role::Owner => Scope::Admin,
        }
    }
}

impl std::str::FromStr for Role {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, String> {
        serde_json::from_value(Value::String(s.to_ascii_lowercase()))
            .map_err(|_| format!("unknown role {s:?}: use owner, admin, builder, member or viewer"))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Org {
    pub id: String,
    /// Letters, digits and `-`; picks the organization in URLs and headers.
    pub slug: String,
    pub name: String,
    /// Branding and limits; see the server's org settings.
    pub settings: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct User {
    pub id: String,
    pub email: String,
    pub name: Option<String>,
    /// The identity provider's `issuer|subject`, once they have signed in.
    #[serde(skip_serializing)]
    pub subject: Option<String>,
    pub disabled: bool,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: Option<DateTime<Utc>>,
}

/// A user's place in an organization.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Membership {
    pub org_id: String,
    pub org_slug: String,
    pub org_name: String,
    pub user_id: String,
    pub email: String,
    pub name: Option<String>,
    pub role: Role,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Team {
    pub id: String,
    pub org_id: String,
    pub name: String,
    /// User ids.
    pub members: Vec<String>,
    pub created_at: DateTime<Utc>,
}

/// A signed-in browser.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Login {
    pub user_id: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}
