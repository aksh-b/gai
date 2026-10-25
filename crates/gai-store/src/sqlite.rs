use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use gai_core::{Message, Usage};
use serde_json::Value;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow, SqliteSynchronous,
};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool};

use crate::*;

#[path = "sqlite_blobs.rs"]
mod blobs;
#[path = "sqlite_governance.rs"]
mod governance;
#[path = "sqlite_memory.rs"]
mod memory;
#[path = "sqlite_personas.rs"]
mod personas;
#[path = "sqlite_quality.rs"]
mod quality;
#[path = "sqlite_triggers.rs"]
mod triggers;
#[path = "sqlite_workflows.rs"]
mod workflows;

/// SQLite-backed [`Store`]. Uses WAL mode, so readers never block the
/// writer.
#[derive(Clone)]
pub struct SqliteStore {
    pool: SqlitePool,
    _temp: Option<std::sync::Arc<TempFiles>>,
}

/// Deletes a temporary database's files once the last store clone is gone.
struct TempFiles(std::path::PathBuf);

impl Drop for TempFiles {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let mut p = self.0.clone().into_os_string();
            p.push(suffix);
            let _ = std::fs::remove_file(p);
        }
    }
}

impl SqliteStore {
    /// Open (creating if needed) the database at `path` and run migrations.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        if let Some(dir) = path.as_ref().parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)
                .map_err(|e| StoreError::Database(format!("{}: {e}", dir.display())))?;
        }
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(10));
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(opts)
            .await?;
        Self::migrate(pool, None).await
    }

    /// A throwaway database in the temp directory, deleted when the store
    /// is dropped. Used by tests and one-off CLI runs.
    ///
    /// This is a real file rather than `:memory:`: an in-memory database
    /// lives on a single connection, and cancelling a run mid-query makes
    /// the pool discard that connection, taking the data with it.
    pub async fn temporary() -> Result<Self> {
        let path = std::env::temp_dir().join(format!("{}.db", gai_core::new_id("gai-tmp")));
        let mut store = Self::open(&path).await?;
        store._temp = Some(std::sync::Arc::new(TempFiles(path)));
        Ok(store)
    }

    async fn migrate(pool: SqlitePool, temp: Option<std::sync::Arc<TempFiles>>) -> Result<Self> {
        sqlx::migrate!("./migrations/sqlite")
            .run(&pool)
            .await
            .map_err(|e| StoreError::Database(format!("migration failed: {e}")))?;
        Ok(Self { pool, _temp: temp })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// A transaction that takes the write lock up front.
    ///
    /// A plain `BEGIN` starts as a reader; if another connection writes
    /// before this one does, SQLite cannot upgrade it and fails at once with
    /// "database is locked" (the busy timeout does not apply). `BEGIN
    /// IMMEDIATE` waits for the lock instead, and serializes read-then-write
    /// sequences such as picking the next message number.
    async fn write_tx(&self) -> Result<sqlx::Transaction<'static, Sqlite>> {
        Ok(self.pool.begin_with("BEGIN IMMEDIATE").await?)
    }
}

fn now() -> DateTime<Utc> {
    Utc::now()
}

fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).expect("serializable")
}

fn parse<T: serde::de::DeserializeOwned>(s: &str) -> Result<T> {
    Ok(serde_json::from_str(s)?)
}

fn opt_parse<T: serde::de::DeserializeOwned>(s: Option<String>) -> Result<Option<T>> {
    s.map(|s| parse(&s)).transpose()
}

fn agent_version(r: &SqliteRow) -> Result<AgentVersion> {
    Ok(AgentVersion {
        name: r.try_get("name")?,
        version: r.try_get::<i64, _>("version")? as u32,
        spec: parse(&r.try_get::<String, _>("spec")?)?,
        note: r.try_get("note")?,
        created_at: r.try_get("created_at")?,
    })
}

fn session(r: &SqliteRow) -> Result<Session> {
    Ok(Session {
        id: r.try_get("id")?,
        agent: r.try_get("agent")?,
        title: r.try_get("title")?,
        user_id: r.try_get("user_id")?,
        metadata: parse(&r.try_get::<String, _>("metadata")?)?,
        created_at: r.try_get("created_at")?,
        updated_at: r.try_get("updated_at")?,
    })
}

