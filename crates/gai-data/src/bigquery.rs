//! BigQuery over its REST API. Every query is dry-run first: that checks
//! it is a `SELECT` (on read-only connections) and that it scans no more
//! than `max_bytes_billed`. Signing in uses a service account's key (a
//! signed JWT traded for an access token, kept until it expires) or a
//! given access token.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::{Column, ColumnInfo, DataError, Database, QueryResult, Result, Schema, Table, invalid};

pub(crate) enum Auth {
    /// A service account's JSON key.
    ServiceAccount(String),
    Token(String),
}

pub(crate) struct Bq {
    http: reqwest::Client,
    auth: Auth,
    token: Mutex<Option<(String, Instant)>>,
    project: String,
    dataset: Option<String>,
    location: Option<String>,
    max_bytes: u64,
    api: String,
    read_only: bool,
    timeout: u64,
}

#[derive(Deserialize)]
struct Key {
    client_email: String,
    private_key: String,
    #[serde(default)]
    token_uri: Option<String>,
}

impl Bq {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        http: reqwest::Client,
        auth: Auth,
        project: String,
        dataset: Option<String>,
        location: Option<String>,
        max_bytes: u64,
        api_url: Option<String>,
        read_only: bool,
        timeout: u64,
    ) -> Self {
        Self {
            http,
            auth,
            token: Mutex::default(),
            project,
            dataset,
            location,
            max_bytes,
            api: api_url
                .unwrap_or_else(|| "https://bigquery.googleapis.com".into())
                .trim_end_matches('/')
                .to_owned(),
            read_only,
            timeout,
        }
    }

    async fn token(&self) -> Result<String> {
        let key = match &self.auth {
            Auth::Token(t) => return Ok(t.clone()),
            Auth::ServiceAccount(k) => k,
        };
        let mut cached = self.token.lock().await;
        if let Some((t, until)) = cached.as_ref()
            && Instant::now() < *until
        {
            return Ok(t.clone());
        }
        let key: Key = serde_json::from_str(key)
            .map_err(|e| invalid(format!("the service account key: {e}")))?;
        let token_uri = key
            .token_uri
            .unwrap_or_else(|| "https://oauth2.googleapis.com/token".into());
        let now = chrono::Utc::now().timestamp();
        let claims = json!({
            "iss": key.client_email,
            "scope": if self.read_only { "https://www.googleapis.com/auth/bigquery.readonly" } else { "https://www.googleapis.com/auth/bigquery" },
            "aud": token_uri,
            "iat": now,
            "exp": now + 3600,
        });
        let enc = jsonwebtoken::EncodingKey::from_rsa_pem(key.private_key.as_bytes())
            .map_err(|e| invalid(format!("the service account's private key: {e}")))?;
        let jwt = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
            &claims,
            &enc,
        )
        .map_err(|e| invalid(format!("signing in to BigQuery: {e}")))?;
        let r: Value = self
            .http
            .post(&token_uri)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", jwt.as_str()),
            ])
            .send()
            .await
            .map_err(|e| DataError::Database(format!("signing in to BigQuery: {e}")))?
            .json()
            .await
            .map_err(|e| DataError::Database(format!("signing in to BigQuery: {e}")))?;
        let t = r["access_token"]
            .as_str()
            .ok_or_else(|| {
                DataError::Database(format!(
                    "BigQuery refused the sign-in: {}",
                    r["error_description"].as_str().unwrap_or("")
                ))
            })?
            .to_owned();
        let secs = r["expires_in"].as_u64().unwrap_or(3600).saturating_sub(60);
        *cached = Some((t.clone(), Instant::now() + Duration::from_secs(secs)));
        Ok(t)
    }

    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let mut rb = self
            .http
            .request(method, format!("{}{path}", self.api))
            .bearer_auth(self.token().await?);
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        let r = rb
            .send()
            .await
            .map_err(|e| DataError::Database(format!("BigQuery: {e}")))?;
        let status = r.status();
        let v: Value = r.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(DataError::Database(format!(
                "BigQuery: {}",
                v["error"]["message"].as_str().unwrap_or(status.as_str())
            )));
        }
        Ok(v)
    }

    fn body(&self, sql: &str, dry_run: bool, max_rows: usize) -> Value {
        let mut b = json!({
            "query": sql,
            "useLegacySql": false,
            "dryRun": dry_run,
            "maxResults": max_rows + 1,
            "timeoutMs": self.timeout * 1000,
            "maximumBytesBilled": self.max_bytes.to_string(),
        });
        if let Some(d) = &self.dataset {
            b["defaultDataset"] = json!({"projectId": self.project, "datasetId": d});
        }
        if let Some(l) = &self.location {
            b["location"] = json!(l);
        }
        b
    }

    /// The dry run: its statement type and bytes, refusing what isn't allowed.
    async fn check(&self, sql: &str) -> Result<(String, u64)> {
        let v = self
            .call(
                reqwest::Method::POST,
                &format!("/bigquery/v2/projects/{}/queries", self.project),
                Some(self.body(sql, true, 0)),
            )
            .await?;
        let kind = v["statementType"]
            .as_str()
            .or(v["statistics"]["query"]["statementType"].as_str())
            .unwrap_or("SELECT")
            .to_owned();
        let bytes = v["totalBytesProcessed"]
            .as_str()
            .and_then(|b| b.parse().ok())
            .unwrap_or(0);
        if self.read_only && kind != "SELECT" {
            return Err(DataError::Refused(format!(
                "only queries (SELECT) run here, not {kind}"
            )));
        }
        if bytes > self.max_bytes {
            return Err(DataError::Refused(format!(
                "the query would scan {} MB, over this connection's limit of {} MB: filter by date or select fewer columns",
                bytes / 1_000_000,
                self.max_bytes / 1_000_000
            )));
        }
        Ok((kind, bytes))
    }

    async fn run(&self, sql: &str, max_rows: usize) -> Result<Value> {
        let mut v = self
            .call(
                reqwest::Method::POST,
                &format!("/bigquery/v2/projects/{}/queries", self.project),
                Some(self.body(sql, false, max_rows)),
            )
            .await?;
        let deadline = Instant::now() + Duration::from_secs(self.timeout);
        while v["jobComplete"] == false {
            if Instant::now() > deadline {
                return Err(DataError::Database(format!(
                    "the query took over {} seconds",
                    self.timeout
                )));
            }
            let job = v["jobReference"]["jobId"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let mut path = format!(
                "/bigquery/v2/projects/{}/queries/{job}?maxResults={}&timeoutMs=10000",
                self.project,
                max_rows + 1
            );
            if let Some(l) = v["jobReference"]["location"].as_str() {
                path.push_str(&format!("&location={l}"));
            }
            v = self.call(reqwest::Method::GET, &path, None).await?;
        }
        Ok(v)
    }
}

