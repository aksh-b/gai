//! SQLite implementation of apps.

use async_trait::async_trait;
use serde_json::Value;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use super::{SqliteStore, json, now, parse};
use crate::*;

fn app(r: &SqliteRow) -> Result<AppRecord> {
    Ok(AppRecord {
        name: r.try_get("name")?,
        spec: parse(&r.try_get::<String, _>("spec")?)?,
        key_hash: r.try_get("key_hash")?,
        key_prefix: r.try_get("key_prefix")?,
        created_at: r.try_get("created_at")?,
        updated_at: r.try_get("updated_at")?,
    })
}

#[async_trait]
impl AppStore for SqliteStore {
    async fn put_app(&self, name: &str, spec: &Value) -> Result<()> {
        let ts = now();
        sqlx::query(
            "INSERT INTO apps (name, spec, created_at, updated_at) VALUES (?, ?, ?, ?)
             ON CONFLICT (name) DO UPDATE SET spec = excluded.spec, updated_at = excluded.updated_at",
        )
        .bind(name)
        .bind(json(spec))
        .bind(ts)
        .bind(ts)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_app(&self, name: &str) -> Result<Option<AppRecord>> {
        let row = sqlx::query("SELECT * FROM apps WHERE name = ?")
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(app).transpose()
    }

    async fn list_apps(&self) -> Result<Vec<AppRecord>> {
        let rows = sqlx::query("SELECT * FROM apps ORDER BY name")
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(app).collect()
    }

    async fn delete_app(&self, name: &str) -> Result<bool> {
        let r = sqlx::query("DELETE FROM apps WHERE name = ?")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    async fn set_app_key(&self, name: &str, key: Option<(&str, &str)>) -> Result<()> {
        let r = sqlx::query(
            "UPDATE apps SET key_hash = ?, key_prefix = ?, updated_at = ? WHERE name = ?",
        )
        .bind(key.map(|k| k.0))
        .bind(key.map(|k| k.1))
        .bind(now())
        .bind(name)
        .execute(&self.pool)
        .await?;
        if r.rows_affected() == 0 {
            return Err(StoreError::NotFound(format!("app {name}")));
        }
        Ok(())
    }
}
