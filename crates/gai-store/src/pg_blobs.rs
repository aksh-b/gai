//! Postgres implementation of blob storage: bytes live in the database next
//! to their metadata, so a backup of the database is a backup of the media.

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use sqlx::Row;
use sqlx::postgres::PgRow;

use super::{PostgresStore, json, now, parse};
use crate::*;

/// Every column but the bytes.
const META: &str = "id, media_type, size, sha256, name, run_id, source, metadata, created_at";

fn blob(r: &PgRow) -> Result<Blob> {
    Ok(Blob {
        id: r.try_get("id")?,
        media_type: r.try_get("media_type")?,
        size: r.try_get::<i64, _>("size")? as u64,
        sha256: r.try_get("sha256")?,
        name: r.try_get("name")?,
        run_id: r.try_get("run_id")?,
        source: r.try_get("source")?,
        metadata: parse(&r.try_get::<String, _>("metadata")?)?,
        created_at: r.try_get("created_at")?,
    })
}

#[async_trait]
impl BlobStore for PostgresStore {
    async fn put_blob(&self, new: NewBlob) -> Result<Blob> {
        let digest = Sha256::digest(&new.data);
        let sha256: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        let b = Blob {
            id: gai_core::new_id("blob"),
            media_type: new.media_type,
            size: new.data.len() as u64,
            sha256,
            name: new.name,
            run_id: new.run_id,
            source: new.source,
            metadata: if new.metadata.is_null() {
                serde_json::json!({})
            } else {
                new.metadata
            },
            created_at: now(),
        };
        sqlx::query(
            "INSERT INTO blobs (id, media_type, size, sha256, name, run_id, source, metadata, data, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(&b.id)
        .bind(&b.media_type)
        .bind(b.size as i64)
        .bind(&b.sha256)
        .bind(&b.name)
        .bind(&b.run_id)
        .bind(&b.source)
        .bind(json(&b.metadata))
        .bind(&new.data)
        .bind(b.created_at)
        .execute(&self.pool)
        .await?;
        Ok(b)
    }

    async fn get_blob(&self, id: &str) -> Result<Option<Blob>> {
        let row = sqlx::query(&format!("SELECT {META} FROM blobs WHERE id = $1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(blob).transpose()
    }

    async fn blob_data(&self, id: &str) -> Result<Option<Vec<u8>>> {
        Ok(sqlx::query_scalar("SELECT data FROM blobs WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?)
    }

    async fn list_blobs(&self, run_id: Option<&str>, limit: u32) -> Result<Vec<Blob>> {
        let rows = match run_id {
            Some(r) => {
                sqlx::query(&format!(
                    "SELECT {META} FROM blobs WHERE run_id = $1 ORDER BY created_at DESC, id DESC LIMIT $2"
                ))
                .bind(r)
                .bind(limit as i64)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(&format!(
                    "SELECT {META} FROM blobs ORDER BY created_at DESC, id DESC LIMIT $1"
                ))
                .bind(limit as i64)
                .fetch_all(&self.pool)
                .await?
            }
        };
        rows.iter().map(blob).collect()
    }

    async fn delete_blob(&self, id: &str) -> Result<bool> {
        let r = sqlx::query("DELETE FROM blobs WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }
}
