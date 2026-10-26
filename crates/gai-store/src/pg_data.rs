//! Postgres implementation of data connections.

use async_trait::async_trait;
use serde_json::Value;
use sqlx::Row;
use sqlx::postgres::PgRow;

use super::{PostgresStore, json, now, parse};
use crate::*;

fn connection(r: &PgRow) -> Result<DataConnection> {
    Ok(DataConnection {
        name: r.try_get("name")?,
        kind: r.try_get("kind")?,
        spec: parse(&r.try_get::<String, _>("spec")?)?,
        schema: r
            .try_get::<Option<String>, _>("schema")?
            .map(|s| parse(&s))
            .transpose()?,
        schema_at: r.try_get("schema_at")?,
        created_at: r.try_get("created_at")?,
        updated_at: r.try_get("updated_at")?,
    })
}

#[async_trait]
impl DataStore for PostgresStore {
    async fn put_connection(&self, name: &str, kind: &str, spec: &Value) -> Result<()> {
        let ts = now();
        sqlx::query(
            "INSERT INTO data_connections (name, kind, spec, created_at, updated_at) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (name) DO UPDATE SET kind = excluded.kind, spec = excluded.spec,
                 schema = NULL, schema_at = NULL, updated_at = excluded.updated_at",
        )
        .bind(name)
        .bind(kind)
        .bind(json(spec))
        .bind(ts)
        .bind(ts)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_connection(&self, name: &str) -> Result<Option<DataConnection>> {
        let row = sqlx::query("SELECT * FROM data_connections WHERE name = $1")
            .bind(name)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(connection).transpose()
    }

    async fn list_connections(&self) -> Result<Vec<DataConnection>> {
        let rows = sqlx::query("SELECT * FROM data_connections ORDER BY name")
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(connection).collect()
    }

    async fn delete_connection(&self, name: &str) -> Result<bool> {
        let r = sqlx::query("DELETE FROM data_connections WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected() > 0)
    }

    async fn save_connection_schema(&self, name: &str, schema: &Value) -> Result<()> {
        let r =
            sqlx::query("UPDATE data_connections SET schema = $1, schema_at = $2 WHERE name = $3")
                .bind(json(schema))
                .bind(now())
                .bind(name)
                .execute(&self.pool)
                .await?;
        if r.rows_affected() == 0 {
            return Err(StoreError::NotFound(format!("connection {name}")));
        }
        Ok(())
    }
}