fn stored_message(r: &SqliteRow) -> Result<StoredMessage> {
    Ok(StoredMessage {
        seq: r.try_get("seq")?,
        run_id: r.try_get("run_id")?,
        message: parse(&r.try_get::<String, _>("body")?)?,
        created_at: r.try_get("created_at")?,
    })
}

fn run(r: &SqliteRow) -> Result<Run> {
    let status: String = r.try_get("status")?;
    Ok(Run {
        id: r.try_get("id")?,
        session_id: r.try_get("session_id")?,
        agent: r.try_get("agent")?,
        agent_version: r
            .try_get::<Option<i64>, _>("agent_version")?
            .map(|v| v as u32),
        parent_run_id: r.try_get("parent_run_id")?,
        status: RunStatus::parse(&status)
            .ok_or_else(|| StoreError::Corrupt(format!("run status {status:?}")))?,
        input: parse(&r.try_get::<String, _>("input")?)?,
        output: opt_parse(r.try_get("output")?)?,
        error: r.try_get("error")?,
        usage: parse(&r.try_get::<String, _>("usage")?)?,
        cost: r.try_get("cost")?,
        steps: r.try_get::<i64, _>("steps")? as u32,
        metadata: parse(&r.try_get::<String, _>("metadata")?)?,
        created_at: r.try_get("created_at")?,
        updated_at: r.try_get("updated_at")?,
        finished_at: r.try_get("finished_at")?,
    })
}

fn step(r: &SqliteRow) -> Result<Step> {
    let kind: String = r.try_get("kind")?;
    let status: String = r.try_get("status")?;
    Ok(Step {
        run_id: r.try_get("run_id")?,
        seq: r.try_get::<i64, _>("seq")? as u32,
        kind: parse(&json(&kind))?,
        name: r.try_get("name")?,
        status: parse(&json(&status))?,
        input: opt_parse(r.try_get("input")?)?,
        output: opt_parse(r.try_get("output")?)?,
        usage: opt_parse(r.try_get("usage")?)?,
        cost: r.try_get("cost")?,
        started_at: r.try_get("started_at")?,
        finished_at: r.try_get("finished_at")?,
    })
}

fn enum_str<T: serde::Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(Value::String(s)) => s,
        _ => unreachable!("unit enum"),
    }
}

#[async_trait]
impl AgentStore for SqliteStore {
    async fn put_agent(&self, name: &str, spec: &Value, note: Option<&str>) -> Result<u32> {
        let mut tx = self.write_tx().await?;
        let latest: Option<(i64, String)> = sqlx::query_as(
            "SELECT v.version, v.spec FROM agents a JOIN agent_versions v ON v.name = a.name AND v.version = a.latest_version WHERE a.name = ?",
        )
        .bind(name)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((v, s)) = &latest
            && parse::<Value>(s)? == *spec
        {
            return Ok(*v as u32);
        }
        let version = latest.map(|(v, _)| v + 1).unwrap_or(1);
        let ts = now();
        sqlx::query(
            "INSERT INTO agents (name, latest_version, created_at, updated_at) VALUES (?, ?, ?, ?)
             ON CONFLICT(name) DO UPDATE SET latest_version = excluded.latest_version, updated_at = excluded.updated_at",
        )
        .bind(name)
        .bind(version)
        .bind(ts)
        .bind(ts)
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO agent_versions (name, version, spec, note, created_at) VALUES (?, ?, ?, ?, ?)")
            .bind(name)
            .bind(version)
            .bind(json(spec))
            .bind(note)
            .bind(ts)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(version as u32)
    }

    async fn get_agent(&self, name: &str, version: Option<u32>) -> Result<Option<AgentVersion>> {
        let row = match version {
            Some(v) => {
                sqlx::query("SELECT * FROM agent_versions WHERE name = ? AND version = ?")
                    .bind(name)
                    .bind(v as i64)
                    .fetch_optional(&self.pool)
                    .await?
            }
            None => {
                sqlx::query(
                    "SELECT v.* FROM agents a JOIN agent_versions v ON v.name = a.name AND v.version = a.latest_version WHERE a.name = ?",
                )
                .bind(name)
                .fetch_optional(&self.pool)
                .await?
            }
        };
        row.as_ref().map(agent_version).transpose()
    }

