//! Postgres implementation of the audit log, retention and forgetting users.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::Row;

use super::{PostgresStore, json, now, parse};
use crate::*;

#[async_trait]
impl GovernanceStore for PostgresStore {
    async fn add_audit(&self, e: NewAudit) -> Result<i64> {
        let detail = if e.detail.is_null() {
            serde_json::json!({})
        } else {
            e.detail
        };
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO audit_log (at, actor, actor_id, action, target, status, detail) VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
        )
        .bind(now())
        .bind(&e.actor)
        .bind(&e.actor_id)
        .bind(&e.action)
        .bind(&e.target)
        .bind(e.status as i64)
        .bind(json(&detail))
        .fetch_one(&self.pool)
        .await?;
        Ok(id)
    }

    async fn list_audit(&self, f: AuditFilter) -> Result<Vec<AuditEntry>> {
        let rows = sqlx::query(
            "SELECT * FROM audit_log
             WHERE ($1 IS NULL OR actor = $1)
               AND ($2 IS NULL OR substr(target, 1, length($2)) = $2)
               AND ($3 IS NULL OR at >= $3)
               AND ($4 IS NULL OR at < $4)
             ORDER BY id DESC LIMIT $5",
        )
        .bind(f.actor)
        .bind(f.target)
        .bind(f.since)
        .bind(f.until)
        .bind(if f.limit == 0 { 100 } else { f.limit as i64 })
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok(AuditEntry {
                    id: r.try_get("id")?,
                    at: r.try_get("at")?,
                    actor: r.try_get("actor")?,
                    actor_id: r.try_get("actor_id")?,
                    action: r.try_get("action")?,
                    target: r.try_get("target")?,
                    status: r.try_get::<i64, _>("status")? as u16,
                    detail: parse(&r.try_get::<String, _>("detail")?)?,
                })
            })
            .collect()
    }

    async fn session_agents(&self) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT DISTINCT agent FROM sessions WHERE agent IS NOT NULL ORDER BY agent",
        )
        .fetch_all(&self.pool)
        .await?)
    }

    async fn prune_sessions(&self, agent: &str, before: DateTime<Utc>) -> Result<Deleted> {
        let mut tx = self.write_tx().await?;
        let where_ = "agent = $1 AND updated_at < $2";
        let runs: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM runs WHERE session_id IN (SELECT id FROM sessions WHERE {where_})"
        ))
        .bind(agent)
        .bind(before)
        .fetch_one(&mut *tx)
        .await?;
        let blobs = sqlx::query(&format!(
            "DELETE FROM blobs WHERE run_id IN (SELECT id FROM runs WHERE session_id IN (SELECT id FROM sessions WHERE {where_}))"
        ))
        .bind(agent)
        .bind(before)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        let sessions = sqlx::query(&format!("DELETE FROM sessions WHERE {where_}"))
            .bind(agent)
            .bind(before)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(Deleted {
            sessions,
            runs: runs as u64,
            blobs,
            ..Default::default()
        })
    }

    async fn forget_user(&self, user_id: &str) -> Result<Deleted> {
        let mut tx = self.write_tx().await?;
        let runs: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM runs WHERE session_id IN (SELECT id FROM sessions WHERE user_id = $1)",
        )
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
        let blobs = sqlx::query(
            "DELETE FROM blobs WHERE run_id IN (SELECT r.id FROM runs r JOIN sessions s ON s.id = r.session_id WHERE s.user_id = $1)",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        // Ratings on other users' runs that this user gave.
        let feedback = sqlx::query("DELETE FROM feedback WHERE user_id = $1")
            .bind(user_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        let sessions = sqlx::query("DELETE FROM sessions WHERE user_id = $1")
            .bind(user_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        let memory_blocks = sqlx::query("DELETE FROM memory_blocks WHERE scope = $1")
            .bind(format!("user:{user_id}"))
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(Deleted {
            sessions,
            runs: runs as u64,
            blobs,
            memory_blocks,
            feedback,
        })
    }
}
