//! Databases for gai agents. A connection names a database (Postgres,
//! MySQL, SQLite, BigQuery or Snowflake) and how to sign in, by the
//! environment variable or secret that holds the password or token, never
//! the value. Connections are read-only unless they say otherwise: queries
//! run in read-only transactions where the database has them, and are
//! checked to be one `SELECT` first. Schemas are read from the database and
//! kept, so agents can write SQL against them.

mod bigquery;
mod mysql;
mod pg;
mod snowflake;
pub mod sql;
mod sqlite;
pub mod tools;

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use gai_store::{DataConnection, SharedStore, StoreError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum DataError {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    /// Not allowed on this connection (a write on a read-only one).
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    Database(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub type Result<T> = std::result::Result<T, DataError>;

impl From<sqlx::Error> for DataError {
    fn from(e: sqlx::Error) -> Self {
        match e.as_database_error() {
            Some(d) => DataError::Database(d.message().to_owned()),
            None => DataError::Database(e.to_string()),
        }
    }
}

fn invalid(e: impl std::fmt::Display) -> DataError {
    DataError::Invalid(e.to_string())
}

fn yes() -> bool {
    true
}

fn default_rows() -> usize {
    1000
}

fn default_timeout() -> u64 {
    30
}

/// A database agents can query.
///
/// ```yaml
/// name: app
/// kind: postgres
/// url: postgres://reader@db.internal:5432/app
/// password_env: APP_DB_PASSWORD
/// description: Users, signups and orders
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ConnectionSpec {
    /// Letters, digits, `-` and `_`.
    pub name: String,
    /// What is in it, for agents choosing a connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Refuse anything but queries (default true).
    #[serde(default = "yes")]
    pub read_only: bool,
    /// Rows returned by one query at most.
    #[serde(default = "default_rows")]
    pub max_rows: usize,
    /// Seconds one query may take.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(flatten)]
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Kind {
    Postgres {
        /// `postgres://user@host:5432/db` (no password: use `password_env`).
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        password_env: Option<String>,
    },
    Mysql {
        /// `mysql://user@host:3306/db`.
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        password_env: Option<String>,
    },
    Sqlite {
        /// A database file on the server.
        path: String,
    },
    Bigquery {
        project: String,
        /// The dataset unqualified table names are in, and whose schema is
        /// read (default: every dataset, up to 10).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dataset: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        location: Option<String>,
        /// Environment variable (or secret) with a service account's JSON key.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        credentials_env: Option<String>,
        /// Or one with an OAuth access token.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token_env: Option<String>,
        /// Refuse queries that would scan more than this (default 10 GB).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_bytes_billed: Option<u64>,
        /// For tests and emulators.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_url: Option<String>,
    },
    Snowflake {
        /// The account identifier, like `myorg-myaccount`.
        account: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        database: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        schema: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        warehouse: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        /// Environment variable (or secret) with a programmatic access
        /// token or OAuth token.
        token_env: String,
        /// `PROGRAMMATIC_ACCESS_TOKEN` (default) or `OAUTH`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token_type: Option<String>,
        /// For tests: instead of `https://<account>.snowflakecomputing.com`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_url: Option<String>,
    },
}

impl Kind {
    pub fn name(&self) -> &'static str {
        match self {
            Kind::Postgres { .. } => "postgres",
            Kind::Mysql { .. } => "mysql",
            Kind::Sqlite { .. } => "sqlite",
            Kind::Bigquery { .. } => "bigquery",
            Kind::Snowflake { .. } => "snowflake",
        }
    }
}

/// A database's tables and their columns.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Schema {
    pub tables: Vec<Table>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Table {
    /// The schema or dataset it is in, when the database has them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub name: String,
    /// `table` or `view`.
    pub kind: String,
    pub columns: Vec<ColumnInfo>,
    /// About how many rows, when the database says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
}

impl Schema {
    /// As compact text for a model: one line per table.
    pub fn describe(&self, only: &[String]) -> String {
        let mut out = String::new();
        for t in &self.tables {
            let full = match &t.schema {
                Some(s) => format!("{s}.{}", t.name),
                None => t.name.clone(),
            };
            if !only.is_empty()
                && !only
                    .iter()
                    .any(|o| o.eq_ignore_ascii_case(&t.name) || o.eq_ignore_ascii_case(&full))
            {
                continue;
            }
            let cols: Vec<String> = t
                .columns
                .iter()
                .map(|c| {
                    format!(
                        "{} {}{}",
                        c.name,
                        c.data_type.to_lowercase(),
                        if c.nullable { "" } else { " not null" }
                    )
                })
                .collect();
            let rows = t.rows.map(|r| format!(" (~{r} rows)")).unwrap_or_default();
            let kind = if t.kind == "view" { " view" } else { "" };
            out.push_str(&format!("{full}{kind}{rows}: {}\n", cols.join(", ")));
        }
        out
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Column {
    pub name: String,
    /// The database's type name.
    pub data_type: String,
}

/// What a query returned: at most the connection's `max_rows`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QueryResult {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Value>>,
    /// There were more rows than returned.
    pub truncated: bool,
    pub elapsed_ms: u64,
}

impl QueryResult {
    /// The rows as a Markdown table (the first `max`).
    pub fn markdown(&self, max: usize) -> String {
        if self.columns.is_empty() {
            return "(no columns)".into();
        }
        let esc = |s: String| s.replace('|', "\\|").replace('\n', " ");
        let mut out = format!(
            "| {} |\n|{}\n",
            self.columns
                .iter()
                .map(|c| esc(c.name.clone()))
                .collect::<Vec<_>>()
                .join(" | "),
            " --- |".repeat(self.columns.len())
        );
        for r in self.rows.iter().take(max) {
            let cells: Vec<String> = r
                .iter()
                .map(|v| match v {
                    Value::Null => "".into(),
                    Value::String(s) => esc(s.clone()),
                    v => esc(v.to_string()),
                })
                .collect();
            out.push_str(&format!("| {} |\n", cells.join(" | ")));
        }
        out
    }

    pub fn csv(&self) -> String {
        let cell = |s: String| {
            if s.contains([',', '"', '\n', '\r']) {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s
            }
        };
        let mut out = self
            .columns
            .iter()
            .map(|c| cell(c.name.clone()))
            .collect::<Vec<_>>()
            .join(",");
        out.push('\n');
        for r in &self.rows {
            let cells: Vec<String> = r
                .iter()
                .map(|v| match v {
                    Value::Null => String::new(),
                    Value::String(s) => cell(s.clone()),
                    v => cell(v.to_string()),
                })
                .collect();
            out.push_str(&cells.join(","));
            out.push('\n');
        }
        out
    }
}

/// One open database.
#[async_trait]
pub(crate) trait Database: Send + Sync {
    async fn schema(&self) -> Result<Schema>;
    /// Run one checked query, returning at most `max_rows`.
    async fn query(&self, sql: &str, max_rows: usize) -> Result<QueryResult>;
    /// What the database would do, without running it.
    async fn explain(&self, sql: &str) -> Result<String>;
    /// Run a statement that changes data; rows affected.
    async fn execute(&self, sql: &str) -> Result<u64>;
}

/// Every connection of one gai instance (an organization's, when confined).
#[derive(Clone)]
pub struct Data {
    inner: Arc<Inner>,
}

/// An open database and the spec it was opened with.
type Opened = (ConnectionSpec, Arc<dyn Database>);

struct Inner {
    store: SharedStore,
    /// Set for an organization sharing the server: SQLite files only under
    /// this folder, no private hosts or custom API URLs, and passwords from
    /// its own secrets instead of the server's environment.
    confined: Option<PathBuf>,
    open: Mutex<HashMap<String, Opened>>,
    http: reqwest::Client,
}

/// Settings holding an organization's secrets (see gai-memory).
const SECRET_PREFIX: &str = "secret:";

fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 64
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl Data {
    pub fn new(store: SharedStore) -> Self {
        Self {
            inner: Arc::new(Inner {
                store,
                confined: None,
                open: Mutex::default(),
                http: reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(120))
                    .build()
                    .expect("http client"),
            }),
        }
    }

    /// Confine connections to `root` (see [`Data`]).
    pub fn confined(store: SharedStore, root: PathBuf) -> Self {
        let mut d = Self::new(store);
        Arc::get_mut(&mut d.inner).expect("new").confined = Some(root);
        d
    }

    pub fn store(&self) -> &SharedStore {
        &self.inner.store
    }

    /// Read and check a connection spec.
    pub fn parse(value: Value) -> Result<ConnectionSpec> {
        let spec: ConnectionSpec =
            serde_json::from_value(value).map_err(|e| invalid(format!("connection: {e}")))?;
        if !valid_name(&spec.name) {
            return Err(invalid(
                "connection names use letters, digits, '-' and '_' (at most 64)",
            ));
        }
        if spec.max_rows == 0 || spec.max_rows > 100_000 {
            return Err(invalid("max_rows: 1 to 100000"));
        }
        if let Kind::Postgres { url, .. } | Kind::Mysql { url, .. } = &spec.kind {
            let u = url::Url::parse(url).map_err(|e| invalid(format!("url: {e}")))?;
            if u.password().is_some() {
                return Err(invalid(
                    "put the password in password_env, not in the url (urls are shown to users)",
                ));
            }
        }
        Ok(spec)
    }

    fn check_confined(&self, spec: &ConnectionSpec) -> Result<()> {
        let Some(root) = &self.inner.confined else {
            return Ok(());
        };
        match &spec.kind {
            Kind::Sqlite { path } => {
                self.sqlite_path(path, Some(root))?;
            }
            Kind::Postgres { url, .. } | Kind::Mysql { url, .. } => {
                let u = url::Url::parse(url).map_err(invalid)?;
                let host = u.host_str().unwrap_or_default();
                if gai_tools::builtin::is_private_host(host, u.port().unwrap_or(5432)) {
                    return Err(DataError::Refused("databases on private network addresses aren't available to this organization".into()));
                }
            }
            Kind::Bigquery {
                api_url: Some(_), ..
            }
            | Kind::Snowflake {
                api_url: Some(_), ..
            } => {
                return Err(DataError::Refused(
                    "api_url isn't available to this organization".into(),
                ));
            }
            _ => {}
        }
        Ok(())
    }

    fn sqlite_path(&self, path: &str, root: Option<&PathBuf>) -> Result<PathBuf> {
        let Some(root) = root else {
            return Ok(PathBuf::from(path));
        };
        let p = Path::new(path);
        let rel = p.strip_prefix(root).unwrap_or(p);
        if rel
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
        {
            return Err(DataError::Refused(format!(
                "{path}: use a database file inside {} (relative paths start there)",
                root.display()
            )));
        }
        Ok(root.join(rel))
    }

    pub async fn save(&self, spec: &ConnectionSpec) -> Result<()> {
        self.check_confined(spec)?;
        let value = serde_json::to_value(spec).expect("serializable spec");
        self.inner
            .store
            .put_connection(&spec.name, spec.kind.name(), &value)
            .await?;
        self.inner.open.lock().await.remove(&spec.name);
        Ok(())
    }

    pub async fn get(&self, name: &str) -> Result<DataConnection> {
        self.inner
            .store
            .get_connection(name)
            .await?
            .ok_or_else(|| DataError::NotFound(format!("no connection {name}")))
    }

    pub async fn list(&self) -> Result<Vec<DataConnection>> {
        Ok(self.inner.store.list_connections().await?)
    }

    pub async fn delete(&self, name: &str) -> Result<bool> {
        self.inner.open.lock().await.remove(name);
        Ok(self.inner.store.delete_connection(name).await?)
    }

    pub async fn spec(&self, name: &str) -> Result<ConnectionSpec> {
        Self::parse(self.get(name).await?.spec)
    }

    /// A password or token: the environment variable, or (confined) the
    /// organization's secret of that name.
    async fn secret(&self, var: &str) -> Result<String> {
        if self.inner.confined.is_none() {
            return std::env::var(var)
                .ok()
                .filter(|v| !v.is_empty())
                .ok_or_else(|| invalid(format!("set {var} for this connection")));
        }
        self.inner
            .store
            .get_setting(&format!("{SECRET_PREFIX}{var}"))
            .await?
            .filter(|v| !v.is_empty())
            .ok_or_else(|| invalid(format!("save a secret named {var} for this connection")))
    }

    async fn open(&self, name: &str) -> Result<(ConnectionSpec, Arc<dyn Database>)> {
        let spec = self.spec(name).await?;
        let mut open = self.inner.open.lock().await;
        if let Some((s, db)) = open.get(name)
            && *s == spec
        {
            return Ok((spec, db.clone()));
        }
        self.check_confined(&spec)?;
        let timeout = spec.timeout_secs.clamp(1, 600);
        let db: Arc<dyn Database> = match &spec.kind {
            Kind::Postgres { url, password_env } => {
                let pw = match password_env {
                    Some(v) => Some(self.secret(v).await?),
                    None => None,
                };
                Arc::new(pg::Pg::connect(url, pw, timeout).await?)
            }
            Kind::Mysql { url, password_env } => {
                let pw = match password_env {
                    Some(v) => Some(self.secret(v).await?),
                    None => None,
                };
                Arc::new(mysql::My::connect(url, pw, timeout).await?)
            }
            Kind::Sqlite { path } => {
                let p = self.sqlite_path(path, self.inner.confined.as_ref())?;
                Arc::new(sqlite::Lite::open(&p, spec.read_only, timeout).await?)
            }
            Kind::Bigquery {
                project,
                dataset,
                location,
                credentials_env,
                token_env,
                max_bytes_billed,
                api_url,
            } => {
                let auth = match (credentials_env, token_env) {
                    (Some(c), _) => bigquery::Auth::ServiceAccount(self.secret(c).await?),
                    (None, Some(t)) => bigquery::Auth::Token(self.secret(t).await?),
                    (None, None) => {
                        return Err(invalid(
                            "a bigquery connection needs credentials_env or token_env",
                        ));
                    }
                };
                Arc::new(bigquery::Bq::new(
                    self.inner.http.clone(),
                    auth,
                    project.clone(),
                    dataset.clone(),
                    location.clone(),
                    max_bytes_billed.unwrap_or(10_000_000_000),
                    api_url.clone(),
                    spec.read_only,
                    timeout,
                ))
            }
            Kind::Snowflake {
                account,
                database,
                schema,
                warehouse,
                role,
                token_env,
                token_type,
                api_url,
            } => Arc::new(snowflake::Sf::new(
                self.inner.http.clone(),
                self.secret(token_env).await?,
                token_type
                    .clone()
                    .unwrap_or_else(|| "PROGRAMMATIC_ACCESS_TOKEN".into()),
                api_url
                    .clone()
                    .unwrap_or_else(|| format!("https://{account}.snowflakecomputing.com")),
                database.clone(),
                schema.clone(),
                warehouse.clone(),
                role.clone(),
                timeout,
            )),
        };
        open.insert(name.to_owned(), (spec.clone(), db.clone()));
        Ok((spec, db))
    }

    /// The connection's schema: the one kept from last time, or read from
    /// the database (and kept) when there is none or `refresh`.
    pub async fn schema(&self, name: &str, refresh: bool) -> Result<Schema> {
        if !refresh
            && let Some(s) = self.get(name).await?.schema
            && let Ok(s) = serde_json::from_value::<Schema>(s)
        {
            return Ok(s);
        }
        let (_, db) = self.open(name).await?;
        let s = db.schema().await?;
        self.inner
            .store
            .save_connection_schema(name, &serde_json::to_value(&s).expect("schema"))
            .await?;
        Ok(s)
    }

    /// Run one query (a `SELECT`, or `WITH ... SELECT`). At most `limit`
    /// rows, and never more than the connection's `max_rows`.
    pub async fn query(&self, name: &str, sql: &str, limit: Option<usize>) -> Result<QueryResult> {
        let (spec, db) = self.open(name).await?;
        let sql = sql::check_query(sql, spec.kind.name())?;
        let max = limit.unwrap_or(spec.max_rows).clamp(1, spec.max_rows);
        let started = std::time::Instant::now();
        let mut r = tokio::time::timeout(
            std::time::Duration::from_secs(spec.timeout_secs.clamp(1, 600) + 5),
            db.query(&sql, max),
        )
        .await
        .map_err(|_| {
            DataError::Database(format!("the query took over {} seconds", spec.timeout_secs))
        })??;
        r.elapsed_ms = started.elapsed().as_millis() as u64;
        Ok(r)
    }

    /// The plan (or BigQuery's byte estimate) for a query, without running it.
    pub async fn dry_run(&self, name: &str, sql: &str) -> Result<String> {
        let (spec, db) = self.open(name).await?;
        let sql = sql::check_query(sql, spec.kind.name())?;
        db.explain(&sql).await
    }

    /// Run one statement that changes data, on a connection that allows it.
    pub async fn execute(&self, name: &str, sql: &str) -> Result<u64> {
        let (spec, db) = self.open(name).await?;
        if spec.read_only {
            return Err(DataError::Refused(format!(
                "{name} is read-only: set read_only: false on the connection to change data"
            )));
        }
        let sql = sql::check_single(sql, spec.kind.name())?;
        db.execute(&sql).await
    }
}

/// Column names of a result must differ, to be turned into rows.
pub(crate) fn unique_columns(cols: &[Column]) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for c in cols {
        if !seen.insert(c.name.to_lowercase()) {
            return Err(invalid(format!(
                "two columns are called {:?}: give them different names with AS",
                c.name
            )));
        }
    }
    if cols.len() > 100 {
        return Err(invalid("select at most 100 columns"));
    }
    Ok(())
}

/// The rows of an array-per-row JSON result, cut at `max`.
pub(crate) fn rows_from_json(texts: Vec<Value>, max: usize) -> (Vec<Vec<Value>>, bool) {
    let truncated = texts.len() > max;
    let rows = texts
        .into_iter()
        .take(max)
        .map(|v| match v {
            Value::Array(a) => a,
            Value::String(s) => serde_json::from_str::<Vec<Value>>(&s).unwrap_or_default(),
            _ => Vec::new(),
        })
        .collect();
    (rows, truncated)
}
