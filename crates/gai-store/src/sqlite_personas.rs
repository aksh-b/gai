//! SQLite implementation of persona storage.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::Value;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use super::{SqliteStore, json, now, parse};
use crate::*;

fn version(r: &SqliteRow) -> Result<PersonaVersion> {
    Ok(PersonaVersion {
        name: r.try_get("name")?,
        version: r.try_get::<i64, _>("version")? as u32,
        spec: parse(&r.try_get::<String, _>("spec")?)?,
        note: r.try_get("note")?,
        created_at: r.try_get("created_at")?,
    })
}

#[async_trait]
impl PersonaStore for SqliteStore {
    async fn put_persona(&self, name: &str, spec: &Value, note: Option<&str>) -> Result<u32> {
        let mut tx = self.write_tx().await?;
        let latest: Option<(i64, String)> = sqlx::query_as(
            "SELECT v.version, v.spec FROM personas w JOIN persona_versions v ON v.name = w.name AND v.version = w.latest_version WHERE w.name = ?",
        )
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
            "INSERT INTO personas (name, latest_version, display_name, description, avatar, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(name) DO UPDATE SET latest_version = excluded.latest_version,
                 display_name = excluded.display_name, description = excluded.description,
                 avatar = excluded.avatar, updated_at = excluded.updated_at",
        )
        .bind(name)
        .bind(v)
        .bind(spec["display_name"].as_str())
        .bind(spec["description"].as_str())
        .bind(spec["avatar"].as_str())
        .bind(ts)
        .bind(ts)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO persona_versions (name, version, spec, note, created_at) VALUES (?, ?, ?, ?, ?)",
        )
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

    async fn get_persona(&self, name: &str, v: Option<u32>) -> Result<Option<PersonaVersion>> {
        let row = match v {
            Some(v) => {
                sqlx::query("SELECT * FROM persona_versions WHERE name = ? AND version = ?")
                    .bind(name)
                    .bind(v as i64)
                    .fetch_optional(&self.pool)
                    .await?
            }
            None => {
                sqlx::query(
                    "SELECT v.* FROM personas w JOIN persona_versions v ON v.name = w.name AND v.version = w.latest_version WHERE w.name = ?",
                )
                .bind(name)
                .fetch_optional(&self.pool)
                .await?
            }
        };
        row.as_ref().map(version).transpose()
    }

    async fn list_personas(&self) -> Result<Vec<PersonaSummary>> {
        let rows = sqlx::query("SELECT * FROM personas ORDER BY name")
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|r| {
                Ok(PersonaSummary {
                    name: r.try_get("name")?,
                    latest_version: r.try_get::<i64, _>("latest_version")? as u32,
                    display_name: r.try_get("display_name")?,
                    description: r.try_get("description")?,
                    avatar: r.try_get("avatar")?,
                    labels: parse(&r.try_get::<String, _>("labels")?)?,
                    created_at: r.try_get("created_at")?,
                    updated_at: r.try_get("updated_at")?,
                })
            })
            .collect()
    }

    async fn list_persona_versions(&self, name: &str) -> Result<Vec<PersonaVersion>> {
        let rows =
            sqlx::query("SELECT * FROM persona_versions WHERE name = ? ORDER BY version DESC")
                .bind(name)
                .fetch_all(&self.pool)
                .await?;
        rows.iter().map(version).collect()
    }

    async fn set_persona_label(&self, name: &str, label: &str, version: Option<u32>) -> Result<()> {
        let mut tx = self.write_tx().await?;
        let labels: Option<String> =
            sqlx::query_scalar("SELECT labels FROM personas WHERE name = ?")
                .bind(name)
                .fetch_optional(&mut *tx)
                .await?;
        let mut labels: BTreeMap<String, u32> =
            parse(&labels.ok_or_else(|| StoreError::NotFound(format!("persona {name}")))?)?;
        match version {
            Some(v) => {
                let exists: Option<i64> = sqlx::query_scalar(
                    "SELECT 1 FROM persona_versions WHERE name = ? AND version = ?",
                )
                .bind(name)
                .bind(v as i64)
                .fetch_optional(&mut *tx)
                .await?;
                if exists.is_none() {
                    return Err(StoreError::NotFound(format!("persona {name} version {v}")));
                }
                labels.insert(label.to_owned(), v);
            }
            None => {
                labels.remove(label);
            }
        }
        sqlx::query("UPDATE personas SET labels = ?, updated_at = ? WHERE name = ?")
            .bind(json(&labels))
            .bind(now())
            .bind(name)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn delete_persona(&self, name: &str) -> Result<bool> {
        let r = sqlx::query("DELETE FROM personas WHERE name = ?")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }
}
