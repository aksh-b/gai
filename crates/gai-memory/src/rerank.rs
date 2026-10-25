//! Reranking: after hybrid search picks candidates, a model that reads the
//! query and each passage together orders them again.
//!
//! ```toml
//! [knowledge.rerank]
//! backend = "http"                          # http | llm
//! url = "https://api.cohere.com/v2/rerank"  # Cohere, Jina, Voyage, or TEI's /rerank
//! model = "rerank-v3.5"
//! api_key_env = "COHERE_API_KEY"
//! candidates = 30
//! ```
//!
//! `llm` asks a chat model (`model = "anthropic/claude-haiku-5-5"`) to grade
//! the passages instead.

use std::sync::Arc;

use gai_llm::{ChatRequest, ModelRegistry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{MemoryError, Result};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RerankBackend {
    /// No reranking.
    #[default]
    Off,
    /// A rerank API (Cohere, Jina, Voyage, Hugging Face TEI).
    Http,
    /// A chat model grading each passage.
    Llm,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct RerankSettings {
    pub backend: RerankBackend,
    /// The rerank model (`http`), or a chat model address (`llm`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The rerank endpoint (`http`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Environment variable holding the API key (`http`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// How many of the fused results are reranked.
    pub candidates: usize,
}

impl Default for RerankSettings {
    fn default() -> Self {
        Self {
            backend: RerankBackend::Off,
            model: None,
            url: None,
            api_key_env: None,
            candidates: 30,
        }
    }
}

/// Scores, 0 to 1, for `passages` against `query`, in the passages' order.
pub async fn rerank(
    s: &RerankSettings,
    models: &Arc<ModelRegistry>,
    http: &reqwest::Client,
    query: &str,
    passages: &[String],
) -> Result<Vec<f32>> {
    if passages.is_empty() {
        return Ok(Vec::new());
    }
    match s.backend {
        RerankBackend::Off => Ok(vec![0.0; passages.len()]),
        RerankBackend::Http => http_rerank(s, http, query, passages).await,
        RerankBackend::Llm => llm_rerank(s, models, query, passages).await,
    }
}

async fn http_rerank(
    s: &RerankSettings,
    http: &reqwest::Client,
    query: &str,
    passages: &[String],
) -> Result<Vec<f32>> {
    let url = s
        .url
        .as_deref()
        .ok_or_else(|| MemoryError::Invalid("knowledge.rerank needs a url".into()))?;
    // Cohere, Jina and Voyage take `documents`; TEI takes `texts`.
    let mut body = json!({"query": query, "documents": passages, "texts": passages});
    if let Some(m) = &s.model {
        body["model"] = json!(m);
    }
    let mut rb = http.post(url).json(&body);
    if let Some(key) = s.api_key_env.as_deref().and_then(|v| std::env::var(v).ok()) {
        rb = rb.bearer_auth(key);
    }
    let r = rb
        .send()
        .await
        .map_err(|e| MemoryError::Invalid(format!("rerank: {e}")))?;
    let status = r.status();
    let v: Value = r.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(MemoryError::Invalid(format!("rerank: {status} {v}")));
    }
    let items = v["results"]
        .as_array()
        .or_else(|| v["data"].as_array())
        .or_else(|| v.as_array())
        .ok_or_else(|| MemoryError::Invalid(format!("rerank: unexpected reply {v}")))?;
    let mut scores = vec![0.0f32; passages.len()];
    for it in items {
        let i = it["index"].as_u64().unwrap_or(u64::MAX) as usize;
        let score = it["relevance_score"]
            .as_f64()
            .or_else(|| it["score"].as_f64())
            .unwrap_or(0.0);
        if let Some(slot) = scores.get_mut(i) {
            *slot = score as f32;
        }
    }
    Ok(scores)
}

async fn llm_rerank(
    s: &RerankSettings,
    models: &Arc<ModelRegistry>,
    query: &str,
    passages: &[String],
) -> Result<Vec<f32>> {
    let model = s
        .model
        .as_deref()
        .ok_or_else(|| MemoryError::Invalid("knowledge.rerank (llm) needs a model".into()))?;
    let m = models.chat(model)?;
    let mut prompt = format!("Search query: {query}\n\nPassages:\n");
    for (i, p) in passages.iter().enumerate() {
        let p: String = p.chars().take(1200).collect();
        prompt.push_str(&format!("[{}] {}\n\n", i + 1, p.replace('\n', " ")));
    }
    let mut req = ChatRequest::user(prompt).with_system(
        "You rank search results. For each passage, give how well it answers the query, from 0 (unrelated) to 10 (answers it directly). Reply with only JSON: {\"scores\": [<one number per passage, in order>]}.",
    );
    req.temperature = Some(0.0);
    req.max_tokens = Some(400);
    let text = m.complete(req).await?.text();
    let start = text.find('{').unwrap_or(0);
    let end = text.rfind('}').map(|e| e + 1).unwrap_or(text.len());
    let v: Value = serde_json::from_str(&text[start..end]).map_err(|_| {
        MemoryError::Invalid(format!("the rerank model did not answer in JSON: {text}"))
    })?;
    let scores: Vec<f32> = v["scores"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|x| (x.as_f64().unwrap_or(0.0) / 10.0) as f32)
                .collect()
        })
        .unwrap_or_default();
    if scores.len() != passages.len() {
        return Err(MemoryError::Invalid(format!(
            "the rerank model scored {} of {} passages",
            scores.len(),
            passages.len()
        )));
    }
    Ok(scores)
}
