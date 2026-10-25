//! SQLite implementation of the audit log, retention and forgetting users.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::Row;

use super::{SqliteStore, json, now, parse};
use crate::*;

#[async_trait]
impl GovernanceStore for SqliteStore {
    async fn add_audit(&self, e: NewAudit) -> Result<i64> {
        let detail = if e.detail.is_null() {
            serde_json::json!({})
        } else {
            e.detail
        };
        let r = sqlx::query(
            "INSERT INTO audit_log (at, actor, actor_id, action, target, status, detail) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(now())
        .bind(&e.actor)
        .bind(&e.actor_id)
        .bind(&e.action)
        .bind(&e.target)
        .bind(e.status as i64)
        .bind(json(&detail))
        .execute(&self.pool)
        .await?;
        Ok(r.last_insert_rowid())
    }

    async fn list_audit(&self, f: AuditFilter) -> Result<Vec<AuditEntry>> {
        let rows = sqlx::query(
            "SELECT * FROM audit_log
             WHERE (?1 IS NULL OR actor = ?1)
               AND (?2 IS NULL OR substr(target, 1, length(?2)) = ?2)
               AND (?3 IS NULL OR at >= ?3)
               AND (?4 IS NULL OR at < ?4)
             ORDER BY id DESC LIMIT ?5",
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
        let where_ = "agent = ?1 AND updated_at < ?2";
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
            "SELECT COUNT(*) FROM runs WHERE session_id IN (SELECT id FROM sessions WHERE user_id = ?1)",
        )
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
        let blobs = sqlx::query(
            "DELETE FROM blobs WHERE run_id IN (SELECT r.id FROM runs r JOIN sessions s ON s.id = r.session_id WHERE s.user_id = ?1)",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        // Ratings on other users' runs that this user gave.
        let feedback = sqlx::query("DELETE FROM feedback WHERE user_id = ?1")
            .bind(user_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        let sessions = sqlx::query("DELETE FROM sessions WHERE user_id = ?1")
            .bind(user_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        let memory_blocks = sqlx::query("DELETE FROM memory_blocks WHERE scope = ?1")
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

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn audit_entries_filter_by_actor_target_and_time() {
        let s = SqliteStore::temporary().await.unwrap();
        for (actor, target) in [
            ("ada", "agents/a"),
            ("ada", "keys/k1"),
            ("alan", "agents/b"),
        ] {
            s.add_audit(NewAudit {
                actor: actor.into(),
                actor_id: format!("key_{actor}"),
                action: "POST /v1/x".into(),
                target: target.into(),
                status: 200,
                detail: json!({"n": 1}),
            })
            .await
            .unwrap();
        }
        let all = s.list_audit(AuditFilter::default()).await.unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].actor, "alan", "newest first");
        let ada = s
            .list_audit(AuditFilter {
                actor: Some("ada".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(ada.len(), 2);
        let agents = s
            .list_audit(AuditFilter {
                target: Some("agents/".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(agents.len(), 2);
        let later = s
            .list_audit(AuditFilter {
                since: Some(Utc::now() + Duration::hours(1)),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(later.is_empty());
    }

    #[tokio::test]
    async fn pruning_and_forgetting() {
        let s = SqliteStore::temporary().await.unwrap();
        let mk = |agent: &str, user: &str| NewSession {
            agent: Some(agent.into()),
            user_id: Some(user.into()),
            ..Default::default()
        };
        let old = s.create_session(mk("support", "ada")).await.unwrap();
        let keep = s.create_session(mk("other", "ada")).await.unwrap();
        let alan = s.create_session(mk("support", "alan")).await.unwrap();
        for ses in [&old, &keep, &alan] {
            let run = s
                .create_run(NewRun {
                    session_id: ses.id.clone(),
                    agent: ses.agent.clone().unwrap(),
                    input: json!("hi"),
                    ..Default::default()
                })
                .await
                .unwrap();
            s.put_blob(NewBlob {
                media_type: "text/plain".into(),
                name: None,
                run_id: Some(run.id.clone()),
                source: None,
                metadata: json!({}),
                data: b"x".to_vec(),
            })
            .await
            .unwrap();
            s.put_feedback(&run, "ada", 1, None, &[]).await.unwrap();
        }
        s.create_block(
            NewBlock {
                scope: "user:ada".into(),
                label: "human".into(),
                value: "likes tea".into(),
                limit: 100,
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
        assert_eq!(s.session_agents().await.unwrap(), ["other", "support"]);

        // Nothing is old enough yet.
        let none = s
            .prune_sessions("support", Utc::now() - Duration::days(1))
            .await
            .unwrap();
        assert_eq!(none, Deleted::default());
        let pruned = s
            .prune_sessions("support", Utc::now() + Duration::seconds(1))
            .await
            .unwrap();
        assert_eq!((pruned.sessions, pruned.runs, pruned.blobs), (2, 2, 2));
        assert!(s.get_session(&keep.id).await.unwrap().is_some());

        let gone = s.forget_user("ada").await.unwrap();
        assert_eq!(
            (gone.sessions, gone.runs, gone.blobs, gone.memory_blocks),
            (1, 1, 1, 1)
        );
        assert!(gone.feedback >= 1);
        assert!(s.get_session(&keep.id).await.unwrap().is_none());
        assert!(s.find_block("user:ada", "human").await.unwrap().is_none());
    }
}
