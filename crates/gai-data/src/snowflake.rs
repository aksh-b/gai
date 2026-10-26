//! Snowflake over its SQL API (`/api/v2/statements`), signed in with a
//! programmatic access token or an OAuth token. Snowflake has no read-only
//! transactions, so queries are checked to be one `SELECT` before they are
//! sent; give the connection's role read access only, too.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::{Column, ColumnInfo, DataError, Database, QueryResult, Result, Schema, Table};

pub(crate) struct Sf {
    http: reqwest::Client,
    token: String,
    token_type: String,
    base: String,
    database: Option<String>,
    schema: Option<String>,
    warehouse: Option<String>,
    role: Option<String>,
    timeout: u64,
}

impl Sf {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        http: reqwest::Client,
        token: String,
        token_type: String,
        base: String,
        database: Option<String>,
        schema: Option<String>,
        warehouse: Option<String>,
        role: Option<String>,
        timeout: u64,
    ) -> Self {
        Self {
            http,
            token,
            token_type,
            base: base.trim_end_matches('/').to_owned(),
            database,
            schema,
            warehouse,
            role,
            timeout,
        }
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.token)
            .header("X-Snowflake-Authorization-Token-Type", &self.token_type)
            .header("Accept", "application/json")
    }

    async fn run(&self, sql: &str) -> Result<Value> {
        let mut body = json!({
            "statement": sql,
            "timeout": self.timeout,
            "parameters": {"MULTI_STATEMENT_COUNT": "1"},
        });
        for (k, v) in [
            ("database", &self.database),
            ("schema", &self.schema),
            ("warehouse", &self.warehouse),
            ("role", &self.role),
        ] {
            if let Some(v) = v {
                body[k] = json!(v);
            }
        }
        let r = self
            .req(reqwest::Method::POST, "/api/v2/statements?nullable=true")
            .json(&body)
            .send()
            .await
            .map_err(|e| DataError::Database(format!("Snowflake: {e}")))?;
        let mut status = r.status();
        let mut v: Value = r.json().await.unwrap_or(Value::Null);
        let deadline = Instant::now() + Duration::from_secs(self.timeout + 5);
        // 202: still running; ask for it until it's done.
        while status.as_u16() == 202 {
            if Instant::now() > deadline {
                return Err(DataError::Database(format!(
                    "the query took over {} seconds",
                    self.timeout
                )));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            let handle = v["statementHandle"].as_str().unwrap_or_default().to_owned();
            let r = self
                .req(
                    reqwest::Method::GET,
                    &format!("/api/v2/statements/{handle}"),
                )
                .send()
                .await
                .map_err(|e| DataError::Database(format!("Snowflake: {e}")))?;
            status = r.status();
            v = r.json().await.unwrap_or(Value::Null);
        }
        if !status.is_success() {
            return Err(DataError::Database(format!(
                "Snowflake: {}",
                v["message"].as_str().unwrap_or(status.as_str())
            )));
        }
        Ok(v)
    }
}

/// A Snowflake cell (the API sends text), typed by its column.
fn cell(v: &Value, col: &Value) -> Value {
    let Some(s) = v.as_str() else {
        return v.clone();
    };
    let ty = col["type"].as_str().unwrap_or_default().to_lowercase();
    match ty.as_str() {
        "fixed" if col["scale"].as_i64().unwrap_or(0) == 0 => s
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or_else(|_| json!(s)),
        "fixed" | "real" => s
            .parse::<f64>()
            .map(Value::from)
            .unwrap_or_else(|_| json!(s)),
        "boolean" => json!(s.eq_ignore_ascii_case("true")),
        // Days since 1970.
        "date" => s
            .parse::<i64>()
            .ok()
            .and_then(|d| {
                chrono::NaiveDate::from_ymd_opt(1970, 1, 1)?
                    .checked_add_signed(chrono::Duration::days(d))
            })
            .map(|d| json!(d.to_string()))
            .unwrap_or_else(|| json!(s)),
        // Seconds since 1970 (with a fraction; TIMESTAMP_TZ adds an offset).
        "timestamp_ntz" | "timestamp_ltz" | "timestamp_tz" => s
            .split_whitespace()
            .next()
            .and_then(|x| x.parse::<f64>().ok())
            .and_then(|secs| {
                chrono::DateTime::from_timestamp(secs.floor() as i64, (secs.fract() * 1e9) as u32)
            })
            .map(|t| json!(t.naive_utc().to_string()))
            .unwrap_or_else(|| json!(s)),
        _ => json!(s),
    }
}

#[async_trait]
impl Database for Sf {
    async fn schema(&self) -> Result<Schema> {
        let v = self
            .run(
                "SELECT c.table_schema, c.table_name, c.column_name, c.data_type, c.is_nullable, t.table_type, t.row_count
                 FROM information_schema.columns c
                 JOIN information_schema.tables t ON t.table_schema = c.table_schema AND t.table_name = c.table_name
                 WHERE c.table_schema <> 'INFORMATION_SCHEMA'
                 ORDER BY c.table_schema, c.table_name, c.ordinal_position
                 LIMIT 20000",
            )
            .await?;
        let mut tables: Vec<Table> = Vec::new();
        for row in v["data"].as_array().into_iter().flatten() {
            let f = |i: usize| row[i].as_str().unwrap_or_default().to_owned();
            let (schema, name) = (f(0), f(1));
            if tables
                .last()
                .is_none_or(|t| t.name != name || t.schema.as_deref() != Some(schema.as_str()))
            {
                tables.push(Table {
                    schema: Some(schema),
                    name,
                    kind: if f(5).contains("VIEW") {
                        "view".into()
                    } else {
                        "table".into()
                    },
                    columns: Vec::new(),
                    rows: f(6).parse().ok(),
                });
            }
            tables
                .last_mut()
                .expect("a table")
                .columns
                .push(ColumnInfo {
                    name: f(2),
                    data_type: f(3),
                    nullable: f(4) == "YES",
                });
        }
        Ok(Schema { tables })
    }

    async fn query(&self, sql: &str, max_rows: usize) -> Result<QueryResult> {
        let v = self.run(sql).await?;
        let meta = &v["resultSetMetaData"];
        let cols = meta["rowType"].as_array().cloned().unwrap_or_default();
        let columns = cols
            .iter()
            .map(|c| Column {
                name: c["name"].as_str().unwrap_or_default().to_owned(),
                data_type: c["type"].as_str().unwrap_or_default().to_lowercase(),
            })
            .collect();
        let data = v["data"].as_array().cloned().unwrap_or_default();
        let total = meta["numRows"].as_u64().unwrap_or(data.len() as u64);
        let rows = data
            .iter()
            .take(max_rows)
            .map(|r| {
                cols.iter()
                    .enumerate()
                    .map(|(i, c)| cell(&r[i], c))
                    .collect()
            })
            .collect();
        Ok(QueryResult {
            columns,
            rows,
            truncated: total > max_rows as u64,
            elapsed_ms: 0,
        })
    }

    async fn explain(&self, sql: &str) -> Result<String> {
        let v = self.run(&format!("EXPLAIN USING TEXT {sql}")).await?;
        Ok(v["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| r[0].as_str())
            .collect::<Vec<_>>()
            .join("\n"))
    }

    async fn execute(&self, sql: &str) -> Result<u64> {
        let v = self.run(sql).await?;
        Ok(v["stats"]["numRowsInserted"].as_u64().unwrap_or(0)
            + v["stats"]["numRowsUpdated"].as_u64().unwrap_or(0)
            + v["stats"]["numRowsDeleted"].as_u64().unwrap_or(0))
    }
}
