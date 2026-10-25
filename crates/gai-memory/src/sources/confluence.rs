//! Pages in a Confluence space, through the v2 REST API. Page bodies come
//! with the listing (storage format, HTML); version numbers are the versions.

use async_trait::async_trait;
use base64::Engine;
use serde_json::Value;

use super::{Connector, Fetched, Item, invalid};
use crate::Result;

pub struct Confluence {
    http: reqwest::Client,
    base: String,
    space: String,
    auth: String,
}

impl Confluence {
    pub fn new(
        http: reqwest::Client,
        base_url: &str,
        space: &str,
        email: Option<String>,
        token: String,
    ) -> Self {
        let auth = match email {
            Some(e) => format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(format!("{e}:{token}"))
            ),
            None => format!("Bearer {token}"),
        };
        Self {
            http,
            base: base_url.trim_end_matches('/').to_owned(),
            space: space.to_owned(),
            auth,
        }
    }

    /// The site's origin, for `_links.next`, which is relative to it.
    fn origin(&self) -> String {
        match url::Url::parse(&self.base) {
            Ok(u) => u.origin().ascii_serialization(),
            Err(_) => self.base.clone(),
        }
    }

    async fn get(&self, url: &str) -> Result<Value> {
        let r = self
            .http
            .get(url)
            .header("authorization", &self.auth)
            .header("accept", "application/json")
            .send()
            .await
            .map_err(invalid)?;
        let status = r.status();
        let v: Value = r.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(invalid(format!(
                "Confluence: {status} {}",
                v["message"].as_str().unwrap_or("")
            )));
        }
        Ok(v)
    }
}

#[async_trait]
impl Connector for Confluence {
    async fn list(&self) -> Result<Vec<Item>> {
        let spaces = self
            .get(&format!("{}/api/v2/spaces?keys={}", self.base, self.space))
            .await?;
        let id = spaces["results"][0]["id"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| spaces["results"][0]["id"].as_i64().map(|i| i.to_string()))
            .ok_or_else(|| invalid(format!("no Confluence space {}", self.space)))?;
        let mut next = Some(format!(
            "{}/api/v2/spaces/{id}/pages?limit=100&body-format=storage",
            self.base
        ));
        let mut items = Vec::new();
        while let Some(url) = next.take() {
            let page = self.get(&url).await?;
            for p in page["results"].as_array().into_iter().flatten() {
                let Some(pid) = p["id"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| p["id"].as_i64().map(|i| i.to_string()))
                else {
                    continue;
                };
                let title = p["title"].as_str().unwrap_or_default().to_owned();
                let web = p["_links"]["webui"].as_str().unwrap_or_default();
                let link = format!("{}{web}", self.base);
                let html = format!(
                    "<html><head><title>{}</title></head><body>{}</body></html>",
                    title,
                    p["body"]["storage"]["value"].as_str().unwrap_or_default()
                );
                items.push(Item {
                    key: if web.is_empty() {
                        format!("{}/pages/{pid}", self.base)
                    } else {
                        link.clone()
                    },
                    version: p["version"]["number"].as_i64().unwrap_or(0).to_string(),
                    title: Some(title),
                    url: Some(link),
                    acl: Vec::new(),
                    content: Some(Fetched {
                        name: "page.html".into(),
                        content_type: Some("text/html".into()),
                        bytes: html.into_bytes(),
                    }),
                });
            }
            next = page["_links"]["next"].as_str().map(|n| {
                if n.starts_with("http") {
                    n.to_owned()
                } else {
                    format!("{}{n}", self.origin())
                }
            });
        }
        Ok(items)
    }

    async fn fetch(&self, item: &Item) -> Result<Fetched> {
        // Bodies always come with the listing.
        item.content
            .clone()
            .ok_or_else(|| invalid(format!("{}: no body", item.key)))
    }
}
