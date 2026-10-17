//! SQLite implementation of trigger storage.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use super::{SqliteStore, json, now, opt_parse, parse};
use crate::*;

fn trigger(r: &SqliteRow) -> Result<TriggerRecord> {
    Ok(TriggerRecord {
        id: r.try_get("id")?,
        name: r.try_get("name")?,
        kind: r.try_get("kind")?,
        enabled: r.try_get::<i64, _>("enabled")? != 0,
        spec: parse(&r.try_get::<String, _>("spec")?)?,
        secret: r.try_get("secret")?,
        next_fire_at: r.try_get("next_fire_at")?,
        last_fired_at: r.try_get("last_fired_at")?,
        created_at: r.try_get("created_at")?,
        updated_at: r.try_get("updated_at")?,
    })
}

fn event(r: &SqliteRow) -> Result<TriggerEvent> {
    Ok(TriggerEvent {
        id: r.try_get("id")?,
        trigger_id: r.try_get("trigger_id")?,
        fired_at: r.try_get("fired_at")?,
        reason: r.try_get("reason")?,
        payload: opt_parse(r.try_get("payload")?)?,
        run_id: r.try_get("run_id")?,
        status: r.try_get("status")?,
        error: r.try_get("error")?,
    })
}

#[async_trait]
impl TriggerStore for SqliteStore {
    async fn create_trigger(
        &self,
        name: &str,
        kind: &str,
        spec: &Value,
        secret: Option<&str>,
    ) -> Result<TriggerRecord> {
        let ts = now();
        let id = gai_core::new_id("trg");
        let r = sqlx::query(
            "INSERT INTO triggers (id, name, kind, enabled, spec, secret, created_at, updated_at)
             VALUES (?, ?, ?, 1, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(name)
        .bind(kind)
        .bind(json(spec))
        .bind(secret)
        .bind(ts)
        .bind(ts)
        .execute(&self.pool)
        .await;
        match r {
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                return Err(StoreError::Conflict(format!(
                    "a trigger named {name} exists"
                )));
            }
            r => {
                r?;
            }
        }
        let found = self.get_trigger(&id).await?;
        found.ok_or(StoreError::NotFound(id))
    }

    async fn update_trigger(
        &self,
        id: &str,
        name: &str,
        kind: &str,
        spec: &Value,
    ) -> Result<TriggerRecord> {
        let r = sqlx::query(
            "UPDATE triggers SET name = ?, kind = ?, spec = ?, updated_at = ? WHERE id = ?",
        )
        .bind(name)
        .bind(kind)
        .bind(json(spec))
        .bind(now())
        .bind(id)
        .execute(&self.pool)
        .await;
        match r {
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                return Err(StoreError::Conflict(format!(
                    "a trigger named {name} exists"
                )));
            }
            Ok(r) if r.rows_affected() == 0 => {
                return Err(StoreError::NotFound(format!("trigger {id}")));
            }
            r => {
                r?;
            }
        }
        self.get_trigger(id)
            .await?
            .ok_or_else(|| StoreError::NotFound(id.to_owned()))
    }

    async fn get_trigger(&self, id_or_name: &str) -> Result<Option<TriggerRecord>> {
        let row = sqlx::query("SELECT * FROM triggers WHERE id = ? OR name = ?")
            .bind(id_or_name)
            .bind(id_or_name)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(trigger).transpose()
    }

    async fn list_triggers(&self) -> Result<Vec<TriggerRecord>> {
        let rows = sqlx::query("SELECT * FROM triggers ORDER BY name")
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(trigger).collect()
    }

    async fn delete_trigger(&self, id: &str) -> Result<bool> {
        let r = sqlx::query("DELETE FROM triggers WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    async fn set_trigger_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        sqlx::query("UPDATE triggers SET enabled = ?, updated_at = ? WHERE id = ?")
            .bind(enabled as i64)
            .bind(now())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn set_trigger_next(&self, id: &str, next: Option<DateTime<Utc>>) -> Result<()> {
        sqlx::query("UPDATE triggers SET next_fire_at = ? WHERE id = ?")
            .bind(next)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn advance_trigger(
        &self,
        id: &str,
        expected: DateTime<Utc>,
        next: Option<DateTime<Utc>>,
        fired: DateTime<Utc>,
    ) -> Result<bool> {
        let r = sqlx::query(
            "UPDATE triggers SET next_fire_at = ?, last_fired_at = ? WHERE id = ? AND next_fire_at = ?",
        )
        .bind(next)
        .bind(fired)
        .bind(id)
        .bind(expected)
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected() == 1)
    }

    async fn touch_trigger(&self, id: &str, fired: DateTime<Utc>) -> Result<()> {
        sqlx::query("UPDATE triggers SET last_fired_at = ? WHERE id = ?")
            .bind(fired)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn add_trigger_event(&self, e: NewTriggerEvent) -> Result<i64> {
        let r = sqlx::query(
            "INSERT INTO trigger_events (trigger_id, fired_at, reason, payload, run_id, status, error)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&e.trigger_id)
        .bind(now())
        .bind(&e.reason)
        .bind(e.payload.as_ref().map(json))
        .bind(&e.run_id)
        .bind(&e.status)
        .bind(&e.error)
        .execute(&self.pool)
        .await?;
        Ok(r.last_insert_rowid())
    }

    async fn list_trigger_events(&self, trigger_id: &str, limit: u32) -> Result<Vec<TriggerEvent>> {
        let rows = sqlx::query(
            "SELECT * FROM trigger_events WHERE trigger_id = ? ORDER BY id DESC LIMIT ?",
        )
        .bind(trigger_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(event).collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn triggers_fire_once_per_due_time() {
        let s = SqliteStore::temporary().await.unwrap();
        let t = s
            .create_trigger("nightly", "cron", &json!({"schedule": "0 2 * * *"}), None)
            .await
            .unwrap();
        assert!(t.enabled && t.next_fire_at.is_none());
        assert!(
            s.create_trigger("nightly", "cron", &json!({}), None)
                .await
                .is_err()
        );
        let due = Utc::now();
        s.set_trigger_next(&t.id, Some(due)).await.unwrap();
        let next = due + chrono::Duration::days(1);
        // Two schedulers race for the same firing: only one wins.
        assert!(
            s.advance_trigger(&t.id, due, Some(next), due)
                .await
                .unwrap()
        );
        assert!(
            !s.advance_trigger(&t.id, due, Some(next), due)
                .await
                .unwrap()
        );
        let got = s.get_trigger("nightly").await.unwrap().unwrap();
        assert_eq!(got.next_fire_at, Some(next));
        assert!(got.last_fired_at.is_some());

        s.add_trigger_event(NewTriggerEvent {
            trigger_id: t.id.clone(),
            reason: "schedule".into(),
            payload: Some(json!({"n": 1})),
            run_id: Some("run_x".into()),
            status: "started".into(),
            error: None,
        })
        .await
        .unwrap();
        let events = s.list_trigger_events(&t.id, 10).await.unwrap();
        assert_eq!(events[0].payload, Some(json!({"n": 1})));

        let renamed = s
            .update_trigger(
                &t.id,
                "nightly-2",
                "cron",
                &json!({"schedule": "0 3 * * *"}),
            )
            .await
            .unwrap();
        assert_eq!(renamed.name, "nightly-2");
        s.set_trigger_enabled(&t.id, false).await.unwrap();
        assert!(!s.get_trigger(&t.id).await.unwrap().unwrap().enabled);
        assert!(s.delete_trigger(&t.id).await.unwrap());
        assert!(s.list_trigger_events(&t.id, 10).await.unwrap().is_empty());
    }
}
