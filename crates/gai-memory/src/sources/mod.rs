//! Sources: keep a knowledge collection in step with somewhere else.
//!
//! A source lists its items with a version each (a file's size and time,
//! a page's version number, a git blob's hash). A sync fetches and ingests
//! only items that are new or changed, and removes documents whose items
//! are gone, so running it often is cheap.
//!
//! ```yaml
//! name: handbook
//! collection: handbook
//! kind: confluence
//! base_url: https://acme.atlassian.net/wiki
//! space: HR
//! email_env: CONFLUENCE_EMAIL
//! token_env: CONFLUENCE_TOKEN
//! every_minutes: 60
//! ```
//!
//! Kinds: `folder`, `web` (pages and sitemaps), `github`, `confluence` and
//! `notion`. Items keep who may see them (`acl`) in the document metadata
//! where the source says so.

mod confluence;
mod folder;
mod github;
mod notion;
mod web;

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use gai_store::SourceRecord;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Knowledge, MemoryError, Result};

/// A source and where it syncs to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SourceSpec {
    /// Letters, digits, `-`, `_` and `.`.
    pub name: String,
    /// The knowledge collection it fills (created if missing).
    pub collection: String,
    /// Sync this often while `gai serve` runs; none for by hand only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every_minutes: Option<u32>,
    /// Stop after this many items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_items: Option<usize>,
    /// Who may find what it brings in: `team:<name>`, `user:<email>`,
    /// `role:<role>` (that role or higher). Empty: everyone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acl: Vec<String>,
    #[serde(flatten)]
    pub kind: SourceKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceKind {
    /// Files in a folder on the server (text, Markdown, HTML, PDF, code).
    Folder {
        path: String,
        #[serde(default = "yes")]
        recursive: bool,
    },
    /// Web pages: the given URLs, the pages in a sitemap, and pages they
    /// link to on the same site, up to `depth` links away.
    Web {
        #[serde(default)]
        urls: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sitemap: Option<String>,
        #[serde(default)]
        depth: u32,
        #[serde(default = "max_pages")]
        max_pages: usize,
        /// Let it fetch localhost and private addresses.
        #[serde(default)]
        allow_private: bool,
    },
    /// Files in a GitHub repository.
    Github {
        /// `owner/name`.
        repo: String,
        /// Defaults to the repository's default branch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        /// Only files under this folder.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        /// Environment variable with a token (needed for private repos).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token_env: Option<String>,
        /// For GitHub Enterprise: `https://github.example.com/api/v3`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_url: Option<String>,
    },
    /// Pages in a Confluence space.
    Confluence {
        /// `https://<site>.atlassian.net/wiki`, or a Data Center base URL.
        base_url: String,
        /// The space key.
        space: String,
        /// Environment variable with the account email (Cloud). Without it
        /// the token is sent as a bearer token (Data Center).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        email_env: Option<String>,
        token_env: String,
    },
    /// Notion pages: a database's pages, given pages, or every page shared
    /// with the integration.
    Notion {
        token_env: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        database_id: Option<String>,
        #[serde(default)]
        page_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_url: Option<String>,
    },
}

fn yes() -> bool {
    true
}

fn max_pages() -> usize {
    200
}

impl SourceKind {
    pub fn name(&self) -> &'static str {
        match self {
            SourceKind::Folder { .. } => "folder",
            SourceKind::Web { .. } => "web",
            SourceKind::Github { .. } => "github",
            SourceKind::Confluence { .. } => "confluence",
            SourceKind::Notion { .. } => "notion",
        }
    }
}

/// One thing a source holds.
#[derive(Clone, Debug, Default)]
pub struct Item {
    /// Stable and unique in the source: becomes the document's `source`.
    pub key: String,
    /// Changes whenever the content does.
    pub version: String,
    pub title: Option<String>,
    /// Where people can open it.
    pub url: Option<String>,
    /// Who may see it, when the source says.
    pub acl: Vec<String>,
    /// Content found while listing, so `fetch` isn't needed.
    pub content: Option<Fetched>,
}

#[derive(Clone, Debug)]
pub struct Fetched {
    /// A file name, for picking the loader (`page.html`, `notes.md`).
    pub name: String,
    pub content_type: Option<String>,
    pub bytes: Vec<u8>,
}

#[async_trait]
pub trait Connector: Send + Sync {
    async fn list(&self) -> Result<Vec<Item>>;
    async fn fetch(&self, item: &Item) -> Result<Fetched>;
}

/// What one sync did.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SyncStats {
    pub added: u64,
    pub updated: u64,
    pub removed: u64,
    pub unchanged: u64,
    pub failed: u64,
    /// The first few failures.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

/// Settings holding an organization's secrets: `secret:<NAME>`.
pub const SECRET_PREFIX: &str = "secret:";

pub(crate) fn env(var: &str) -> Result<String> {
    std::env::var(var)
        .ok()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| MemoryError::Invalid(format!("set {var} for this source")))
}

