//! Postgres implementation of source storage.

use async_trait::async_trait;
use serde_json::Value;
use sqlx::Row;
use sqlx::postgres::PgRow;

use super::{PostgresStore, json, now, parse};
use crate::*;

fn source(r: &PgRow) -> Result<SourceRecord> {
    Ok(SourceRecord {
        name: r.try_get("name")?,
        collection: r.try_get("collection")?,
        kind: r.try_get("kind")?,
        spec: parse(&r.try_get::<String, _>("spec")?)?,
        state: parse(&r.try_get::<String, _>("state")?)?,
        status: r.try_get("status")?,
        error: r.try_get("error")?,
        stats: parse(&r.try_get::<String, _>("stats")?)?,
        last_sync_at: r.try_get("last_sync_at")?,
        created_at: r.try_get("created_at")?,
        updated_at: r.try_get("updated_at")?,
    })
}

#[async_trait]
impl SourceStore for PostgresStore {
    async fn put_source(
        &self,
        name: &str,
        collection: &str,
        kind: &str,
        spec: &Value,
    ) -> Result<()> {
        let ts = now();
        sqlx::query(
            "INSERT INTO sources (name, collection, kind, spec, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT(name) DO UPDATE SET collection = excluded.collection, kind = excluded.kind,
                 spec = excluded.spec, updated_at = excluded.updated_at",
        )
        .bind(name)
        .bind(collection)
        .bind(kind)
        .bind(json(spec))
        .bind(ts)
        .bind(ts)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_source(&self, name: &str) -> Result<Option<SourceRecord>> {
        let row = sqlx::query("SELECT * FROM sources WHERE name = $1")
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(source).transpose()
    }

    async fn list_sources(&self) -> Result<Vec<SourceRecord>> {
        let rows = sqlx::query("SELECT * FROM sources ORDER BY name")
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(source).collect()
    }

    async fn delete_source(&self, name: &str) -> Result<bool> {
        let r = sqlx::query("DELETE FROM sources WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    async fn save_source_sync(
        &self,
        name: &str,
        status: &str,
        error: Option<&str>,
        stats: &Value,
        state: Option<&Value>,
    ) -> Result<()> {
        let ts = now();
        let r = sqlx::query(
            "UPDATE sources SET status = $1, error = $2, stats = $3, state = COALESCE($4, state),
                 last_sync_at = CASE WHEN $5::TEXT = 'syncing' THEN last_sync_at ELSE $6 END, updated_at = $7
             WHERE name = $8",
        )
        .bind(status)
        .bind(error)
        .bind(json(stats))
        .bind(state.map(json))
        .bind(status)
        .bind(ts)
        .bind(ts)
        .bind(name)
        .execute(&self.pool)
        .await?;
        if r.rows_affected() == 0 {
            return Err(StoreError::NotFound(format!("source {name}")));
        }
        Ok(())
    }
}
