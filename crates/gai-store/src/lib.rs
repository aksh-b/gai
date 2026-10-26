//! Storage for gai.
//!
//! The rest of gai talks to the repository traits ([`AgentStore`],
//! [`SessionStore`], [`RunStore`]) through [`Store`], so the backend can be
//! swapped. [`SqliteStore`] is the default: one file, zero setup. A
//! Postgres backend is started behind the `postgres` feature
//! ([`PostgresStore`]); see its docs for what it covers so far.

#[doc(hidden)]
pub mod conformance;
pub mod keys;
#[cfg(feature = "postgres")]
mod pg_vector;
#[cfg(feature = "postgres")]
mod postgres;
mod sqlite;
mod types;

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use gai_core::{Message, Usage};
use serde_json::Value;

#[cfg(feature = "postgres")]
pub use pg_vector::PgVectorIndex;
#[cfg(feature = "postgres")]
pub use postgres::PostgresStore;
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
    /// Messages matching words, best first, across conversations.
    async fn search_messages(&self, q: &MessageSearch) -> Result<Vec<MessageHit>>;
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
    /// Runs, success, latency, cost and ratings per agent version.
    async fn agent_stats(&self, filter: StatsFilter) -> Result<Vec<AgentStats>>;

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
    async fn create_key(&self, key: &NewKey) -> Result<ApiKey>;
    /// An unrevoked key by hash.
    async fn find_key(&self, hash: &str) -> Result<Option<ApiKey>>;
    /// Keys of one organization, or of all.
    async fn list_keys(&self, org_id: Option<&str>) -> Result<Vec<ApiKey>>;
    async fn revoke_key(&self, id: &str) -> Result<bool>;
    async fn touch_key(&self, id: &str) -> Result<()>;
    async fn add_key_spend(&self, id: &str, usd: f64) -> Result<()>;
    /// Whether any unrevoked key exists.
    async fn has_keys(&self) -> Result<bool>;
}

/// Databases agents query.
#[async_trait]
pub trait DataStore: Send + Sync {
    /// Create or replace; a new spec forgets the stored schema.
    async fn put_connection(&self, name: &str, kind: &str, spec: &Value) -> Result<()>;
    async fn get_connection(&self, name: &str) -> Result<Option<DataConnection>>;
    async fn list_connections(&self) -> Result<Vec<DataConnection>>;
    async fn delete_connection(&self, name: &str) -> Result<bool>;
    async fn save_connection_schema(&self, name: &str, schema: &Value) -> Result<()>;
    /// Create or replace (results are kept).
    async fn put_dashboard(&self, name: &str, spec: &Value) -> Result<()>;
    async fn get_dashboard(&self, name: &str) -> Result<Option<DashboardRecord>>;
    async fn list_dashboards(&self) -> Result<Vec<DashboardRecord>>;
    async fn delete_dashboard(&self, name: &str) -> Result<bool>;
    async fn save_dashboard_results(&self, name: &str, results: &Value) -> Result<()>;
}

/// Recordings and what was made of them.
#[async_trait]
pub trait RecordingStore: Send + Sync {
    /// Starts as `processing`.
    async fn create_recording(&self, new: NewRecording) -> Result<Recording>;
    async fn get_recording(&self, id: &str) -> Result<Option<Recording>>;
    /// Newest first.
    async fn list_recordings(&self, limit: u32) -> Result<Vec<Recording>>;
    /// Set the status and what processing made (or why it failed).
    async fn finish_recording(
        &self,
        id: &str,
        status: &str,
        result: Option<&Value>,
        error: Option<&str>,
    ) -> Result<()>;
    async fn delete_recording(&self, id: &str) -> Result<bool>;
}

