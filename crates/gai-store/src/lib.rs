//! Storage for gai.
//!
//! The rest of gai talks to the repository traits ([`AgentStore`],
//! [`SessionStore`], [`RunStore`]) through [`Store`], so the backend can be
//! swapped. [`SqliteStore`] is the default: one file, zero setup.

pub mod keys;
mod sqlite;
mod types;

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use gai_core::{Message, Usage};
use serde_json::Value;

pub use sqlite::SqliteStore;
pub use types::*;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("database error: {0}")]
    Database(String),
    #[error("corrupt record: {0}")]
    Corrupt(String),
}

impl From<sqlx::Error> for StoreError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => Self::NotFound("row".into()),
            other => Self::Database(other.to_string()),
        }
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        Self::Corrupt(e.to_string())
    }
}

pub type Result<T, E = StoreError> = std::result::Result<T, E>;

#[async_trait]
pub trait AgentStore: Send + Sync {
    /// Save a spec as the agent's next version. Saving a spec identical to
    /// the latest version returns that version instead of adding one.
    async fn put_agent(&self, name: &str, spec: &Value, note: Option<&str>) -> Result<u32>;
    /// A specific version, or the latest when `version` is `None`.
    async fn get_agent(&self, name: &str, version: Option<u32>) -> Result<Option<AgentVersion>>;
    async fn list_agents(&self) -> Result<Vec<AgentSummary>>;
    async fn list_agent_versions(&self, name: &str) -> Result<Vec<AgentVersion>>;
    /// Point `label` (e.g. `prod`) at a version; `None` removes the label.
    async fn set_agent_label(&self, name: &str, label: &str, version: Option<u32>) -> Result<()>;
    async fn delete_agent(&self, name: &str) -> Result<bool>;
}

#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn create_session(&self, new: NewSession) -> Result<Session>;
    async fn get_session(&self, id: &str) -> Result<Option<Session>>;
    async fn list_sessions(&self, agent: Option<&str>, limit: u32) -> Result<Vec<Session>>;
    async fn update_session_title(&self, id: &str, title: &str) -> Result<()>;
    async fn delete_session(&self, id: &str) -> Result<bool>;
    /// Append messages in order; returns their sequence numbers.
    async fn append_messages(
        &self,
        session_id: &str,
        run_id: Option<&str>,
        messages: &[Message],
    ) -> Result<Vec<i64>>;
    /// Record a summary that replaces all messages up to and including
    /// `covers_seq` from now on.
    async fn add_compaction(
        &self,
        session_id: &str,
        run_id: Option<&str>,
        covers_seq: i64,
        summary: &str,
    ) -> Result<()>;
    async fn load_history(&self, session_id: &str) -> Result<History>;
    /// Every message, ignoring compactions (for transcripts and the UI).
    async fn list_messages(&self, session_id: &str) -> Result<Vec<StoredMessage>>;
    /// A new session holding a copy of `from`'s rows (messages and
    /// compactions) with a sequence number below `before_seq`, or all of
    /// them. Sequence numbers are kept, so the copy reads the same.
    async fn fork_session(
        &self,
        from: &str,
        before_seq: Option<i64>,
        new: NewSession,
    ) -> Result<Session>;
}

#[async_trait]
pub trait RunStore: Send + Sync {
    async fn create_run(&self, new: NewRun) -> Result<Run>;
    async fn get_run(&self, id: &str) -> Result<Option<Run>>;
    async fn list_runs(&self, filter: RunFilter) -> Result<Vec<Run>>;
    async fn update_run(&self, id: &str, update: RunUpdate) -> Result<()>;

    async fn start_step(
        &self,
        run_id: &str,
        seq: u32,
        kind: StepKind,
        name: Option<&str>,
        input: Option<&Value>,
    ) -> Result<()>;
    async fn finish_step(
        &self,
        run_id: &str,
        seq: u32,
        status: StepStatus,
        output: Option<&Value>,
        usage: Option<&Usage>,
        cost: Option<f64>,
    ) -> Result<()>;
    async fn list_steps(&self, run_id: &str) -> Result<Vec<Step>>;

    /// Take the oldest queued background run (`metadata.background`) and
    /// mark it running, atomically, so two workers never take the same run.
    async fn claim_queued_run(&self) -> Result<Option<Run>>;

    async fn append_event(&self, run_id: &str, seq: u64, event: &Value) -> Result<()>;
    async fn list_events(
        &self,
        run_id: &str,
        after_seq: Option<u64>,
        limit: u32,
    ) -> Result<Vec<StoredEvent>>;

