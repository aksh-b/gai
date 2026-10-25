//! Files in a GitHub repository, through the REST API: the branch's tree
//! (one call), then each new or changed blob. Blob hashes are the versions.

use std::path::Path;

use async_trait::async_trait;
use base64::Engine;
use serde_json::Value;

use super::{Connector, Fetched, Item, invalid};
use crate::Result;

pub struct Github {
    http: reqwest::Client,
    api: String,
    repo: String,
    branch: Option<String>,
    prefix: Option<String>,
    token: Option<String>,
}

impl Github {
    pub fn new(
        http: reqwest::Client,
        repo: &str,
        branch: Option<String>,
        prefix: Option<String>,
        token: Option<String>,
        api_url: Option<String>,
    ) -> Result<Self> {
        if repo.split('/').count() != 2 {
            return Err(invalid(format!("repo {repo:?}: use owner/name")));
        }
        Ok(Self {
            http,
            api: api_url
                .unwrap_or_else(|| "https://api.github.com".into())
                .trim_end_matches('/')
                .to_owned(),
            repo: repo.to_owned(),
            branch,
            prefix: prefix
                .map(|p| p.trim_matches('/').to_owned())
                .filter(|p| !p.is_empty()),
            token,
        })
    }

    async fn get(&self, path: &str) -> Result<Value> {
        let mut rb = self
            .http
            .get(format!("{}{path}", self.api))
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28");
        if let Some(t) = &self.token {
            rb = rb.bearer_auth(t);
        }
        let r = rb.send().await.map_err(invalid)?;
        let status = r.status();
        let v: Value = r.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(invalid(format!(
                "GitHub {path}: {status} {}",
                v["message"].as_str().unwrap_or("")
            )));
        }
        Ok(v)
    }

    async fn branch(&self) -> Result<String> {
        if let Some(b) = &self.branch {
            return Ok(b.clone());
        }
        let r = self.get(&format!("/repos/{}", self.repo)).await?;
        r["default_branch"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| invalid("GitHub didn't say the default branch"))
    }
}

#[async_trait]
impl Connector for Github {
    async fn list(&self) -> Result<Vec<Item>> {
        let branch = self.branch().await?;
        let tree = self
            .get(&format!(
                "/repos/{}/git/trees/{branch}?recursive=1",
                self.repo
            ))
            .await?;
        if tree["truncated"].as_bool() == Some(true) {
            tracing::warn!(repo = %self.repo, "GitHub truncated the tree; set path to sync a folder");
        }
        Ok(tree["tree"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|e| e["type"] == "blob")
            .filter_map(|e| {
                let path = e["path"].as_str()?;
                if let Some(p) = &self.prefix
                    && !(path == p || path.starts_with(&format!("{p}/")))
                {
                    return None;
                }
                if !crate::load::is_supported(Path::new(path)) {
                    return None;
                }
                let url = format!("https://github.com/{}/blob/{branch}/{path}", self.repo);
                Some(Item {
                    key: url.clone(),
                    version: e["sha"].as_str()?.to_owned(),
                    title: Some(path.to_owned()),
                    url: Some(url),
                    acl: Vec::new(),
                    content: None,
                })
            })
            .collect())
    }

    async fn fetch(&self, item: &Item) -> Result<Fetched> {
        let blob = self
            .get(&format!("/repos/{}/git/blobs/{}", self.repo, item.version))
            .await?;
        let b64: String = blob["content"]
            .as_str()
            .unwrap_or_default()
            .split_whitespace()
            .collect();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| invalid(format!("{}: {e}", item.key)))?;
        let name = item
            .title
            .as_deref()
            .and_then(|p| p.rsplit('/').next())
            .unwrap_or("file")
            .to_owned();
        Ok(Fetched {
            name,
            content_type: None,
            bytes,
        })
    }
}