    async fn list_agents(&self) -> Result<Vec<AgentSummary>> {
        let rows = sqlx::query("SELECT * FROM agents ORDER BY name")
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|r| {
                Ok(AgentSummary {
                    name: r.try_get("name")?,
                    latest_version: r.try_get::<i64, _>("latest_version")? as u32,
                    labels: parse(&r.try_get::<String, _>("labels")?)?,
                    created_at: r.try_get("created_at")?,
                    updated_at: r.try_get("updated_at")?,
                })
            })
            .collect()
    }

    async fn list_agent_versions(&self, name: &str) -> Result<Vec<AgentVersion>> {
        let rows = sqlx::query("SELECT * FROM agent_versions WHERE name = ? ORDER BY version DESC")
            .bind(name)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(agent_version).collect()
    }

    async fn set_agent_label(&self, name: &str, label: &str, version: Option<u32>) -> Result<()> {
        let mut tx = self.write_tx().await?;
        let labels: Option<String> = sqlx::query_scalar("SELECT labels FROM agents WHERE name = ?")
            .bind(name)
            .fetch_optional(&mut *tx)
            .await?;
        let mut labels: BTreeMap<String, u32> =
            parse(&labels.ok_or_else(|| StoreError::NotFound(format!("agent {name}")))?)?;
        match version {
            Some(v) => {
                let exists: Option<i64> = sqlx::query_scalar(
                    "SELECT 1 FROM agent_versions WHERE name = ? AND version = ?",
                )
                .bind(name)
                .bind(v as i64)
                .fetch_optional(&mut *tx)
                .await?;
                if exists.is_none() {
                    return Err(StoreError::NotFound(format!("agent {name} version {v}")));
                }
                labels.insert(label.to_owned(), v);
            }
            None => {
                labels.remove(label);
            }
        }
        sqlx::query("UPDATE agents SET labels = ?, updated_at = ? WHERE name = ?")
            .bind(json(&labels))
            .bind(now())
            .bind(name)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn delete_agent(&self, name: &str) -> Result<bool> {
        let r = sqlx::query("DELETE FROM agents WHERE name = ?")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }
}

