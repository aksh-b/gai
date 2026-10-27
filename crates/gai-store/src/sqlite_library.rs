//! SQLite implementation of the library (versioned items, ratings) and jobs.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::Value;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use super::{SqliteStore, json, now, parse};
use crate::*;

fn version(r: &SqliteRow) -> Result<LibraryVersion> {
    Ok(LibraryVersion {
        kind: r.try_get("kind")?,
        name: r.try_get("name")?,
        version: r.try_get::<i64, _>("version")? as u32,
        spec: parse(&r.try_get::<String, _>("spec")?)?,
        note: r.try_get("note")?,
        created_at: r.try_get("created_at")?,
    })
}

fn item(r: &SqliteRow) -> Result<LibraryItem> {
    Ok(LibraryItem {
        kind: r.try_get("kind")?,
        name: r.try_get("name")?,
        latest_version: r.try_get::<i64, _>("latest_version")? as u32,
        title: r.try_get("title")?,
        description: r.try_get("description")?,
        labels: parse(&r.try_get::<String, _>("labels")?)?,
        rating: r.try_get("rating")?,
        ratings: r.try_get::<i64, _>("ratings")? as u32,
        created_at: r.try_get("created_at")?,
        updated_at: r.try_get("updated_at")?,
    })
}

fn job(r: &SqliteRow) -> Result<Job> {
    Ok(Job {
        id: r.try_get("id")?,
        kind: r.try_get("kind")?,
        status: r.try_get("status")?,
        input: parse(&r.try_get::<String, _>("input")?)?,
        result: r
            .try_get::<Option<String>, _>("result")?
            .map(|s| parse(&s))
            .transpose()?,
        error: r.try_get("error")?,
        created_at: r.try_get("created_at")?,
        updated_at: r.try_get("updated_at")?,
    })
}

/// Items with their mean rating and number of ratings.
const ITEMS: &str = "SELECT i.*, r.rating, COALESCE(r.ratings, 0) AS ratings FROM library_items i
     LEFT JOIN (SELECT kind, name, CAST(AVG(stars) AS DOUBLE PRECISION) AS rating, COUNT(*) AS ratings
                FROM library_ratings GROUP BY kind, name) r
       ON r.kind = i.kind AND r.name = i.name";