pub(crate) fn invalid(e: impl std::fmt::Display) -> MemoryError {
    MemoryError::Invalid(e.to_string())
}

impl Sources {
    async fn connector(&self, spec: &SourceSpec) -> Result<Box<dyn Connector>> {
        let http = &self.http;
        Ok(match &spec.kind {
            SourceKind::Folder { path, recursive } => {
                let path = self.folder_path(path)?;
                Box::new(folder::Folder::new(&path.to_string_lossy(), *recursive))
            }
            SourceKind::Web {
                urls,
                sitemap,
                depth,
                max_pages,
                allow_private,
            } => Box::new(web::Web::new(
                http.clone(),
                urls.clone(),
                sitemap.clone(),
                *depth,
                *max_pages,
                *allow_private && self.confined.is_none(),
            )),
            SourceKind::Github {
                repo,
                branch,
                path,
                token_env,
                api_url,
            } => Box::new(github::Github::new(
                http.clone(),
                repo,
                branch.clone(),
                path.clone(),
                self.secret_opt(token_env.as_deref()).await?,
                api_url.clone(),
            )?),
            SourceKind::Confluence {
                base_url,
                space,
                email_env,
                token_env,
            } => Box::new(confluence::Confluence::new(
                http.clone(),
                base_url,
                space,
                self.secret_opt(email_env.as_deref()).await?,
                self.secret(token_env).await?,
            )),
            SourceKind::Notion {
                token_env,
                database_id,
                page_ids,
                api_url,
            } => Box::new(notion::Notion::new(
                http.clone(),
                self.secret(token_env).await?,
                database_id.clone(),
                page_ids.clone(),
                api_url.clone(),
            )),
        })
    }
}

/// Sources: saving them, and syncing them into knowledge.
#[derive(Clone)]
pub struct Sources {
    knowledge: Arc<Knowledge>,
    http: reqwest::Client,
    /// Set for an organization sharing the server: folders only under this
    /// one, no private addresses, and tokens from the organization's
    /// secrets instead of the server's environment.
    confined: Option<PathBuf>,
}

impl Sources {
    pub fn new(knowledge: Arc<Knowledge>) -> Self {
        Self {
            knowledge,
            http: reqwest::Client::builder()
                .user_agent(concat!("gai/", env!("CARGO_PKG_VERSION")))
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("http client"),
            confined: None,
        }
    }

    /// Confine sources to `root` (see [`Sources`]).
    pub fn confined(mut self, root: PathBuf) -> Self {
        self.confined = Some(root);
        self
    }

    /// Where a folder source reads. Confined, it is a path inside the
    /// root (relative, or absolute under it), without `..`.
    fn folder_path(&self, path: &str) -> Result<PathBuf> {
        let Some(root) = &self.confined else {
            return Ok(PathBuf::from(path));
        };
        let p = Path::new(path);
        let rel = p.strip_prefix(root).unwrap_or(p);
        if rel
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
        {
            return Err(invalid(format!(
                "folder {path}: use a folder inside {} (relative paths start there)",
                root.display()
            )));
        }
        Ok(root.join(rel))
    }

    /// A token or account named by a `*_env` field: the environment
    /// variable, or, confined, the organization's secret of that name.
    async fn secret(&self, var: &str) -> Result<String> {
        if self.confined.is_none() {
            return env(var);
        }
        self.knowledge
            .store()
            .get_setting(&format!("{SECRET_PREFIX}{var}"))
            .await?
            .filter(|v| !v.is_empty())
            .ok_or_else(|| invalid(format!("save a secret named {var} for this source")))
    }

    async fn secret_opt(&self, var: Option<&str>) -> Result<Option<String>> {
        match var {
            Some(v) => self.secret(v).await.map(Some),
            None => Ok(None),
        }
    }

    pub fn parse(value: Value) -> Result<SourceSpec> {
        let spec: SourceSpec =
            serde_json::from_value(value).map_err(|e| invalid(format!("source: {e}")))?;
        Knowledge::validate_name(&spec.name)?;
        Knowledge::validate_name(&spec.collection)?;
        crate::knowledge::validate_acl(&spec.acl)?;
        Ok(spec)
    }

    pub async fn save(&self, spec: &SourceSpec) -> Result<()> {
        Knowledge::validate_name(&spec.name)?;
        Knowledge::validate_name(&spec.collection)?;
        if self.confined.is_some() {
            match &spec.kind {
                SourceKind::Folder { path, .. } => {
                    self.folder_path(path)?;
                }
                SourceKind::Web {
                    allow_private: true,
                    ..
                } => {
                    return Err(invalid(
                        "allow_private isn't available to this organization",
                    ));
                }
                _ => {}
            }
        }
        crate::knowledge::validate_acl(&spec.acl)?;
        let value = serde_json::to_value(spec).expect("serializable spec");
        let store = self.knowledge.store();
        let before = store.get_source(&spec.name).await?;
        store
            .put_source(&spec.name, &spec.collection, spec.kind.name(), &value)
            .await?;
        // A new acl must reach every item, changed or not: forget the versions.
        if let Some(b) = before
            && b.spec.get("acl") != value.get("acl")
        {
            store
                .save_source_sync(
                    &spec.name,
                    &b.status,
                    b.error.as_deref(),
                    &b.stats,
                    Some(&json!({})),
                )
                .await?;
        }
        Ok(())
    }