/// A knowledge graph per collection, built from its chunks.
#[async_trait]
pub trait GraphStore: Send + Sync {
    /// Chunks of `collection` not read into the graph yet, in order.
    async fn graph_pending(&self, collection: &str, limit: u32) -> Result<Vec<Chunk>>;
    /// Add what was found in some chunks and mark `done` as read. Entities
    /// merge by name (case and spacing aside); relations to names not seen
    /// yet create those entities.
    async fn graph_add(
        &self,
        collection: &str,
        done: &[i64],
        entities: &[NewEntity],
        relations: &[NewRelation],
    ) -> Result<()>;
    /// Entities still mentioned somewhere, most mentioned first; those whose
    /// name contains `contains` when it isn't empty.
    async fn graph_nodes(
        &self,
        collection: &str,
        contains: &str,
        limit: u32,
    ) -> Result<Vec<GraphNode>>;
    async fn graph_nodes_by_id(&self, ids: &[i64]) -> Result<Vec<GraphNode>>;
    /// Relations (touching `nodes`, or all), strongest first.
    async fn graph_edges(
        &self,
        collection: &str,
        nodes: &[i64],
        limit: u32,
    ) -> Result<Vec<GraphEdge>>;
    /// Chunks mentioning the most of `nodes`: `(chunk id, how many)`.
    async fn graph_chunks(&self, nodes: &[i64], limit: u32) -> Result<Vec<(i64, u32)>>;
    /// Entities these chunks mention.
    async fn graph_chunk_nodes(&self, chunks: &[i64]) -> Result<Vec<i64>>;
    async fn graph_stats(&self, collection: &str) -> Result<GraphStats>;
    /// Forget the collection's graph (it is rebuilt from scratch next time).
    async fn graph_clear(&self, collection: &str) -> Result<()>;
}

/// Organizations, users, memberships and teams, and which team owns which
/// resource. Kept in the main database for every organization.
#[async_trait]
pub trait DirectoryStore: Send + Sync {
    /// Fails with `Conflict` when the slug is taken.
    async fn create_org(&self, slug: &str, name: &str) -> Result<Org>;
    /// By id or slug.
    async fn get_org(&self, id_or_slug: &str) -> Result<Option<Org>>;
    async fn list_orgs(&self) -> Result<Vec<Org>>;
    async fn update_org(
        &self,
        id: &str,
        name: Option<&str>,
        settings: Option<&Value>,
    ) -> Result<Org>;
    /// Removes its memberships, teams, ownerships and keys. The default
    /// organization can't be deleted.
    async fn delete_org(&self, id: &str) -> Result<bool>;

    /// A user who hasn't signed in yet (an invitation). Returns the existing
    /// user when the email is known.
    async fn create_user(&self, email: &str, name: Option<&str>) -> Result<User>;
    /// At sign-in: the user with this identity-provider subject, else the
    /// user invited with this (verified) email, else a new user.
    async fn sign_in_user(&self, subject: &str, email: &str, name: Option<&str>) -> Result<User>;
    async fn get_user(&self, id: &str) -> Result<Option<User>>;
    async fn find_user_by_email(&self, email: &str) -> Result<Option<User>>;
    async fn list_users(&self) -> Result<Vec<User>>;
    async fn set_user_disabled(&self, id: &str, disabled: bool) -> Result<bool>;

    async fn set_membership(&self, org_id: &str, user_id: &str, role: Role) -> Result<()>;
    /// Also takes the user out of the organization's teams.
    async fn remove_membership(&self, org_id: &str, user_id: &str) -> Result<bool>;
    async fn list_members(&self, org_id: &str) -> Result<Vec<Membership>>;
    async fn user_memberships(&self, user_id: &str) -> Result<Vec<Membership>>;

    /// Fails with `Conflict` when the organization has a team by that name.
    async fn create_team(&self, org_id: &str, name: &str) -> Result<Team>;
    /// By id or name.
    async fn get_team(&self, org_id: &str, id_or_name: &str) -> Result<Option<Team>>;
    async fn list_teams(&self, org_id: &str) -> Result<Vec<Team>>;
    /// Also clears what it owned.
    async fn delete_team(&self, org_id: &str, team_id: &str) -> Result<bool>;
    async fn add_team_member(&self, team_id: &str, user_id: &str) -> Result<()>;
    async fn remove_team_member(&self, team_id: &str, user_id: &str) -> Result<bool>;

    /// Give a resource (`agent`, `collection`, `workflow`, ...) to a team,
    /// or back to the whole organization with `None`.
    async fn set_owner(
        &self,
        org_id: &str,
        kind: &str,
        name: &str,
        team_id: Option<&str>,
    ) -> Result<()>;
    async fn get_owner(&self, org_id: &str, kind: &str, name: &str) -> Result<Option<String>>;
    /// `(kind, name, team id)` of every owned resource.
    async fn list_owners(&self, org_id: &str) -> Result<Vec<(String, String, String)>>;