#[async_trait]
impl LibraryStore for SqliteStore {
    async fn put_item(
        &self,
        kind: &str,
        name: &str,
        spec: &Value,
        note: Option<&str>,
    ) -> Result<u32> {
        let mut tx = self.write_tx().await?;
        let latest: Option<(i64, String)> = sqlx::query_as(
            "SELECT v.version, v.spec FROM library_items i JOIN library_versions v
               ON v.kind = i.kind AND v.name = i.name AND v.version = i.latest_version
             WHERE i.kind = ? AND i.name = ?",
        )
        .bind(kind)
        .bind(name)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((v, s)) = &latest
            && parse::<Value>(s)? == *spec
        {
            return Ok(*v as u32);
        }
        let v = latest.map(|(v, _)| v + 1).unwrap_or(1);
        let ts = now();
        sqlx::query(
            "INSERT INTO library_items (kind, name, latest_version, title, description, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (kind, name) DO UPDATE SET latest_version = excluded.latest_version,
                 title = excluded.title, description = excluded.description,
                 updated_at = excluded.updated_at",
        )
        .bind(kind)
        .bind(name)
        .bind(v)
        .bind(spec["title"].as_str())
        .bind(spec["description"].as_str())
        .bind(ts)
        .bind(ts)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO library_versions (kind, name, version, spec, note, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(kind)
        .bind(name)
        .bind(v)
        .bind(json(spec))
        .bind(note)
        .bind(ts)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(v as u32)
    }

    async fn get_item(
        &self,
        kind: &str,
        name: &str,
        v: Option<u32>,
    ) -> Result<Option<LibraryVersion>> {
        let row =
            match v {
                Some(v) => sqlx::query(
                    "SELECT * FROM library_versions WHERE kind = ? AND name = ? AND version = ?",
                )
                .bind(kind)
                .bind(name)
                .bind(v as i64)
                .fetch_optional(&self.pool)
                .await?,
                None => {
                    sqlx::query(
                        "SELECT v.* FROM library_items i JOIN library_versions v
                       ON v.kind = i.kind AND v.name = i.name AND v.version = i.latest_version
                     WHERE i.kind = ? AND i.name = ?",
                    )
                    .bind(kind)
                    .bind(name)
                    .fetch_optional(&self.pool)
                    .await?
                }
            };
        row.as_ref().map(version).transpose()
    }

    async fn item(&self, kind: &str, name: &str) -> Result<Option<LibraryItem>> {
        let row = sqlx::query(&format!("{ITEMS} WHERE i.kind = ? AND i.name = ?"))
            .bind(kind)
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(item).transpose()
    }

    async fn list_items(&self, kind: &str) -> Result<Vec<LibraryItem>> {
        let rows = sqlx::query(&format!("{ITEMS} WHERE i.kind = ? ORDER BY i.name"))
            .bind(kind)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(item).collect()
    }

    async fn list_item_versions(&self, kind: &str, name: &str) -> Result<Vec<LibraryVersion>> {
        let rows = sqlx::query(
            "SELECT * FROM library_versions WHERE kind = ? AND name = ? ORDER BY version DESC",
        )
        .bind(kind)
        .bind(name)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(version).collect()
    }

    async fn set_item_label(
        &self,
        kind: &str,
        name: &str,
        label: &str,
        version: Option<u32>,
    ) -> Result<()> {
        let mut tx = self.write_tx().await?;
        let labels: Option<String> =
            sqlx::query_scalar("SELECT labels FROM library_items WHERE kind = ? AND name = ?")
                .bind(kind)
                .bind(name)
                .fetch_optional(&mut *tx)
                .await?;
        let mut labels: BTreeMap<String, u32> =
            parse(&labels.ok_or_else(|| StoreError::NotFound(format!("{kind} {name}")))?)?;
        match version {
            Some(v) => {
                let exists: Option<i64> = sqlx::query_scalar(
                    "SELECT version FROM library_versions WHERE kind = ? AND name = ? AND version = ?",
                )
                .bind(kind)
                .bind(name)
                .bind(v as i64)
                .fetch_optional(&mut *tx)
                .await?;
                if exists.is_none() {
                    return Err(StoreError::NotFound(format!("{kind} {name} version {v}")));
                }
                labels.insert(label.to_owned(), v);
            }
            None => {
                labels.remove(label);
            }
        }
        sqlx::query(
            "UPDATE library_items SET labels = ?, updated_at = ? WHERE kind = ? AND name = ?",
        )
        .bind(json(&labels))
        .bind(now())
        .bind(kind)
        .bind(name)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn delete_item(&self, kind: &str, name: &str) -> Result<bool> {
        let r = sqlx::query("DELETE FROM library_items WHERE kind = ? AND name = ?")
            .bind(kind)
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    async fn rate_item(
        &self,
        kind: &str,
        name: &str,
        rater: &str,
        stars: u8,
        comment: Option<&str>,
    ) -> Result<()> {
        if !(1..=5).contains(&stars) {
            return Err(StoreError::Conflict("stars go from 1 to 5".into()));
        }
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT latest_version FROM library_items WHERE kind = ? AND name = ?",
        )
        .bind(kind)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        if found.is_none() {
            return Err(StoreError::NotFound(format!("{kind} {name}")));
        }
        sqlx::query(
            "INSERT INTO library_ratings (kind, name, rater, stars, comment, created_at) VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT (kind, name, rater) DO UPDATE SET stars = excluded.stars,
                 comment = excluded.comment, created_at = excluded.created_at",
        )
        .bind(kind)
        .bind(name)
        .bind(rater)
        .bind(stars as i64)
        .bind(comment)
        .bind(now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_ratings(&self, kind: &str, name: &str) -> Result<Vec<LibraryRating>> {
        let rows = sqlx::query(
            "SELECT * FROM library_ratings WHERE kind = ? AND name = ? ORDER BY created_at DESC, rater",
        )
        .bind(kind)
        .bind(name)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok(LibraryRating {
                    rater: r.try_get("rater")?,
                    stars: r.try_get::<i64, _>("stars")? as u8,
                    comment: r.try_get("comment")?,
                    created_at: r.try_get("created_at")?,
                })
            })
            .collect()
    }

    async fn create_job(&self, kind: &str, input: &Value) -> Result<Job> {
        let ts = now();
        let id = gai_core::new_id("job");
        sqlx::query(
            "INSERT INTO jobs (id, kind, status, input, created_at, updated_at) VALUES (?, ?, 'running', ?, ?, ?)",
        )
        .bind(&id)
        .bind(kind)
        .bind(json(input))
        .bind(ts)
        .bind(ts)
        .execute(&self.pool)
        .await?;
        Ok(Job {
            id,
            kind: kind.to_owned(),
            status: "running".into(),
            input: input.clone(),
            result: None,
            error: None,
            created_at: ts,
            updated_at: ts,
        })
    }

    async fn get_job(&self, id: &str) -> Result<Option<Job>> {
        let row = sqlx::query("SELECT * FROM jobs WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(job).transpose()
    }

    async fn list_jobs(&self, kind: &str, limit: u32) -> Result<Vec<Job>> {
        let rows = sqlx::query(
            "SELECT * FROM jobs WHERE kind = ? ORDER BY created_at DESC, id DESC LIMIT ?",
        )
        .bind(kind)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(job).collect()
    }

    async fn update_job(
        &self,
        id: &str,
        status: &str,
        result: Option<&Value>,
        error: Option<&str>,
    ) -> Result<()> {
        let r = sqlx::query(
            "UPDATE jobs SET status = ?, result = ?, error = ?, updated_at = ? WHERE id = ?",
        )
        .bind(status)
        .bind(result.map(json))
        .bind(error)
        .bind(now())
        .bind(id)
        .execute(&self.pool)
        .await?;
        if r.rows_affected() == 0 {
            return Err(StoreError::NotFound(format!("job {id}")));
        }
        Ok(())
    }
}
