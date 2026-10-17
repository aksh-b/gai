//! SQLite implementation of blob storage: bytes live in the database next
//! to their metadata, so a backup of the database is a backup of the media.

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use super::{SqliteStore, json, now, parse};
use crate::*;

/// Every column but the bytes.
const META: &str = "id, media_type, size, sha256, name, run_id, source, metadata, created_at";

fn blob(r: &SqliteRow) -> Result<Blob> {
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
impl BlobStore for SqliteStore {
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
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        let row = sqlx::query(&format!("SELECT {META} FROM blobs WHERE id = ?"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(blob).transpose()
    }

    async fn blob_data(&self, id: &str) -> Result<Option<Vec<u8>>> {
        Ok(sqlx::query_scalar("SELECT data FROM blobs WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?)
    }

    async fn list_blobs(&self, run_id: Option<&str>, limit: u32) -> Result<Vec<Blob>> {
        let rows = match run_id {
            Some(r) => {
                sqlx::query(&format!(
                    "SELECT {META} FROM blobs WHERE run_id = ? ORDER BY created_at DESC, id DESC LIMIT ?"
                ))
                .bind(r)
                .bind(limit as i64)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query(&format!(
                    "SELECT {META} FROM blobs ORDER BY created_at DESC, id DESC LIMIT ?"
                ))
                .bind(limit as i64)
                .fetch_all(&self.pool)
                .await?
            }
        };
        rows.iter().map(blob).collect()
    }

    async fn delete_blob(&self, id: &str) -> Result<bool> {
        let r = sqlx::query("DELETE FROM blobs WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn blobs_round_trip() {
        let s = SqliteStore::temporary().await.unwrap();
        let b = s
            .put_blob(NewBlob {
                media_type: "audio/wav".into(),
                name: Some("hi.wav".into()),
                run_id: Some("run_1".into()),
                source: Some("speak".into()),
                data: b"RIFF....".to_vec(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(b.size, 8);
        assert_eq!(b.sha256.len(), 64);
        assert_eq!(
            s.get_blob(&b.id).await.unwrap().unwrap().name.as_deref(),
            Some("hi.wav")
        );
        assert_eq!(s.blob_data(&b.id).await.unwrap().unwrap(), b"RIFF....");
        s.put_blob(NewBlob {
            media_type: "image/png".into(),
            data: vec![1],
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(s.list_blobs(None, 10).await.unwrap().len(), 2);
        assert_eq!(s.list_blobs(Some("run_1"), 10).await.unwrap().len(), 1);
        assert!(s.delete_blob(&b.id).await.unwrap());
        assert!(s.blob_data(&b.id).await.unwrap().is_none());
    }
}