    /// A browser sign-in, stored by the hash of its token.
    async fn create_login(
        &self,
        hash: &str,
        user_id: &str,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<()>;
    /// An unexpired sign-in and its (enabled) user. Marks the user seen.
    async fn find_login(&self, hash: &str) -> Result<Option<(Login, User)>>;
    async fn delete_login(&self, hash: &str) -> Result<()>;
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

/// Settings changed at runtime, as name/value pairs. Values may be secret
/// (provider API keys), so they are never listed by value.
#[async_trait]
pub trait SettingsStore: Send + Sync {
    async fn get_setting(&self, name: &str) -> Result<Option<String>>;
    async fn put_setting(&self, name: &str, value: &str) -> Result<()>;
    async fn delete_setting(&self, name: &str) -> Result<bool>;
    /// Names and values of every setting whose name starts with `prefix`.
    async fn list_settings(&self, prefix: &str) -> Result<Vec<(String, String)>>;
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
    /// Chunks by id with the scores given, in that order (for hits from a
    /// separate vector index).
    async fn chunk_hits(&self, scored: Vec<(i64, f32)>) -> Result<Vec<ChunkHit>>;
    /// Stored embeddings, by chunk id: one document's, or a collection's in
    /// pages (`after` the last id seen).
    async fn chunk_vectors(
        &self,
        collection: &str,
        document_id: Option<&str>,
        after: i64,
        limit: u32,
    ) -> Result<Vec<(i64, Vec<f32>)>>;
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

/// Versioned personas: reusable voices agents and chats can take on.
#[async_trait]
pub trait PersonaStore: Send + Sync {
    /// Save a spec as the persona's next version (or return the latest
    /// version when nothing changed).
    async fn put_persona(&self, name: &str, spec: &Value, note: Option<&str>) -> Result<u32>;
    async fn get_persona(&self, name: &str, version: Option<u32>)
    -> Result<Option<PersonaVersion>>;
    async fn list_personas(&self) -> Result<Vec<PersonaSummary>>;
    async fn list_persona_versions(&self, name: &str) -> Result<Vec<PersonaVersion>>;
    /// Point `label` at a version; `None` removes the label.
    async fn set_persona_label(&self, name: &str, label: &str, version: Option<u32>) -> Result<()>;
    async fn delete_persona(&self, name: &str) -> Result<bool>;
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

/// Ratings of replies.
#[async_trait]
pub trait FeedbackStore: Send + Sync {
    /// Rate a run; rating it again (same user) replaces the rating.
    #[allow(clippy::too_many_arguments)]
    async fn put_feedback(
        &self,
        run: &Run,
        user_id: &str,
        rating: i8,
        comment: Option<&str>,
        tags: &[String],
    ) -> Result<Feedback>;
    async fn delete_feedback(&self, run_id: &str, user_id: &str) -> Result<bool>;
    /// Newest first.
    async fn list_feedback(&self, filter: FeedbackFilter) -> Result<Vec<Feedback>>;
    /// Good and bad counts per agent version, for one agent or all.
    async fn feedback_summary(&self, agent: Option<&str>) -> Result<Vec<FeedbackSummary>>;
}

/// Eval sets (versioned), evaluations and their per-case results.
#[async_trait]
pub trait EvalStore: Send + Sync {
    /// Save a spec as the set's next version (or return the latest when
    /// nothing changed).
    async fn put_eval_set(&self, name: &str, spec: &Value, note: Option<&str>) -> Result<u32>;
    async fn get_eval_set(
        &self,
        name: &str,
        version: Option<u32>,
    ) -> Result<Option<EvalSetVersion>>;
    async fn list_eval_sets(&self) -> Result<Vec<EvalSetSummary>>;
    async fn list_eval_set_versions(&self, name: &str) -> Result<Vec<EvalSetVersion>>;
    async fn delete_eval_set(&self, name: &str) -> Result<bool>;

    async fn create_eval_run(
        &self,
        eval_set: &str,
        eval_version: u32,
        target: &Value,
    ) -> Result<EvalRun>;
    async fn finish_eval_run(
        &self,
        id: &str,
        status: &str,
        summary: &Value,
        error: Option<&str>,
    ) -> Result<()>;
    async fn get_eval_run(&self, id: &str) -> Result<Option<EvalRun>>;
    /// Newest first; one set or all.
    async fn list_eval_runs(&self, eval_set: Option<&str>, limit: u32) -> Result<Vec<EvalRun>>;
    async fn delete_eval_run(&self, id: &str) -> Result<bool>;
    async fn put_eval_result(&self, result: &EvalResult) -> Result<()>;
    /// In case order.
    async fn list_eval_results(&self, eval_run_id: &str) -> Result<Vec<EvalResult>>;
}

/// The audit log, retention and deleting a person's data.
#[async_trait]
pub trait GovernanceStore: Send + Sync {
    async fn add_audit(&self, entry: NewAudit) -> Result<i64>;
    /// Newest first.
    async fn list_audit(&self, filter: AuditFilter) -> Result<Vec<AuditEntry>>;
    /// Agents that have conversations, for retention sweeps.
    async fn session_agents(&self) -> Result<Vec<String>>;
    /// Delete `agent`'s conversations (and their runs, steps, events and
    /// files) last active before `before`.
    async fn prune_sessions(
        &self,
        agent: &str,
        before: chrono::DateTime<chrono::Utc>,
    ) -> Result<Deleted>;
    /// Delete everything kept about a user: their conversations (with runs
    /// and files), their memory blocks and their ratings.
    async fn forget_user(&self, user_id: &str) -> Result<Deleted>;
}

/// Outside sources synced into collections.
#[async_trait]
pub trait SourceStore: Send + Sync {
    /// Create or change a source; its sync state is kept.
    async fn put_source(
        &self,
        name: &str,
        collection: &str,
        kind: &str,
        spec: &Value,
    ) -> Result<()>;
    async fn get_source(&self, name: &str) -> Result<Option<SourceRecord>>;
    async fn list_sources(&self) -> Result<Vec<SourceRecord>>;
    async fn delete_source(&self, name: &str) -> Result<bool>;
    /// Record a sync: `status`, its error, counts, and (when given) the new
    /// item state.
    async fn save_source_sync(
        &self,
        name: &str,
        status: &str,
        error: Option<&str>,
        stats: &Value,
        state: Option<&Value>,
    ) -> Result<()>;
}

/// A vector index beside the store (pgvector, Qdrant): the store keeps
/// chunks and embeddings, the index answers nearest-neighbour searches.
/// Points are chunk ids.
#[async_trait]
pub trait VectorIndex: Send + Sync {
    /// `pgvector`, `qdrant`, ...
    fn backend(&self) -> &'static str;
    async fn upsert(&self, collection: &str, points: &[(i64, Vec<f32>)]) -> Result<()>;
    async fn remove(&self, collection: &str, ids: &[i64]) -> Result<()>;
    async fn drop_collection(&self, collection: &str) -> Result<()>;
    /// The `k` nearest chunks by cosine similarity (higher is better).
    async fn search(
        &self,
        collections: &[String],
        query: &[f32],
        k: usize,
    ) -> Result<Vec<(i64, f32)>>;
}

pub type SharedVectorIndex = Arc<dyn VectorIndex>;

/// Everything gai stores.
pub trait Store:
    DirectoryStore
    + DataStore
    + RecordingStore
    + GraphStore
    + AgentStore
    + SessionStore
    + RunStore
    + KeyStore
    + McpStore
    + SettingsStore
    + MemoryStore
    + KnowledgeStore
    + SkillStore
    + WorkflowStore
    + PersonaStore
    + BlobStore
    + TriggerStore
    + FeedbackStore
    + EvalStore
    + GovernanceStore
    + SourceStore
{
}
impl<T> Store for T where
    T: DirectoryStore
        + DataStore
        + RecordingStore
        + GraphStore
        + AgentStore
        + SessionStore
        + RunStore
        + KeyStore
        + McpStore
        + SettingsStore
        + MemoryStore
        + KnowledgeStore
        + SkillStore
        + WorkflowStore
        + PersonaStore
        + BlobStore
        + TriggerStore
        + FeedbackStore
        + EvalStore
        + GovernanceStore
        + SourceStore
{
}

pub type SharedStore = Arc<dyn Store>;