/// A BigQuery cell, typed by its field.
fn cell(v: &Value, ty: &str) -> Value {
    let Some(s) = v.as_str() else {
        return v.clone();
    };
    match ty {
        "INTEGER" | "INT64" => s
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or_else(|_| json!(s)),
        "FLOAT" | "FLOAT64" | "NUMERIC" | "BIGNUMERIC" => s
            .parse::<f64>()
            .map(Value::from)
            .unwrap_or_else(|_| json!(s)),
        "BOOLEAN" | "BOOL" => json!(s == "true"),
        "TIMESTAMP" => s
            .parse::<f64>()
            .ok()
            .and_then(|secs| {
                chrono::DateTime::from_timestamp(
                    (secs.floor()) as i64,
                    ((secs.fract()) * 1e9) as u32,
                )
            })
            .map(|t| json!(t.to_rfc3339()))
            .unwrap_or_else(|| json!(s)),
        _ => json!(s),
    }
}

#[async_trait]
impl Database for Bq {
    async fn schema(&self) -> Result<Schema> {
        let datasets: Vec<String> = match &self.dataset {
            Some(d) => vec![d.clone()],
            None => {
                let v = self
                    .call(
                        reqwest::Method::GET,
                        &format!(
                            "/bigquery/v2/projects/{}/datasets?maxResults=10",
                            self.project
                        ),
                        None,
                    )
                    .await?;
                v["datasets"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|d| {
                        d["datasetReference"]["datasetId"]
                            .as_str()
                            .map(str::to_owned)
                    })
                    .collect()
            }
        };
        let mut tables: Vec<Table> = Vec::new();
        for d in datasets {
            let sql = format!(
                "SELECT c.table_name, c.column_name, c.data_type, c.is_nullable, t.table_type
                 FROM `{p}.{d}`.INFORMATION_SCHEMA.COLUMNS c JOIN `{p}.{d}`.INFORMATION_SCHEMA.TABLES t USING (table_name)
                 ORDER BY c.table_name, c.ordinal_position",
                p = self.project
            );
            let v = self.run(&sql, 20_000).await?;
            for row in v["rows"].as_array().into_iter().flatten() {
                let f = |i: usize| row["f"][i]["v"].as_str().unwrap_or_default().to_owned();
                let name = f(0);
                if tables
                    .last()
                    .is_none_or(|t| t.name != name || t.schema.as_deref() != Some(d.as_str()))
                {
                    tables.push(Table {
                        schema: Some(d.clone()),
                        name,
                        kind: if f(4).contains("VIEW") {
                            "view".into()
                        } else {
                            "table".into()
                        },
                        columns: Vec::new(),
                        rows: None,
                    });
                }
                tables
                    .last_mut()
                    .expect("a table")
                    .columns
                    .push(ColumnInfo {
                        name: f(1),
                        data_type: f(2),
                        nullable: f(3) == "YES",
                    });
            }
        }
        Ok(Schema { tables })
    }

    async fn query(&self, sql: &str, max_rows: usize) -> Result<QueryResult> {
        self.check(sql).await?;
        let v = self.run(sql, max_rows).await?;
        let fields = v["schema"]["fields"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let columns: Vec<Column> = fields
            .iter()
            .map(|f| Column {
                name: f["name"].as_str().unwrap_or_default().to_owned(),
                data_type: f["type"].as_str().unwrap_or_default().to_lowercase(),
            })
            .collect();
        let all = v["rows"].as_array().cloned().unwrap_or_default();
        let total: u64 = v["totalRows"]
            .as_str()
            .and_then(|t| t.parse().ok())
            .unwrap_or(all.len() as u64);
        let rows = all
            .iter()
            .take(max_rows)
            .map(|r| {
                fields
                    .iter()
                    .enumerate()
                    .map(|(i, f)| cell(&r["f"][i]["v"], f["type"].as_str().unwrap_or_default()))
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
        let (kind, bytes) = self.check(sql).await?;
        Ok(format!("{kind}: would scan {:.1} MB", bytes as f64 / 1e6))
    }

    async fn execute(&self, sql: &str) -> Result<u64> {
        let v = self.run(sql, 0).await?;
        Ok(v["numDmlAffectedRows"]
            .as_str()
            .and_then(|n| n.parse().ok())
            .unwrap_or(0))
    }
}
