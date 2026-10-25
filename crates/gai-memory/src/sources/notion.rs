//! Notion pages through the public API: a database's pages, given pages,
//! or everything shared with the integration. `last_edited_time` is the
//! version; page text is read from its blocks (three levels deep).

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{Connector, Fetched, Item, invalid};
use crate::Result;

const NOTION_VERSION: &str = "2022-06-28";

pub struct Notion {
    http: reqwest::Client,
    api: String,
    token: String,
    database: Option<String>,
    pages: Vec<String>,
}

impl Notion {
    pub fn new(
        http: reqwest::Client,
        token: String,
        database: Option<String>,
        pages: Vec<String>,
        api_url: Option<String>,
    ) -> Self {
        Self {
            http,
            api: api_url
                .unwrap_or_else(|| "https://api.notion.com".into())
                .trim_end_matches('/')
                .to_owned(),
            token,
            database,
            pages,
        }
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
            .bearer_auth(&self.token)
            .header("notion-version", NOTION_VERSION);
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        let r = rb.send().await.map_err(invalid)?;
        let status = r.status();
        let v: Value = r.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(invalid(format!(
                "Notion {path}: {status} {}",
                v["message"].as_str().unwrap_or("")
            )));
        }
        Ok(v)
    }

    /// Every result of a paginated POST (search, database query).
    async fn all(&self, path: &str, mut body: Value) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        loop {
            let page = self
                .call(reqwest::Method::POST, path, Some(body.clone()))
                .await?;
            out.extend(page["results"].as_array().cloned().unwrap_or_default());
            match page["next_cursor"].as_str() {
                Some(c) if page["has_more"] == true => body["start_cursor"] = json!(c),
                _ => return Ok(out),
            }
        }
    }

    async fn blocks(&self, id: &str, depth: u32, out: &mut String) -> Result<()> {
        let mut cursor: Option<String> = None;
        loop {
            let mut path = format!("/v1/blocks/{id}/children?page_size=100");
            if let Some(c) = &cursor {
                path.push_str(&format!("&start_cursor={c}"));
            }
            let page = self.call(reqwest::Method::GET, &path, None).await?;
            for b in page["results"].as_array().into_iter().flatten() {
                let kind = b["type"].as_str().unwrap_or_default();
                let text: String = b[kind]["rich_text"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|t| t["plain_text"].as_str())
                    .collect();
                let line = match kind {
                    "heading_1" => format!("# {text}"),
                    "heading_2" => format!("## {text}"),
                    "heading_3" => format!("### {text}"),
                    "bulleted_list_item" | "numbered_list_item" => format!("- {text}"),
                    "to_do" => format!(
                        "- [{}] {text}",
                        if b["to_do"]["checked"] == true {
                            "x"
                        } else {
                            " "
                        }
                    ),
                    "quote" | "callout" => format!("> {text}"),
                    "code" => format!("```\n{text}\n```"),
                    _ => text,
                };
                if !line.trim().is_empty() {
                    out.push_str(&line);
                    out.push_str("\n\n");
                }
                if b["has_children"] == true
                    && depth < 3
                    && kind != "child_page"
                    && let Some(child) = b["id"].as_str()
                {
                    Box::pin(self.blocks(child, depth + 1, out)).await?;
                }
            }
            match page["next_cursor"].as_str() {
                Some(c) if page["has_more"] == true => cursor = Some(c.to_owned()),
                _ => return Ok(()),
            }
        }
    }
}

/// A page's title, from whichever property is the title.
fn title(page: &Value) -> Option<String> {
    page["properties"].as_object()?.values().find_map(|p| {
        (p["type"] == "title").then(|| {
            p["title"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| t["plain_text"].as_str())
                .collect::<String>()
        })
    })
}

fn item(page: &Value) -> Option<Item> {
    let id = page["id"].as_str()?;
    let url = page["url"].as_str().map(str::to_owned);
    Some(Item {
        key: url.clone().unwrap_or_else(|| format!("notion:{id}")),
        version: page["last_edited_time"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        title: title(page),
        url,
        acl: Vec::new(),
        content: None,
    })
}

#[async_trait]
impl Connector for Notion {
    async fn list(&self) -> Result<Vec<Item>> {
        let pages = if let Some(db) = &self.database {
            self.all(
                &format!("/v1/databases/{db}/query"),
                json!({"page_size": 100}),
            )
            .await?
        } else if !self.pages.is_empty() {
            let mut out = Vec::new();
            for p in &self.pages {
                out.push(
                    self.call(reqwest::Method::GET, &format!("/v1/pages/{p}"), None)
                        .await?,
                );
            }
            out
        } else {
            self.all(
                "/v1/search",
                json!({"page_size": 100, "filter": {"property": "object", "value": "page"}}),
            )
            .await?
        };
        Ok(pages
            .iter()
            .filter(|p| p["archived"] != true && p["in_trash"] != true)
            .filter_map(item)
            .collect())
    }

    async fn fetch(&self, item: &Item) -> Result<Fetched> {
        let id = item
            .key
            .rsplit(['-', ':', '/'])
            .next()
            .ok_or_else(|| invalid(format!("{}: no page id", item.key)))?
            .to_owned();
        let mut text = String::new();
        if let Some(t) = &item.title {
            text.push_str(&format!("# {t}\n\n"));
        }
        self.blocks(&id, 0, &mut text).await?;
        Ok(Fetched {
            name: "page.md".into(),
            content_type: Some("text/markdown".into()),
            bytes: text.into_bytes(),
        })
    }
}
