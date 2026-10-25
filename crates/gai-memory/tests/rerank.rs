//! Reranking after hybrid search: a rerank API (Cohere's shape, mocked)
//! and a chat model (scripted).

use std::sync::Arc;

use async_trait::async_trait;
use gai_llm::{
    LlmConfig, LlmError, MockModel, ModelInfo, ModelRegistry, Provider, SharedChatModel,
    text_response,
};
use gai_memory::rerank::{RerankBackend, RerankSettings};
use gai_memory::{DocumentInput, Knowledge, KnowledgeSettings, NewCollection, SearchMode};
use gai_store::{SharedStore, SqliteStore};
use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Scripted(Arc<MockModel>);

#[async_trait]
impl Provider for Scripted {
    fn name(&self) -> &str {
        "script"
    }
    fn chat(&self, _info: ModelInfo) -> Result<SharedChatModel, LlmError> {
        Ok(self.0.clone())
    }
}

async fn knowledge(rerank: RerankSettings, script: Option<Arc<MockModel>>) -> Knowledge {
    let store: SharedStore = Arc::new(SqliteStore::temporary().await.unwrap());
    let mut b = ModelRegistry::builder().config(LlmConfig::default());
    if let Some(m) = script {
        b = b.provider("script", Arc::new(Scripted(m)));
    }
    let k = Knowledge::new(
        store,
        Arc::new(b.build().unwrap()),
        KnowledgeSettings {
            rerank,
            ..Default::default()
        },
    );
    k.create_collection(NewCollection {
        name: "kb".into(),
        ..Default::default()
    })
    .await
    .unwrap();
    for (source, text) in [
        ("a.md", "Leave: twenty five days of leave per year."),
        ("b.md", "Leave requests go to your manager for approval."),
        ("c.md", "The office cafeteria opens at eight."),
    ] {
        k.ingest(
            "kb",
            DocumentInput {
                source: source.into(),
                title: None,
                text: text.into(),
                metadata: json!({}),
            },
        )
        .await
        .unwrap();
    }
    k
}

#[tokio::test]
async fn a_rerank_api_reorders_results() {
    let server = MockServer::start().await;
    // Puts whichever passage mentions the manager first.
    Mock::given(method("POST"))
        .and(path("/v2/rerank"))
        .and(body_partial_json(
            json!({"model": "rerank-v3.5", "query": "who approves leave"}),
        ))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            let results: Vec<serde_json::Value> = body["documents"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(i, d)| {
                    let s = if d.as_str().unwrap().contains("manager") {
                        0.97
                    } else {
                        0.1
                    };
                    json!({"index": i, "relevance_score": s})
                })
                .collect();
            ResponseTemplate::new(200).set_body_json(json!({"results": results}))
        })
        .mount(&server)
        .await;
    let k = knowledge(
        RerankSettings {
            backend: RerankBackend::Http,
            url: Some(format!("{}/v2/rerank", server.uri())),
            model: Some("rerank-v3.5".into()),
            ..Default::default()
        },
        None,
    )
    .await;
    let hits = k
        .search(&["kb".into()], "who approves leave", 3, SearchMode::Hybrid)
        .await
        .unwrap();
    assert_eq!(hits[0].source, "b.md", "{hits:#?}");
    assert_eq!(hits[0].rerank_score, Some(0.97));
    assert!(hits.iter().all(|h| h.rerank_score.is_some()));
}

#[tokio::test]
async fn a_chat_model_can_rerank_and_failures_keep_the_fused_order() {
    // Scores the passages in the order search sends them; it doesn't know
    // which is which, so give the last one the highest grade.
    let script = Arc::new(MockModel::scripted(vec![
        text_response(r#"{"scores": [1, 2, 9]}"#),
        text_response("I would rather not."),
    ]));
    let k = knowledge(
        RerankSettings {
            backend: RerankBackend::Llm,
            model: Some("script/grader".into()),
            ..Default::default()
        },
        Some(script),
    )
    .await;
    let fused = k
        .search(&["kb".into()], "leave days", 3, SearchMode::Hybrid)
        .await;
    let hits = fused.unwrap();
    assert!(hits[0].rerank_score.unwrap() >= 0.9, "{hits:#?}");

    // The model's reply isn't JSON: the fused order stands.
    let hits = k
        .search(&["kb".into()], "leave days", 3, SearchMode::Hybrid)
        .await
        .unwrap();
    assert!(hits.iter().all(|h| h.rerank_score.is_none()), "{hits:#?}");
}
