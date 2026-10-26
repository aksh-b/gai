//! SQLite files: opened read-only (and `query_only`) unless the connection
//! allows writes; rows come back as JSON arrays built by SQLite.

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::{Column as _, Executor, Row, TypeInfo};

use crate::{
    Column, ColumnInfo, DataError, Database, QueryResult, Result, Schema, Table, rows_from_json,
    unique_columns,
};

pub(crate) struct Lite {
    pool: SqlitePool,
    timeout: u64,
}

impl Lite {
    pub async fn open(path: &Path, read_only: bool, timeout: u64) -> Result<Self> {
        if !path.is_file() {
            return Err(DataError::NotFound(format!(
                "no database file at {}",
                path.display()
            )));
        }
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .read_only(read_only)
            .create_if_missing(false)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await?;
        Ok(Self { pool, timeout })
    }
}

#[async_trait]
impl Database for Lite {
    async fn schema(&self) -> Result<Schema> {
        let rows = sqlx::query(
            "SELECT m.name AS table_name, m.type AS table_type, p.name AS column_name, p.type AS data_type, p.\"notnull\" AS not_null
             FROM sqlite_master m JOIN pragma_table_info(m.name) p
             WHERE m.type IN ('table', 'view') AND m.name NOT LIKE 'sqlite_%'
             ORDER BY m.name, p.cid",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut tables: Vec<Table> = Vec::new();
        for r in rows {
            let name: String = r.try_get("table_name")?;
            if tables.last().is_none_or(|t| t.name != name) {
                let kind: String = r.try_get("table_type")?;
                tables.push(Table {
                    schema: None,
                    name,
                    kind,
                    columns: Vec::new(),
                    rows: None,
                });
            }
            tables
                .last_mut()
                .expect("a table")
                .columns
                .push(ColumnInfo {
                    name: r.try_get("column_name")?,
                    data_type: r.try_get("data_type")?,
                    nullable: r.try_get::<i64, _>("not_null")? == 0,
                });
        }
        // Small databases: exact counts are cheap.
        if tables.len() <= 50 {
            for t in tables.iter_mut().filter(|t| t.kind == "table") {
                let n: i64 = sqlx::query_scalar(&format!(
                    "SELECT COUNT(*) FROM {}",
                    crate::sql::quote("sqlite", &t.name)
                ))
                .fetch_one(&self.pool)
                .await?;
                t.rows = Some(n as u64);
            }
        }
        Ok(Schema { tables })
    }

    async fn query(&self, sql: &str, max_rows: usize) -> Result<QueryResult> {
        let mut c = self.pool.acquire().await?;
        // Even a writable file is only read here.
        c.execute("PRAGMA query_only = ON").await?;
        let result = async {
            let described = (&mut *c).describe(sql).await?;
            let columns: Vec<Column> = described
                .columns()
                .iter()
                .map(|col| Column {
                    name: col.name().to_owned(),
                    data_type: col.type_info().name().to_lowercase(),
                })
                .collect();
            unique_columns(&columns)?;
            let wrapped = format!(
                "SELECT json_array({}) AS row FROM ({sql}) AS q LIMIT {}",
                columns
                    .iter()
                    .map(|col| format!("q.{}", crate::sql::quote("sqlite", &col.name)))
                    .collect::<Vec<_>>()
                    .join(", "),
                max_rows + 1
            );
            let texts: Vec<Value> = tokio::time::timeout(
                Duration::from_secs(self.timeout),
                sqlx::query(&wrapped).fetch_all(&mut *c),
            )
            .await
            .map_err(|_| {
                DataError::Database(format!("the query took over {} seconds", self.timeout))
            })??
            .iter()
            .map(|r| r.try_get::<String, _>("row").map(Value::String))
            .collect::<std::result::Result<_, _>>()?;
            let (rows, truncated) = rows_from_json(texts, max_rows);
            Ok(QueryResult {
                columns,
                rows,
                truncated,
                elapsed_ms: 0,
            })
        }
        .await;
        c.execute("PRAGMA query_only = OFF").await?;
        result
    }

    async fn explain(&self, sql: &str) -> Result<String> {
        let rows = sqlx::query(&format!("EXPLAIN QUERY PLAN {sql}"))
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .iter()
            .filter_map(|r| r.try_get::<String, _>("detail").ok())
            .collect::<Vec<_>>()
            .join("\n"))
    }

    async fn execute(&self, sql: &str) -> Result<u64> {
        Ok(sqlx::query(sql).execute(&self.pool).await?.rows_affected())
    }
}
