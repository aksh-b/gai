//! Postgres storage (the `postgres` feature). Groundwork: versioned
//! agents, settings and MCP servers are implemented and pass the same
//! [`conformance`](crate::conformance) checks as SQLite. The other
//! repositories (sessions, runs, keys, memory, knowledge, skills,
//! workflows, personas, blobs, triggers) are not ported yet, so a
//! `PostgresStore` is not a full [`Store`](crate::Store) and gai still runs
//! on SQLite.

use std::collections::BTreeMap;
use std::str::FromStr;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::Value;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions, PgRow};
use sqlx::{Row, types::Json};

use crate::{AgentStore, AgentSummary, AgentVersion, McpStore, Result, SettingsStore, StoreError};

#[derive(Clone)]
pub struct PostgresStore {
    pool: PgPool,
}

impl PostgresStore {
    /// Connect (`postgres://user:pass@host/db`) and bring the schema up to
    /// date.
    pub async fn connect(url: &str) -> Result<Self> {
        let opts =
            PgConnectOptions::from_str(url).map_err(|e| StoreError::Database(e.to_string()))?;
        Self::connect_with(opts).await
    }

    /// Like [`connect`](Self::connect), with options (e.g. a `search_path`).
    pub async fn connect_with(opts: PgConnectOptions) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect_with(opts)
            .await?;
        sqlx::migrate!("migrations/postgres")
            .run(&pool)
            .await
            .map_err(|e| StoreError::Database(format!("migrations: {e}")))?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

fn agent_version(r: &PgRow) -> Result<AgentVersion> {
    Ok(AgentVersion {
        name: r.try_get("name")?,
        version: r.try_get::<i32, _>("version")? as u32,
        spec: r.try_get::<Json<Value>, _>("spec")?.0,
        note: r.try_get("note")?,
        created_at: r.try_get("created_at")?,
    })
}

#[async_trait]
impl AgentStore for PostgresStore {
    async fn put_agent(&self, name: &str, spec: &Value, note: Option<&str>) -> Result<u32> {
        let mut tx = self.pool.begin().await?;
        // Saves of one agent take turns.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
            .bind(format!("agent:{name}"))
            .execute(&mut *tx)
            .await?;
        let latest: Option<(i32, Json<Value>)> = sqlx::query_as(
            "SELECT v.version, v.spec FROM agents a JOIN agent_versions v ON v.name = a.name AND v.version = a.latest_version WHERE a.name = $1",
        )
        .bind(name)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((v, s)) = &latest
            && s.0 == *spec
        {
            return Ok(*v as u32);
        }
        let version = latest.map(|(v, _)| v + 1).unwrap_or(1);
        let ts = Utc::now();
        sqlx::query(
            "INSERT INTO agents (name, latest_version, created_at, updated_at) VALUES ($1, $2, $3, $3)
             ON CONFLICT (name) DO UPDATE SET latest_version = EXCLUDED.latest_version, updated_at = EXCLUDED.updated_at",
        )
        .bind(name)
        .bind(version)
        .bind(ts)
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO agent_versions (name, version, spec, note, created_at) VALUES ($1, $2, $3, $4, $5)")
            .bind(name)
            .bind(version)
            .bind(Json(spec))
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
                sqlx::query("SELECT * FROM agent_versions WHERE name = $1 AND version = $2")
                    .bind(name)
                    .bind(v as i32)
                    .fetch_optional(&self.pool)
                    .await?
            }
            None => {
                sqlx::query(
                    "SELECT v.* FROM agents a JOIN agent_versions v ON v.name = a.name AND v.version = a.latest_version WHERE a.name = $1",
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
                    latest_version: r.try_get::<i32, _>("latest_version")? as u32,
                    labels: r.try_get::<Json<BTreeMap<String, u32>>, _>("labels")?.0,
                    created_at: r.try_get("created_at")?,
                    updated_at: r.try_get("updated_at")?,
                })
            })
            .collect()
    }

    async fn list_agent_versions(&self, name: &str) -> Result<Vec<AgentVersion>> {
        let rows =
            sqlx::query("SELECT * FROM agent_versions WHERE name = $1 ORDER BY version DESC")
                .bind(name)
                .fetch_all(&self.pool)
                .await?;
        rows.iter().map(agent_version).collect()
    }

    async fn set_agent_label(&self, name: &str, label: &str, version: Option<u32>) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let labels: Option<Json<BTreeMap<String, u32>>> =
            sqlx::query_scalar("SELECT labels FROM agents WHERE name = $1 FOR UPDATE")
                .bind(name)
                .fetch_optional(&mut *tx)
                .await?;
        let mut labels = labels
            .ok_or_else(|| StoreError::NotFound(format!("agent {name}")))?
            .0;
        match version {
            Some(v) => {
                let exists: Option<i32> = sqlx::query_scalar(
                    "SELECT 1 FROM agent_versions WHERE name = $1 AND version = $2",
                )
                .bind(name)
                .bind(v as i32)
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
        sqlx::query("UPDATE agents SET labels = $1, updated_at = $2 WHERE name = $3")
            .bind(Json(&labels))
            .bind(Utc::now())
            .bind(name)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn delete_agent(&self, name: &str) -> Result<bool> {
        let r = sqlx::query("DELETE FROM agents WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }
}

#[async_trait]
impl SettingsStore for PostgresStore {
    async fn get_setting(&self, name: &str) -> Result<Option<String>> {
        Ok(
            sqlx::query_scalar("SELECT value FROM settings WHERE name = $1")
                .bind(name)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    async fn put_setting(&self, name: &str, value: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO settings (name, value, updated_at) VALUES ($1, $2, $3)
             ON CONFLICT (name) DO UPDATE SET value = EXCLUDED.value, updated_at = EXCLUDED.updated_at",
        )
        .bind(name)
        .bind(value)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_setting(&self, name: &str) -> Result<bool> {
        let r = sqlx::query("DELETE FROM settings WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    async fn list_settings(&self, prefix: &str) -> Result<Vec<(String, String)>> {
        Ok(sqlx::query_as(
            "SELECT name, value FROM settings WHERE starts_with(name, $1) ORDER BY name",
        )
        .bind(prefix)
        .fetch_all(&self.pool)
        .await?)
    }
}

#[async_trait]
impl McpStore for PostgresStore {
    async fn put_mcp_server(&self, name: &str, config: &Value) -> Result<()> {
        let ts = Utc::now();
        sqlx::query(
            "INSERT INTO mcp_servers (name, config, created_at, updated_at) VALUES ($1, $2, $3, $3)
             ON CONFLICT (name) DO UPDATE SET config = EXCLUDED.config, updated_at = EXCLUDED.updated_at",
        )
        .bind(name)
        .bind(Json(config))
        .bind(ts)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_mcp_server(&self, name: &str) -> Result<bool> {
        let r = sqlx::query("DELETE FROM mcp_servers WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    async fn list_mcp_servers(&self) -> Result<Vec<(String, Value)>> {
        let rows: Vec<(String, Json<Value>)> =
            sqlx::query_as("SELECT name, config FROM mcp_servers ORDER BY name")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(n, c)| (n, c.0)).collect())
    }

    async fn get_mcp_oauth(&self, server: &str) -> Result<Option<Value>> {
        let v: Option<Json<Value>> =
            sqlx::query_scalar("SELECT credentials FROM mcp_oauth WHERE server = $1")
                .bind(server)
                .fetch_optional(&self.pool)
                .await?;
        Ok(v.map(|j| j.0))
    }

    async fn put_mcp_oauth(&self, server: &str, credentials: &Value) -> Result<()> {
        sqlx::query(
            "INSERT INTO mcp_oauth (server, credentials, updated_at) VALUES ($1, $2, $3)
             ON CONFLICT (server) DO UPDATE SET credentials = EXCLUDED.credentials, updated_at = EXCLUDED.updated_at",
        )
        .bind(server)
        .bind(Json(credentials))
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_mcp_oauth(&self, server: &str) -> Result<()> {
        sqlx::query("DELETE FROM mcp_oauth WHERE server = $1")
            .bind(server)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