    async fn record_usage(
        &self,
        run_id: Option<&str>,
        model: &str,
        usage: &Usage,
        cost: Option<f64>,
    ) -> Result<()>;
    /// Usage per model since `since` (RFC 3339), or for all time.
    async fn usage_totals(
        &self,
        since: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Vec<UsageTotal>>;
}

#[async_trait]
pub trait KeyStore: Send + Sync {
    /// Store a key given its hash; the plaintext never reaches the store.
    async fn create_key(
        &self,
        name: &str,
        prefix: &str,
        hash: &str,
        scopes: &[Scope],
        budget_usd: Option<f64>,
    ) -> Result<ApiKey>;
    /// An unrevoked key by hash.
    async fn find_key(&self, hash: &str) -> Result<Option<ApiKey>>;
    async fn list_keys(&self) -> Result<Vec<ApiKey>>;
    async fn revoke_key(&self, id: &str) -> Result<bool>;
    async fn touch_key(&self, id: &str) -> Result<()>;
    async fn add_key_spend(&self, id: &str, usd: f64) -> Result<()>;
    /// Whether any unrevoked key exists.
    async fn has_keys(&self) -> Result<bool>;
}

/// MCP servers added at runtime. The config is stored as JSON; the tools
/// crate owns its shape.
#[async_trait]
pub trait McpStore: Send + Sync {
    async fn put_mcp_server(&self, name: &str, config: &Value) -> Result<()>;
    async fn delete_mcp_server(&self, name: &str) -> Result<bool>;
    async fn list_mcp_servers(&self) -> Result<Vec<(String, Value)>>;
    /// OAuth credentials of a server that needed sign-in.
    async fn get_mcp_oauth(&self, server: &str) -> Result<Option<Value>>;
    async fn put_mcp_oauth(&self, server: &str, credentials: &Value) -> Result<()>;
    async fn delete_mcp_oauth(&self, server: &str) -> Result<()>;
}

/// Memory blocks and their history.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Blocks in a scope, or every block.
    async fn list_blocks(&self, scope: Option<&str>) -> Result<Vec<MemoryBlock>>;
    async fn get_block(&self, id: &str) -> Result<Option<MemoryBlock>>;
    async fn find_block(&self, scope: &str, label: &str) -> Result<Option<MemoryBlock>>;
    /// Create a block; `Conflict` if the scope already has that label.
    async fn create_block(&self, new: NewBlock, actor: Option<&str>) -> Result<MemoryBlock>;
    /// The block with this scope and label, created from `new` if missing.
    async fn ensure_block(&self, new: NewBlock) -> Result<MemoryBlock>;
    /// Set a block's value. With `expected_version`, fails with `Conflict`
    /// if someone else changed the block since it was read.
    async fn set_block_value(
        &self,
        id: &str,
        value: &str,
        expected_version: Option<u32>,
        op: &str,
        actor: Option<&str>,
    ) -> Result<MemoryBlock>;
    async fn update_block_settings(&self, id: &str, s: BlockSettings) -> Result<MemoryBlock>;
    async fn delete_block(&self, id: &str) -> Result<bool>;
    async fn block_history(&self, id: &str) -> Result<Vec<BlockRevision>>;
}

