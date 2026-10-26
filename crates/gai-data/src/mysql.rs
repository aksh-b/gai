//! MySQL (and MariaDB): queries in read-only transactions with an
//! execution time limit; rows come back as JSON arrays built by MySQL.

use std::str::FromStr;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use sqlx::mysql::{MySqlConnectOptions, MySqlPool, MySqlPoolOptions};
use sqlx::pool::PoolConnection;
use sqlx::{Column as _, Executor, MySql, Row, TypeInfo};

use crate::{
    Column, ColumnInfo, Database, QueryResult, Result, Schema, Table, invalid, rows_from_json,
    unique_columns,
};

pub(crate) struct My {
    pool: MySqlPool,
    timeout: u64,
}

impl My {
    pub async fn connect(url: &str, password: Option<String>, timeout: u64) -> Result<Self> {
        let mut opts =
            MySqlConnectOptions::from_str(url).map_err(|e| invalid(format!("url: {e}")))?;
        if let Some(p) = password {
            opts = opts.password(&p);
        }
        let pool = MySqlPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(15))
            .connect_with(opts)
            .await?;
        Ok(Self { pool, timeout })
    }

    /// A connection inside a read-only transaction.
    async fn reading(&self) -> Result<PoolConnection<MySql>> {
        let mut c = self.pool.acquire().await?;
        c.execute(format!("SET SESSION max_execution_time = {}", self.timeout * 1000).as_str())
            .await?;
        c.execute("START TRANSACTION READ ONLY").await?;
        Ok(c)
    }
}

#[async_trait]
impl Database for My {
    async fn schema(&self) -> Result<Schema> {
        let mut c = self.reading().await?;
        let rows = sqlx::query(
            "SELECT CAST(c.TABLE_NAME AS CHAR) AS table_name, CAST(c.COLUMN_NAME AS CHAR) AS column_name,
                    CAST(c.COLUMN_TYPE AS CHAR) AS data_type, c.IS_NULLABLE = 'YES' AS nullable,
                    CAST(t.TABLE_TYPE AS CHAR) AS table_type, t.TABLE_ROWS AS estimate
             FROM information_schema.COLUMNS c
             JOIN information_schema.TABLES t ON t.TABLE_SCHEMA = c.TABLE_SCHEMA AND t.TABLE_NAME = c.TABLE_NAME
             WHERE c.TABLE_SCHEMA = DATABASE()
             ORDER BY c.TABLE_NAME, c.ORDINAL_POSITION
             LIMIT 20000",
        )
        .fetch_all(&mut *c)
        .await?;
        c.execute("ROLLBACK").await?;
        let mut tables: Vec<Table> = Vec::new();
        for r in rows {
            let name: String = r.try_get("table_name")?;
            if tables.last().is_none_or(|t| t.name != name) {
                let kind: String = r.try_get("table_type")?;
                tables.push(Table {
                    schema: None,
                    name,
                    kind: if kind.contains("VIEW") {
                        "view".into()
                    } else {
                        "table".into()
                    },
                    columns: Vec::new(),
                    rows: r.try_get::<Option<u64>, _>("estimate").ok().flatten(),
                });
            }
            let data_type = match r.try_get::<String, _>("data_type") {
                Ok(t) => t,
                Err(_) => {
                    String::from_utf8_lossy(&r.try_get::<Vec<u8>, _>("data_type")?).into_owned()
                }
            };
            tables
                .last_mut()
                .expect("a table")
                .columns
                .push(ColumnInfo {
                    name: r.try_get("column_name")?,
                    data_type,
                    nullable: r.try_get::<i64, _>("nullable")? != 0,
                });
        }
        Ok(Schema { tables })
    }

    async fn query(&self, sql: &str, max_rows: usize) -> Result<QueryResult> {
        let mut c = self.reading().await?;
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
            "SELECT CAST(JSON_ARRAY({}) AS CHAR) AS `row` FROM ({sql}) AS q LIMIT {}",
            columns
                .iter()
                .map(|col| format!("q.{}", crate::sql::quote("mysql", &col.name)))
                .collect::<Vec<_>>()
                .join(", "),
            max_rows + 1
        );
        let texts: Vec<Value> = sqlx::query(&wrapped)
            .fetch_all(&mut *c)
            .await?
            .iter()
            .map(|r| r.try_get::<String, _>("row").map(Value::String))
            .collect::<std::result::Result<_, _>>()?;
        c.execute("ROLLBACK").await?;
        let (rows, truncated) = rows_from_json(texts, max_rows);
        Ok(QueryResult {
            columns,
            rows,
            truncated,
            elapsed_ms: 0,
        })
    }

    async fn explain(&self, sql: &str) -> Result<String> {
        let mut c = self.reading().await?;
        let rows = sqlx::query(&format!("EXPLAIN FORMAT=TREE {sql}"))
            .fetch_all(&mut *c)
            .await;
        let rows = match rows {
            Ok(r) => r,
            // Older servers and MariaDB: the table form.
            Err(_) => {
                sqlx::query(&format!("EXPLAIN {sql}"))
                    .fetch_all(&mut *c)
                    .await?
            }
        };
        c.execute("ROLLBACK").await?;
        Ok(rows
            .iter()
            .map(|r| {
                (0..r.len())
                    .map(|i| r.try_get::<String, _>(i).unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    async fn execute(&self, sql: &str) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        let n = sqlx::query(sql).execute(&mut *tx).await?.rows_affected();
        tx.commit().await?;
        Ok(n)
    }
}
