//! A [`VectorIndex`] in [Qdrant](https://qdrant.tech), over its REST API.
//! Each gai collection is a Qdrant collection (`<prefix><name>`), created
//! with cosine distance the first time vectors arrive; point ids are chunk
//! ids.

use async_trait::async_trait;
use gai_store::{Result, StoreError, VectorIndex};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

pub struct Qdrant {
    http: reqwest::Client,
    url: String,
    api_key: Option<String>,
    prefix: String,
}

fn err(e: impl std::fmt::Display) -> StoreError {
    StoreError::Database(format!("qdrant: {e}"))
}

impl Qdrant {
    pub fn new(url: &str, api_key: Option<String>, prefix: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            url: url.trim_end_matches('/').to_owned(),
            api_key,
            prefix: prefix.to_owned(),
        }
    }

    /// Qdrant collection names allow letters, digits, `-` and `_`.
    fn name(&self, collection: &str) -> String {
        let clean: String = collection
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        format!("{}{clean}", self.prefix)
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<(StatusCode, Value)> {
        let mut rb = self.http.request(method, format!("{}{path}", self.url));
        if let Some(k) = &self.api_key {
            rb = rb.header("api-key", k);
        }
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        let r = rb.send().await.map_err(err)?;
        let status = r.status();
        let v = r.json().await.unwrap_or(Value::Null);
        Ok((status, v))
    }

    async fn ok(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value> {
        let (status, v) = self.call(method, path, body).await?;
        if !status.is_success() {
            return Err(err(format!(
                "{path}: {status} {}",
                v["status"]["error"].as_str().unwrap_or("")
            )));
        }
        Ok(v)
    }

    /// Create the collection for `dims`-sized vectors if it doesn't exist.
    async fn ensure(&self, name: &str, dims: usize) -> Result<()> {
        let (status, v) = self
            .call(Method::GET, &format!("/collections/{name}"), None)
            .await?;
        if status.is_success() {
            let size = v["result"]["config"]["params"]["vectors"]["size"].as_u64();
            if size.is_some_and(|s| s as usize != dims) {
                return Err(err(format!(
                    "collection {name} holds {}-dimension vectors, not {dims}; reindex after changing the embedding model",
                    size.unwrap_or(0)
                )));
            }
            return Ok(());
        }
        self.ok(
            Method::PUT,
            &format!("/collections/{name}"),
            Some(json!({"vectors": {"size": dims, "distance": "Cosine"}})),
        )
        .await?;
        Ok(())
    }
}

#[async_trait]
impl VectorIndex for Qdrant {
    fn backend(&self) -> &'static str {
        "qdrant"
    }

    async fn upsert(&self, collection: &str, points: &[(i64, Vec<f32>)]) -> Result<()> {
        let Some(dims) = points.iter().map(|(_, v)| v.len()).find(|d| *d > 0) else {
            return Ok(());
        };
        let name = self.name(collection);
        self.ensure(&name, dims).await?;
        for batch in points.chunks(256) {
            let pts: Vec<Value> = batch
                .iter()
                .filter(|(_, v)| v.len() == dims)
                .map(
                    |(id, v)| json!({"id": id, "vector": v, "payload": {"collection": collection}}),
                )
                .collect();
            self.ok(
                Method::PUT,
                &format!("/collections/{name}/points?wait=true"),
                Some(json!({"points": pts})),
            )
            .await?;
        }
        Ok(())
    }

    async fn remove(&self, collection: &str, ids: &[i64]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let name = self.name(collection);
        let (status, v) = self
            .call(
                Method::POST,
                &format!("/collections/{name}/points/delete?wait=true"),
                Some(json!({"points": ids})),
            )
            .await?;
        // A collection that never got vectors has nothing to remove.
        if !status.is_success() && status != StatusCode::NOT_FOUND {
            return Err(err(format!("delete: {status} {v}")));
        }
        Ok(())
    }

    async fn drop_collection(&self, collection: &str) -> Result<()> {
        let name = self.name(collection);
        let (status, v) = self
            .call(Method::DELETE, &format!("/collections/{name}"), None)
            .await?;
        if !status.is_success() && status != StatusCode::NOT_FOUND {
            return Err(err(format!("drop: {status} {v}")));
        }
        Ok(())
    }

    async fn search(
        &self,
        collections: &[String],
        query: &[f32],
        k: usize,
    ) -> Result<Vec<(i64, f32)>> {
        let mut out = Vec::new();
        for c in collections {
            let name = self.name(c);
            let (status, v) = self
                .call(
                    Method::POST,
                    &format!("/collections/{name}/points/search"),
                    Some(json!({"vector": query, "limit": k, "with_payload": false})),
                )
                .await?;
            if status == StatusCode::NOT_FOUND {
                continue;
            }
            if !status.is_success() {
                return Err(err(format!("search: {status} {v}")));
            }
            for hit in v["result"].as_array().into_iter().flatten() {
                if let (Some(id), Some(score)) = (hit["id"].as_i64(), hit["score"].as_f64()) {
                    out.push((id, score as f32));
                }
            }
        }
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        out.truncate(k);
        Ok(out)
    }
}
