//! SQLite implementation of recordings.

use async_trait::async_trait;
use serde_json::Value;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use super::{SqliteStore, json, now, parse};
use crate::*;

fn recording(r: &SqliteRow) -> Result<Recording> {
    Ok(Recording {
        id: r.try_get("id")?,
        kind: r.try_get("kind")?,
        title: r.try_get("title")?,
        blob_id: r.try_get("blob_id")?,
        status: r.try_get("status")?,
        options: parse(&r.try_get::<String, _>("options")?)?,
        result: r
            .try_get::<Option<String>, _>("result")?
            .map(|s| parse(&s))
            .transpose()?,
        error: r.try_get("error")?,
        created_at: r.try_get("created_at")?,
        updated_at: r.try_get("updated_at")?,
    })
}

#[async_trait]
impl RecordingStore for SqliteStore {
    async fn create_recording(&self, new: NewRecording) -> Result<Recording> {
        let ts = now();
        let id = gai_core::new_id("rec");
        sqlx::query(
            "INSERT INTO recordings (id, kind, title, blob_id, status, options, created_at, updated_at)
             VALUES (?, ?, ?, ?, 'processing', ?, ?, ?)",
        )
        .bind(&id)
        .bind(&new.kind)
        .bind(&new.title)
        .bind(&new.blob_id)
        .bind(json(&new.options))
        .bind(ts)
        .bind(ts)
        .execute(&self.pool)
        .await?;
        Ok(Recording {
            id,
            kind: new.kind,
            title: new.title,
            blob_id: new.blob_id,
            status: "processing".into(),
            options: new.options,
            result: None,
            error: None,
            created_at: ts,
            updated_at: ts,
        })
    }

    async fn get_recording(&self, id: &str) -> Result<Option<Recording>> {
        let row = sqlx::query("SELECT * FROM recordings WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(recording).transpose()
    }

    async fn list_recordings(&self, limit: u32) -> Result<Vec<Recording>> {
        let rows =
            sqlx::query("SELECT * FROM recordings ORDER BY created_at DESC, id DESC LIMIT ?")
                .bind(limit as i64)
                .fetch_all(&self.pool)
                .await?;
        rows.iter().map(recording).collect()
    }

    async fn finish_recording(
        &self,
        id: &str,
        status: &str,
        result: Option<&Value>,
        error: Option<&str>,
    ) -> Result<()> {
        let r = sqlx::query(
            "UPDATE recordings SET status = ?, result = ?, error = ?, updated_at = ? WHERE id = ?",
        )
        .bind(status)
        .bind(result.map(json))
        .bind(error)
        .bind(now())
        .bind(id)
        .execute(&self.pool)
        .await?;
        if r.rows_affected() == 0 {
            return Err(StoreError::NotFound(format!("recording {id}")));
        }
        Ok(())
    }

    async fn delete_recording(&self, id: &str) -> Result<bool> {
        let r = sqlx::query("DELETE FROM recordings WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }
}