#[async_trait]
impl SessionStore for SqliteStore {
    async fn create_session(&self, new: NewSession) -> Result<Session> {
        let id = gai_core::new_id("ses");
        let ts = now();
        let metadata = if new.metadata.is_null() {
            Value::Object(Default::default())
        } else {
            new.metadata
        };
        sqlx::query("INSERT INTO sessions (id, agent, title, user_id, metadata, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(&id)
            .bind(&new.agent)
            .bind(&new.title)
            .bind(&new.user_id)
            .bind(json(&metadata))
            .bind(ts)
            .bind(ts)
            .execute(&self.pool)
            .await?;
        Ok(Session {
            id,
            agent: new.agent,
            title: new.title,
            user_id: new.user_id,
            metadata,
            created_at: ts,
            updated_at: ts,
        })
    }

    async fn get_session(&self, id: &str) -> Result<Option<Session>> {
        let row = sqlx::query("SELECT * FROM sessions WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(session).transpose()
    }

    async fn list_sessions(&self, agent: Option<&str>, limit: u32) -> Result<Vec<Session>> {
        let rows = sqlx::query("SELECT * FROM sessions WHERE (?1 IS NULL OR agent = ?1) ORDER BY updated_at DESC LIMIT ?2")
            .bind(agent)
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(session).collect()
    }

    async fn update_session_title(&self, id: &str, title: &str) -> Result<()> {
        sqlx::query("UPDATE sessions SET title = ?, updated_at = ? WHERE id = ?")
            .bind(title)
            .bind(now())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn delete_session(&self, id: &str) -> Result<bool> {
        let r = sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    async fn append_messages(
        &self,
        session_id: &str,
        run_id: Option<&str>,
        messages: &[Message],
    ) -> Result<Vec<i64>> {
        let mut tx = self.write_tx().await?;
        let mut seq: i64 =
            sqlx::query_scalar("SELECT COALESCE(MAX(seq), 0) FROM messages WHERE session_id = ?")
                .bind(session_id)
                .fetch_one(&mut *tx)
                .await?;
        let ts = now();
        let mut seqs = Vec::with_capacity(messages.len());
        for m in messages {
            seq += 1;
            sqlx::query("INSERT INTO messages (session_id, seq, run_id, kind, body, created_at) VALUES (?, ?, ?, 'message', ?, ?)")
                .bind(session_id)
                .bind(seq)
                .bind(run_id)
                .bind(json(m))
                .bind(ts)
                .execute(&mut *tx)
                .await?;
            seqs.push(seq);
        }
        sqlx::query("UPDATE sessions SET updated_at = ? WHERE id = ?")
            .bind(ts)
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(seqs)
    }

    async fn fork_session(
        &self,
        from: &str,
        before_seq: Option<i64>,
        new: NewSession,
    ) -> Result<Session> {
        if self.get_session(from).await?.is_none() {
            return Err(StoreError::NotFound(format!("session {from}")));
        }
        let session = self.create_session(new).await?;
        let mut tx = self.write_tx().await?;
        sqlx::query(
            "INSERT INTO messages (session_id, seq, run_id, kind, covers_seq, body, created_at)
             SELECT ?, seq, run_id, kind, covers_seq, body, created_at FROM messages
             WHERE session_id = ? AND seq < ?",
        )
        .bind(&session.id)
        .bind(from)
        .bind(before_seq.unwrap_or(i64::MAX))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(session)
    }

    async fn add_compaction(
        &self,
        session_id: &str,
        run_id: Option<&str>,
        covers_seq: i64,
        summary: &str,
    ) -> Result<()> {
        let mut tx = self.write_tx().await?;
        let seq: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE session_id = ?",
        )
        .bind(session_id)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO messages (session_id, seq, run_id, kind, covers_seq, body, created_at) VALUES (?, ?, ?, 'compaction', ?, ?, ?)",
        )
        .bind(session_id)
        .bind(seq)
        .bind(run_id)
        .bind(covers_seq)
        .bind(json(&summary))
        .bind(now())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn load_history(&self, session_id: &str) -> Result<History> {
        let compaction: Option<(i64, String)> = sqlx::query_as(
            "SELECT covers_seq, body FROM messages WHERE session_id = ? AND kind = 'compaction' ORDER BY seq DESC LIMIT 1",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
        let (after, summary) = match compaction {
            Some((covers, body)) => (covers, Some(parse::<String>(&body)?)),
            None => (0, None),
        };
        let rows = sqlx::query("SELECT * FROM messages WHERE session_id = ? AND kind = 'message' AND seq > ? ORDER BY seq")
            .bind(session_id)
            .bind(after)
            .fetch_all(&self.pool)
            .await?;
        Ok(History {
            summary,
            messages: rows.iter().map(stored_message).collect::<Result<_>>()?,
        })
    }

    async fn list_messages(&self, session_id: &str) -> Result<Vec<StoredMessage>> {
        let rows = sqlx::query(
            "SELECT * FROM messages WHERE session_id = ? AND kind = 'message' ORDER BY seq",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(stored_message).collect()
    }
}

#[async_trait]
impl RunStore for SqliteStore {
    async fn create_run(&self, new: NewRun) -> Result<Run> {
        let id = new.id.unwrap_or_else(|| gai_core::new_id("run"));
        let ts = now();
        let metadata = if new.metadata.is_null() {
            Value::Object(Default::default())
        } else {
            new.metadata
        };
        sqlx::query(
            "INSERT INTO runs (id, session_id, agent, agent_version, parent_run_id, status, input, metadata, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, 'queued', ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&new.session_id)
        .bind(&new.agent)
        .bind(new.agent_version.map(|v| v as i64))
        .bind(&new.parent_run_id)
        .bind(json(&new.input))
        .bind(json(&metadata))
        .bind(ts)
        .bind(ts)
        .execute(&self.pool)
        .await?;
        self.get_run(&id).await?.ok_or(StoreError::NotFound(id))
    }

    async fn get_run(&self, id: &str) -> Result<Option<Run>> {
        let row = sqlx::query("SELECT * FROM runs WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(run).transpose()
    }

    async fn list_runs(&self, f: RunFilter) -> Result<Vec<Run>> {
        let mut q: QueryBuilder<Sqlite> = QueryBuilder::new("SELECT * FROM runs WHERE 1 = 1");
        if let Some(s) = &f.session_id {
            q.push(" AND session_id = ").push_bind(s);
        }
        if let Some(a) = &f.agent {
            q.push(" AND agent = ").push_bind(a);
        }
        if let Some(s) = f.status {
            q.push(" AND status = ").push_bind(s.as_str());
        }
        if let Some(p) = &f.parent_run_id {
            q.push(" AND parent_run_id = ").push_bind(p);
        }
        if let Some(r) = &f.fork_of {
            q.push(" AND json_extract(metadata, '$.fork.run_id') = ")
                .push_bind(r);
        }
        q.push(" ORDER BY created_at DESC, id DESC LIMIT ")
            .push_bind(f.limit.unwrap_or(50) as i64);
        let rows = q.build().fetch_all(&self.pool).await?;
        rows.iter().map(run).collect()
    }

    async fn update_run(&self, id: &str, u: RunUpdate) -> Result<()> {
        let mut q: QueryBuilder<Sqlite> = QueryBuilder::new("UPDATE runs SET updated_at = ");
        let ts = now();
        q.push_bind(ts);
        if let Some(s) = u.status {
            q.push(", status = ").push_bind(s.as_str());
            if s.is_terminal() {
                q.push(", finished_at = ").push_bind(ts);
            }
        }
        if let Some(o) = &u.output {
            q.push(", output = ").push_bind(json(o));
        }
        if let Some(e) = &u.error {
            q.push(", error = ").push_bind(e);
        }
        if let Some(us) = &u.usage {
            q.push(", usage = ").push_bind(json(us));
        }
        if let Some(c) = u.cost {
            q.push(", cost = ").push_bind(c);
        }
        if let Some(s) = u.steps {
            q.push(", steps = ").push_bind(s as i64);
        }
        q.push(" WHERE id = ").push_bind(id);
        let r = q.build().execute(&self.pool).await?;
        if r.rows_affected() == 0 {
            return Err(StoreError::NotFound(format!("run {id}")));
        }
        Ok(())
    }

    async fn start_step(
        &self,
        run_id: &str,
        seq: u32,
        kind: StepKind,
        name: Option<&str>,
        input: Option<&Value>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO run_steps (run_id, seq, kind, name, status, input, started_at) VALUES (?, ?, ?, ?, 'started', ?, ?)
             ON CONFLICT(run_id, seq) DO UPDATE SET status = 'started', input = excluded.input, started_at = excluded.started_at,
             output = NULL, finished_at = NULL",
        )
        .bind(run_id)
        .bind(seq as i64)
        .bind(enum_str(&kind))
        .bind(name)
        .bind(input.map(json))
        .bind(now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn finish_step(
        &self,
        run_id: &str,
        seq: u32,
        status: StepStatus,
        output: Option<&Value>,
        usage: Option<&Usage>,
        cost: Option<f64>,
    ) -> Result<()> {
        sqlx::query("UPDATE run_steps SET status = ?, output = ?, usage = ?, cost = ?, finished_at = ? WHERE run_id = ? AND seq = ?")
            .bind(enum_str(&status))
            .bind(output.map(json))
            .bind(usage.map(json))
            .bind(cost)
            .bind(now())
            .bind(run_id)
            .bind(seq as i64)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn list_steps(&self, run_id: &str) -> Result<Vec<Step>> {
        let rows = sqlx::query("SELECT * FROM run_steps WHERE run_id = ? ORDER BY seq")
            .bind(run_id)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(step).collect()
    }

    async fn claim_queued_run(&self) -> Result<Option<Run>> {
        let row = sqlx::query(
            "UPDATE runs SET status = 'running', updated_at = ?
             WHERE id = (SELECT id FROM runs
                         WHERE status = 'queued' AND json_extract(metadata, '$.background') = 1
                         ORDER BY created_at, id LIMIT 1)
               AND status = 'queued'
             RETURNING *",
        )
        .bind(now())
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(run).transpose()
    }

    async fn append_event(&self, run_id: &str, seq: u64, event: &Value) -> Result<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO run_events (run_id, seq, event, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(run_id)
        .bind(seq as i64)
        .bind(json(event))
        .bind(now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_events(
        &self,
        run_id: &str,
        after_seq: Option<u64>,
        limit: u32,
    ) -> Result<Vec<StoredEvent>> {
        let rows = sqlx::query("SELECT seq, event, created_at FROM run_events WHERE run_id = ? AND seq > ? ORDER BY seq LIMIT ?")
            .bind(run_id)
            .bind(after_seq.map(|s| s as i64).unwrap_or(-1))
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|r| {
                Ok(StoredEvent {
                    seq: r.try_get::<i64, _>("seq")? as u64,
                    event: parse(&r.try_get::<String, _>("event")?)?,
                    created_at: r.try_get("created_at")?,
                })
            })
            .collect()
    }

    async fn record_usage(
        &self,
        run_id: Option<&str>,
        model: &str,
        usage: &Usage,
        cost: Option<f64>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO usage_log (run_id, model, usage, cost, created_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(run_id)
        .bind(model)
        .bind(json(usage))
        .bind(cost)
        .bind(now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn usage_totals(&self, since: Option<DateTime<Utc>>) -> Result<Vec<UsageTotal>> {
        let rows = sqlx::query(
            "SELECT model, usage, cost FROM usage_log WHERE (?1 IS NULL OR created_at >= ?1)",
        )
        .bind(since)
        .fetch_all(&self.pool)
        .await?;
        let mut by_model: BTreeMap<String, UsageTotal> = BTreeMap::new();
        for r in rows {
            let model: String = r.try_get("model")?;
            let usage: Usage = parse(&r.try_get::<String, _>("usage")?)?;
            let cost: Option<f64> = r.try_get("cost")?;
            let t = by_model.entry(model.clone()).or_insert_with(|| UsageTotal {
                model,
                ..Default::default()
            });
            t.calls += 1;
            t.usage += usage;
            t.cost += cost.unwrap_or(0.0);
        }
        Ok(by_model.into_values().collect())
    }
}

fn api_key(r: &SqliteRow) -> Result<ApiKey> {
    Ok(ApiKey {
        id: r.try_get("id")?,
        name: r.try_get("name")?,
        prefix: r.try_get("prefix")?,
        scopes: parse(&r.try_get::<String, _>("scopes")?)?,
        budget_usd: r.try_get("budget_usd")?,
        spent_usd: r.try_get("spent_usd")?,
        created_at: r.try_get("created_at")?,
        last_used_at: r.try_get("last_used_at")?,
        revoked_at: r.try_get("revoked_at")?,
    })
}

#[async_trait]
impl KeyStore for SqliteStore {
    async fn create_key(
        &self,
        name: &str,
        prefix: &str,
        hash: &str,
        scopes: &[Scope],
        budget_usd: Option<f64>,
    ) -> Result<ApiKey> {
        let id = gai_core::new_id("key");
        sqlx::query(
            "INSERT INTO api_keys (id, name, prefix, hash, scopes, budget_usd, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(name)
        .bind(prefix)
        .bind(hash)
        .bind(json(&scopes))
        .bind(budget_usd)
        .bind(now())
        .execute(&self.pool)
        .await?;
        let row = sqlx::query("SELECT * FROM api_keys WHERE id = ?")
            .bind(&id)
            .fetch_one(&self.pool)
            .await?;
        api_key(&row)
    }

    async fn find_key(&self, hash: &str) -> Result<Option<ApiKey>> {
        let row = sqlx::query("SELECT * FROM api_keys WHERE hash = ? AND revoked_at IS NULL")
            .bind(hash)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(api_key).transpose()
    }

    async fn list_keys(&self) -> Result<Vec<ApiKey>> {
        let rows = sqlx::query("SELECT * FROM api_keys ORDER BY created_at DESC")
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(api_key).collect()
    }

    async fn revoke_key(&self, id: &str) -> Result<bool> {
        let r =
            sqlx::query("UPDATE api_keys SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL")
                .bind(now())
                .bind(id)
                .execute(&self.pool)
                .await?;
        Ok(r.rows_affected() > 0)
    }

    async fn touch_key(&self, id: &str) -> Result<()> {
        sqlx::query("UPDATE api_keys SET last_used_at = ? WHERE id = ?")
            .bind(now())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn add_key_spend(&self, id: &str, usd: f64) -> Result<()> {
        sqlx::query("UPDATE api_keys SET spent_usd = spent_usd + ? WHERE id = ?")
            .bind(usd)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn has_keys(&self) -> Result<bool> {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM api_keys WHERE revoked_at IS NULL")
            .fetch_one(&self.pool)
            .await?;
        Ok(n > 0)
    }
}

#[async_trait]
impl McpStore for SqliteStore {
    async fn put_mcp_server(&self, name: &str, config: &Value) -> Result<()> {
        let ts = now();
        sqlx::query(
            "INSERT INTO mcp_servers (name, config, created_at, updated_at) VALUES (?, ?, ?, ?)
             ON CONFLICT(name) DO UPDATE SET config = excluded.config, updated_at = excluded.updated_at",
        )
        .bind(name)
        .bind(json(config))
        .bind(ts)
        .bind(ts)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_mcp_server(&self, name: &str) -> Result<bool> {
        let r = sqlx::query("DELETE FROM mcp_servers WHERE name = ?")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    async fn list_mcp_servers(&self) -> Result<Vec<(String, Value)>> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT name, config FROM mcp_servers ORDER BY name")
                .fetch_all(&self.pool)
                .await?;
        rows.into_iter().map(|(n, c)| Ok((n, parse(&c)?))).collect()
    }

    async fn get_mcp_oauth(&self, server: &str) -> Result<Option<Value>> {
        let row: Option<String> =
            sqlx::query_scalar("SELECT credentials FROM mcp_oauth WHERE server = ?")
                .bind(server)
                .fetch_optional(&self.pool)
                .await?;
        row.map(|c| parse(&c)).transpose()
    }

    async fn put_mcp_oauth(&self, server: &str, credentials: &Value) -> Result<()> {
        sqlx::query(
            "INSERT INTO mcp_oauth (server, credentials, updated_at) VALUES (?, ?, ?)
             ON CONFLICT(server) DO UPDATE SET credentials = excluded.credentials, updated_at = excluded.updated_at",
        )
        .bind(server)
        .bind(json(credentials))
        .bind(now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_mcp_oauth(&self, server: &str) -> Result<()> {
        sqlx::query("DELETE FROM mcp_oauth WHERE server = ?")
            .bind(server)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[async_trait]
impl SettingsStore for SqliteStore {
    async fn get_setting(&self, name: &str) -> Result<Option<String>> {
        Ok(
            sqlx::query_scalar("SELECT value FROM settings WHERE name = ?")
                .bind(name)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    async fn put_setting(&self, name: &str, value: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO settings (name, value, updated_at) VALUES (?, ?, ?)
             ON CONFLICT(name) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(name)
        .bind(value)
        .bind(now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_setting(&self, name: &str) -> Result<bool> {
        let r = sqlx::query("DELETE FROM settings WHERE name = ?")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    async fn list_settings(&self, prefix: &str) -> Result<Vec<(String, String)>> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT name, value FROM settings WHERE substr(name, 1, length(?1)) = ?1 ORDER BY name",
        )
        .bind(prefix)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod conformance {
        use std::sync::Arc;

        use super::*;

        crate::conformance_tests!(async {
            Some(Arc::new(SqliteStore::temporary().await.unwrap()) as SharedStore)
        });
    }

    use serde_json::json;

    #[tokio::test]
    async fn file_database_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/gai.db");
        {
            let s = SqliteStore::open(&path).await.unwrap();
            s.put_agent("x", &json!({}), None).await.unwrap();
        }
        let s = SqliteStore::open(&path).await.unwrap();
        assert_eq!(s.list_agents().await.unwrap().len(), 1);
    }
}