    pub async fn get(&self, name: &str) -> Result<SourceRecord> {
        self.knowledge
            .store()
            .get_source(name)
            .await?
            .ok_or_else(|| MemoryError::NotFound(format!("source {name}")))
    }

    /// Bring the collection in step with the source.
    pub async fn sync(&self, name: &str) -> Result<SyncStats> {
        let record = self.get(name).await?;
        let spec = Self::parse(record.spec.clone())?;
        let store = self.knowledge.store().clone();
        store
            .save_source_sync(name, "syncing", None, &record.stats, None)
            .await?;
        let result = self.run(&spec, &record).await;
        match result {
            Ok((stats, state)) => {
                let value = serde_json::to_value(&stats).expect("stats");
                let status =
                    if stats.failed > 0 && stats.added + stats.updated + stats.unchanged == 0 {
                        "failed"
                    } else {
                        "ok"
                    };
                let error = stats.errors.first().map(String::as_str);
                store
                    .save_source_sync(name, status, error, &value, Some(&state))
                    .await?;
                Ok(stats)
            }
            Err(e) => {
                store
                    .save_source_sync(name, "failed", Some(&e.to_string()), &json!({}), None)
                    .await?;
                Err(e)
            }
        }
    }

    async fn run(&self, spec: &SourceSpec, record: &SourceRecord) -> Result<(SyncStats, Value)> {
        let k = &self.knowledge;
        k.ensure_collection(
            &spec.collection,
            &format!("Synced from the {} source {}", spec.kind.name(), spec.name),
        )
        .await?;
        let conn = self.connector(spec).await?;
        let mut items = conn.list().await?;
        if let Some(max) = spec.max_items {
            items.truncate(max);
        }
        let old: BTreeMap<String, String> =
            serde_json::from_value(record.state.clone()).unwrap_or_default();
        let mut state = BTreeMap::new();
        let mut stats = SyncStats::default();
        let store = k.store().clone();
        for item in &items {
            let known = old.get(&item.key);
            let exists = store
                .find_document(&spec.collection, &item.key)
                .await?
                .is_some();
            if known == Some(&item.version) && exists {
                stats.unchanged += 1;
                state.insert(item.key.clone(), item.version.clone());
                continue;
            }
            let fetched = match &item.content {
                Some(c) => Ok(c.clone()),
                None => conn.fetch(item).await,
            };
            let outcome = match fetched {
                Ok(f) => {
                    let mut meta = json!({"source": spec.name, "version": item.version});
                    if let Some(u) = &item.url {
                        meta["url"] = json!(u);
                    }
                    if !item.acl.is_empty() {
                        meta["acl"] = json!(item.acl);
                    } else if !spec.acl.is_empty() {
                        meta["acl"] = json!(spec.acl);
                    }
                    k.ingest_bytes(
                        &spec.collection,
                        &f.name,
                        f.content_type.as_deref(),
                        f.bytes,
                        Some(item.key.clone()),
                        meta,
                    )
                    .await
                }
                Err(e) => Err(e),
            };
            match outcome {
                Ok(_) => {
                    if exists {
                        stats.updated += 1;
                    } else {
                        stats.added += 1;
                    }
                    state.insert(item.key.clone(), item.version.clone());
                }
                Err(e) => {
                    stats.failed += 1;
                    if stats.errors.len() < 5 {
                        stats.errors.push(format!("{}: {e}", item.key));
                    }
                    // Keep the old version so the next sync tries again.
                    if let Some(v) = known {
                        state.insert(item.key.clone(), format!("{v}~retry"));
                    }
                }
            }
        }
        for key in old.keys().filter(|k| !state.contains_key(*k)) {
            if let Some(doc) = store.find_document(&spec.collection, key).await? {
                k.delete_document(&doc.id).await?;
                stats.removed += 1;
            }
        }
        Ok((stats, serde_json::to_value(state).expect("state")))
    }

    /// Sources whose `every_minutes` has passed since their last sync.
    pub async fn due(&self) -> Result<Vec<String>> {
        let now = chrono::Utc::now();
        Ok(self
            .knowledge
            .store()
            .list_sources()
            .await?
            .into_iter()
            .filter(|r| r.status != "syncing")
            .filter(|r| {
                let every = r.spec["every_minutes"].as_u64().unwrap_or(0);
                every > 0
                    && r.last_sync_at
                        .is_none_or(|t| now - t >= chrono::Duration::minutes(every as i64))
            })
            .map(|r| r.name)
            .collect())
    }
}
