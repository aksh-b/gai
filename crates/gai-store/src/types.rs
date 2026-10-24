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

/// What an API key may do. `admin` implies the others; `run` implies `read`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Admin,
    Run,
    Read,
}

impl Scope {
    pub fn allows(granted: &[Scope], needed: Scope) -> bool {
        granted.iter().any(|g| {
            matches!(
                (g, needed),
                (Scope::Admin, _)
                    | (Scope::Run, Scope::Run | Scope::Read)
                    | (Scope::Read, Scope::Read)
            )
        })
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
