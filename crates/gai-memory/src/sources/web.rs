//! Web pages: start URLs, a sitemap, and same-site links up to a depth.

use std::collections::{HashSet, VecDeque};
use std::sync::OnceLock;

use async_trait::async_trait;
use regex::Regex;
use sha2::{Digest, Sha256};
use url::Url;

use super::{Connector, Fetched, Item, invalid};
use crate::Result;

pub struct Web {
    http: reqwest::Client,
    urls: Vec<String>,
    sitemap: Option<String>,
    depth: u32,
    max_pages: usize,
    allow_private: bool,
}

impl Web {
    pub fn new(
        http: reqwest::Client,
        urls: Vec<String>,
        sitemap: Option<String>,
        depth: u32,
        max_pages: usize,
        allow_private: bool,
    ) -> Self {
        Self {
            http,
            urls,
            sitemap,
            depth,
            max_pages,
            allow_private,
        }
    }

    async fn allowed(&self, u: &Url) -> bool {
        if !matches!(u.scheme(), "http" | "https") {
            return false;
        }
        if self.allow_private {
            return true;
        }
        let (host, port) = (
            u.host_str().unwrap_or_default().to_owned(),
            u.port_or_known_default().unwrap_or(80),
        );
        !tokio::task::spawn_blocking(move || gai_tools::builtin::is_private_host(&host, port))
            .await
            .unwrap_or(true)
    }

    async fn get(&self, u: &Url) -> Result<(Option<String>, Vec<u8>)> {
        let r = self.http.get(u.clone()).send().await.map_err(invalid)?;
        if !r.status().is_success() {
            return Err(invalid(format!("{u}: {}", r.status())));
        }
        let ct = r
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        Ok((ct, r.bytes().await.map_err(invalid)?.to_vec()))
    }
}

fn href() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?i)<a\s[^>]*href\s*=\s*["']([^"'#]+)"#).expect("valid"))
}

fn loc() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)<loc>\s*([^<\s]+)\s*</loc>").expect("valid"))
}

fn title_of(html: &str) -> Option<String> {
    static R: OnceLock<Regex> = OnceLock::new();
    let r = R.get_or_init(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").expect("valid"));
    r.captures(html)
        .map(|c| c[1].split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|t| !t.is_empty())
}

#[async_trait]
impl Connector for Web {
    async fn list(&self) -> Result<Vec<Item>> {
        let mut queue: VecDeque<(Url, u32)> = VecDeque::new();
        for u in &self.urls {
            queue.push_back((Url::parse(u).map_err(|e| invalid(format!("{u}: {e}")))?, 0));
        }
        if let Some(s) = &self.sitemap {
            let su = Url::parse(s).map_err(|e| invalid(format!("{s}: {e}")))?;
            if !self.allowed(&su).await {
                return Err(invalid(format!(
                    "{su} is a private address; set allow_private"
                )));
            }
            let (_, body) = self.get(&su).await?;
            for c in loc().captures_iter(&String::from_utf8_lossy(&body)) {
                if let Ok(u) = Url::parse(&c[1]) {
                    queue.push_back((u, self.depth));
                }
            }
        }
        if queue.is_empty() {
            return Err(invalid("a web source needs urls or a sitemap"));
        }
        let hosts: HashSet<String> = queue
            .iter()
            .filter_map(|(u, _)| u.host_str().map(str::to_owned))
            .collect();
        let mut seen = HashSet::new();
        let mut items = Vec::new();
        while let Some((mut u, d)) = queue.pop_front() {
            u.set_fragment(None);
            if items.len() >= self.max_pages || !seen.insert(u.to_string()) {
                continue;
            }
            if !self.allowed(&u).await {
                continue;
            }
            let (ct, bytes) = match self.get(&u).await {
                Ok(x) => x,
                // A broken link doesn't stop the crawl.
                Err(e) => {
                    tracing::debug!(url = %u, error = %e, "web source: skipped");
                    continue;
                }
            };
            let text = String::from_utf8_lossy(&bytes).into_owned();
            // Servers mislabel pages; the start of the body tells.
            let head = text
                .trim_start()
                .get(..15)
                .unwrap_or_default()
                .to_ascii_lowercase();
            let is_html = ct.as_deref().is_none_or(|c| c.contains("html"))
                || head.starts_with("<!doctype html")
                || head.starts_with("<html");
            if is_html && d < self.depth {
                for c in href().captures_iter(&text) {
                    if let Ok(next) = u.join(&c[1])
                        && next.host_str().is_some_and(|h| hosts.contains(h))
                    {
                        queue.push_back((next, d + 1));
                    }
                }
            }
            let name = if is_html {
                "page.html".to_owned()
            } else {
                u.path_segments()
                    .and_then(|mut s| s.next_back())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("page")
                    .to_owned()
            };
            items.push(Item {
                key: u.to_string(),
                version: format!("{:x}", Sha256::digest(&bytes)),
                title: if is_html { title_of(&text) } else { None },
                url: Some(u.to_string()),
                acl: Vec::new(),
                content: Some(Fetched {
                    name,
                    content_type: ct,
                    bytes,
                }),
            });
        }
        Ok(items)
    }

    async fn fetch(&self, item: &Item) -> Result<Fetched> {
        let u = Url::parse(&item.key).map_err(invalid)?;
        let (ct, bytes) = self.get(&u).await?;
        Ok(Fetched {
            name: "page.html".into(),
            content_type: ct,
            bytes,
        })
    }
}