/// Knowledge collections, documents and searchable chunks.
#[async_trait]
pub trait KnowledgeStore: Send + Sync {
    /// Create a collection; `Conflict` if the name is taken.
    async fn create_collection(&self, c: &Collection) -> Result<()>;
    async fn get_collection(&self, name: &str) -> Result<Option<Collection>>;
    async fn list_collections(&self) -> Result<Vec<Collection>>;
    /// Record the embedding size the first time vectors are stored.
    async fn set_collection_dimensions(&self, name: &str, dimensions: u32) -> Result<()>;
    async fn delete_collection(&self, name: &str) -> Result<bool>;
    /// Add a document and its chunks in one transaction, replacing any
    /// document with the same source in the collection.
    async fn put_document(&self, doc: NewDocument, chunks: Vec<NewChunk>) -> Result<Document>;
    async fn get_document(&self, id: &str) -> Result<Option<Document>>;
    async fn find_document(&self, collection: &str, source: &str) -> Result<Option<Document>>;
    async fn list_documents(
        &self,
        collection: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Document>>;
    async fn delete_document(&self, id: &str) -> Result<bool>;
    async fn document_chunks(&self, id: &str) -> Result<Vec<Chunk>>;
    /// The `k` chunks most similar (cosine) to `query`.
    async fn vector_search(
        &self,
        collections: &[String],
        query: &[f32],
        k: usize,
    ) -> Result<Vec<ChunkHit>>;
    /// The `k` best full-text matches for `query`. The query is plain
    /// words; FTS operators in it are treated as text.
    async fn text_search(
        &self,
        collections: &[String],
        query: &str,
        k: usize,
    ) -> Result<Vec<ChunkHit>>;
}

/// Agent Skills uploaded through the API.
#[async_trait]
pub trait SkillStore: Send + Sync {
    /// Save a skill as its next version; returns the version.
    async fn put_skill(
        &self,
        name: &str,
        description: &str,
        body: &str,
        files: &BTreeMap<String, String>,
    ) -> Result<u32>;
    async fn get_skill(&self, name: &str) -> Result<Option<StoredSkill>>;
    async fn list_skills(&self) -> Result<Vec<StoredSkill>>;
    async fn delete_skill(&self, name: &str) -> Result<bool>;
}

/// Versioned workflow definitions.
#[async_trait]
pub trait WorkflowStore: Send + Sync {
    /// Save a spec as the workflow's next version (or return the latest
    /// version when nothing changed).
    async fn put_workflow(&self, name: &str, spec: &Value, note: Option<&str>) -> Result<u32>;
    async fn get_workflow(
        &self,
        name: &str,
        version: Option<u32>,
    ) -> Result<Option<WorkflowVersion>>;
    async fn list_workflows(&self) -> Result<Vec<WorkflowSummary>>;
    async fn list_workflow_versions(&self, name: &str) -> Result<Vec<WorkflowVersion>>;
    /// Point `label` at a version; `None` removes the label.
    async fn set_workflow_label(&self, name: &str, label: &str, version: Option<u32>)
    -> Result<()>;
    async fn delete_workflow(&self, name: &str) -> Result<bool>;
}

/// Media bytes (images, audio, uploads) with their metadata.
#[async_trait]
pub trait BlobStore: Send + Sync {
    async fn put_blob(&self, new: NewBlob) -> Result<Blob>;
    async fn get_blob(&self, id: &str) -> Result<Option<Blob>>;
    async fn blob_data(&self, id: &str) -> Result<Option<Vec<u8>>>;
    /// Newest first; only those a run made, when `run_id` is given.
    async fn list_blobs(&self, run_id: Option<&str>, limit: u32) -> Result<Vec<Blob>>;
    async fn delete_blob(&self, id: &str) -> Result<bool>;
}

/// Triggers and what they did.
#[async_trait]
pub trait TriggerStore: Send + Sync {
    /// `Conflict` if the name is taken.
    async fn create_trigger(
        &self,
        name: &str,
        kind: &str,
        spec: &Value,
        secret: Option<&str>,
    ) -> Result<TriggerRecord>;
    async fn update_trigger(
        &self,
        id: &str,
        name: &str,
        kind: &str,
        spec: &Value,
    ) -> Result<TriggerRecord>;
    async fn get_trigger(&self, id_or_name: &str) -> Result<Option<TriggerRecord>>;
    async fn list_triggers(&self) -> Result<Vec<TriggerRecord>>;
    async fn delete_trigger(&self, id: &str) -> Result<bool>;
    async fn set_trigger_enabled(&self, id: &str, enabled: bool) -> Result<()>;
    async fn set_trigger_next(
        &self,
        id: &str,
        next: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<()>;
    /// Move a schedule on from `expected` to `next`, recording that it fired
    /// at `fired`, only if it is still at `expected`. Returns whether this
    /// caller won, so two schedulers never fire the same time twice.
    async fn advance_trigger(
        &self,
        id: &str,
        expected: chrono::DateTime<chrono::Utc>,
        next: Option<chrono::DateTime<chrono::Utc>>,
        fired: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool>;
    /// Record that a trigger fired (webhooks, file changes, by hand).
    async fn touch_trigger(&self, id: &str, fired: chrono::DateTime<chrono::Utc>) -> Result<()>;
    async fn add_trigger_event(&self, event: NewTriggerEvent) -> Result<i64>;
    /// Newest first.
    async fn list_trigger_events(&self, trigger_id: &str, limit: u32) -> Result<Vec<TriggerEvent>>;
}

/// Everything gai stores.
pub trait Store:
    AgentStore
    + SessionStore
    + RunStore
    + KeyStore
    + McpStore
    + MemoryStore
    + KnowledgeStore
    + SkillStore
    + WorkflowStore
    + BlobStore
    + TriggerStore
{
}
impl<T> Store for T where
    T: AgentStore
        + SessionStore
        + RunStore
        + KeyStore
        + McpStore
        + MemoryStore
        + KnowledgeStore
        + SkillStore
        + WorkflowStore
        + BlobStore
        + TriggerStore
{
}

pub type SharedStore = Arc<dyn Store>;
