//! Postgres: queries in read-only transactions with a statement timeout;
//! rows come back as JSON built by Postgres itself, so every type converts.

use std::str::FromStr;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use sqlx::{Column as _, Executor, Row, TypeInfo};

use crate::{
    Column, ColumnInfo, Database, QueryResult, Result, Schema, Table, invalid, rows_from_json,
    unique_columns,
};

pub(crate) struct Pg {
    pool: PgPool,
    timeout: u64,
}

impl Pg {
    pub async fn connect(url: &str, password: Option<String>, timeout: u64) -> Result<Self> {
        let mut opts = PgConnectOptions::from_str(url).map_err(|e| invalid(format!("url: {e}")))?;
        if let Some(p) = password {
            opts = opts.password(&p);
        }
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(15))
            .connect_with(opts)
            .await?;
        Ok(Self { pool, timeout })
    }

    async fn begin(&self, read_only: bool) -> Result<sqlx::Transaction<'static, sqlx::Postgres>> {
        let mut tx = self.pool.begin().await?;
        if read_only {
            tx.execute("SET TRANSACTION READ ONLY").await?;
        }
        tx.execute(format!("SET LOCAL statement_timeout = {}", self.timeout * 1000).as_str())
            .await?;
        Ok(tx)
    }
}

#[async_trait]
impl Database for Pg {
    async fn schema(&self) -> Result<Schema> {
        let mut tx = self.begin(true).await?;
        let rows = sqlx::query(
            "SELECT c.table_schema, c.table_name, c.column_name, c.data_type, c.is_nullable = 'YES' AS nullable,
                    t.table_type, GREATEST(cl.reltuples, 0)::BIGINT AS estimate
             FROM information_schema.columns c
             JOIN information_schema.tables t ON t.table_schema = c.table_schema AND t.table_name = c.table_name
             LEFT JOIN pg_namespace n ON n.nspname = c.table_schema
             LEFT JOIN pg_class cl ON cl.relnamespace = n.oid AND cl.relname = c.table_name
             WHERE c.table_schema NOT IN ('pg_catalog', 'information_schema') AND c.table_schema NOT LIKE 'pg_toast%'
             ORDER BY c.table_schema, c.table_name, c.ordinal_position
             LIMIT 20000",
        )
        .fetch_all(&mut *tx)
        .await?;
        tx.rollback().await?;
        let mut tables: Vec<Table> = Vec::new();
        for r in rows {
            let (schema, name): (String, String) =
                (r.try_get("table_schema")?, r.try_get("table_name")?);
            if tables
                .last()
                .is_none_or(|t| t.name != name || t.schema.as_deref() != Some(schema.as_str()))
            {
                let kind: String = r.try_get("table_type")?;
                tables.push(Table {
                    schema: Some(schema),
                    name,
                    kind: if kind == "VIEW" {
                        "view".into()
                    } else {
                        "table".into()
                    },
                    columns: Vec::new(),
                    rows: r
                        .try_get::<Option<i64>, _>("estimate")?
                        .map(|n| n as u64)
                        .filter(|_| kind != "VIEW"),
                });
            }
            tables
                .last_mut()
                .expect("a table")
                .columns
                .push(ColumnInfo {
                    name: r.try_get("column_name")?,
                    data_type: r.try_get("data_type")?,
                    nullable: r.try_get("nullable")?,
                });
        }
        Ok(Schema { tables })
    }

    async fn query(&self, sql: &str, max_rows: usize) -> Result<QueryResult> {
        // Queries always run read-only; changes go through `execute`.
        let mut tx = self.begin(true).await?;
        let described = (&mut *tx).describe(sql).await?;
        let columns: Vec<Column> = described
            .columns()
            .iter()
            .map(|c| Column {
                name: c.name().to_owned(),
                data_type: c.type_info().name().to_lowercase(),
            })
            .collect();
        unique_columns(&columns)?;
        // Postgres turns each row into JSON, whatever its types.
        let wrapped = format!(
            "SELECT json_build_array({})::text AS row FROM ({sql}) AS q LIMIT {}",
            if columns.is_empty() {
                String::new()
            } else {
                columns
                    .iter()
                    .map(|c| format!("q.{}", crate::sql::quote("postgres", &c.name)))
                    .collect::<Vec<_>>()
                    .join(", ")
            },
            max_rows + 1
        );
        let texts: Vec<Value> = sqlx::query(&wrapped)
            .fetch_all(&mut *tx)
            .await?
            .iter()
            .map(|r| r.try_get::<String, _>("row").map(Value::String))
            .collect::<std::result::Result<_, _>>()?;
        tx.rollback().await?;
        let (rows, truncated) = rows_from_json(texts, max_rows);
        Ok(QueryResult {
            columns,
            rows,
            truncated,
            elapsed_ms: 0,
        })
    }

    async fn explain(&self, sql: &str) -> Result<String> {
        let mut tx = self.begin(true).await?;
        let rows = sqlx::query(&format!("EXPLAIN {sql}"))
            .fetch_all(&mut *tx)
            .await?;
        tx.rollback().await?;
        Ok(rows
            .iter()
            .filter_map(|r| r.try_get::<String, _>(0).ok())
            .collect::<Vec<_>>()
            .join("\n"))
    }

    async fn execute(&self, sql: &str) -> Result<u64> {
        let mut tx = self.begin(false).await?;
        let n = sqlx::query(sql).execute(&mut *tx).await?.rows_affected();
        tx.commit().await?;
        Ok(n)
    }
}
