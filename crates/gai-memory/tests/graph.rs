//! A knowledge graph built by a (scripted) model, and graph search finding
//! a passage through what the question's entities are connected to.

use std::sync::Arc;

use async_trait::async_trait;
use gai_llm::{
    LlmConfig, LlmError, MockModel, ModelInfo, ModelRegistry, Provider, SharedChatModel,
    text_response,
};
use gai_memory::{DocumentInput, Knowledge, KnowledgeSettings, NewCollection, SearchMode};
use gai_store::{SharedStore, SqliteStore};
use serde_json::json;

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

async fn knowledge(script: Arc<MockModel>) -> Arc<Knowledge> {
    let store: SharedStore = Arc::new(SqliteStore::temporary().await.unwrap());
    let models = ModelRegistry::builder()
        .config(LlmConfig::default())
        .provider("script", Arc::new(Scripted(script)))
        .build()
        .unwrap();
    let k = Arc::new(Knowledge::new(
        store,
        Arc::new(models),
        KnowledgeSettings::default(),
    ));
    k.create_collection(NewCollection {
        name: "kb".into(),
        metadata: json!({"graph": true}),
        ..Default::default()
    })
    .await
    .unwrap();
    for (source, text) in [
        (
            "a.md",
            "Globex Inc. supplies widgets to the Acme Corporation every quarter.",
        ),
        (
            "b.md",
            "Ada Lovelace is the chief executive of Acme Corporation since 2021.",
        ),
        (
            "c.md",
            "The office cafeteria serves lunch at noon on weekdays.",
        ),
    ] {
        k.ingest(
            "kb",
            DocumentInput {
                source: source.into(),
                text: text.into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    k
}

const FOUND: &str = r#"{"entities": [
  {"name": "Globex Inc.", "type": "organization", "description": "A widget supplier"},
  {"name": "Acme Corporation", "type": "organization"},
  {"name": "Ada Lovelace", "type": "person", "description": "Chief executive of Acme"},
  {"name": "widgets", "type": "nonsense"}
], "relations": [
  {"source": "Globex Inc.", "relation": "supplies", "target": "Acme Corporation"},
  {"source": "Ada Lovelace", "relation": "leads", "target": "Acme Corporation"}
]}"#;

#[tokio::test]
async fn builds_a_graph_and_searches_through_it() {
    let script = Arc::new(MockModel::scripted(vec![text_response(FOUND)]));
    let k = knowledge(script.clone()).await;
    assert_eq!(
        k.graph_model_for("kb").await,
        None,
        "no [knowledge.graph] model set"
    );

    let built = k.build_graph("kb", "script/m").await.unwrap();
    assert_eq!(
        (built.chunks, built.entities, built.relations, built.failed),
        (3, 4, 2, 0)
    );
    assert_eq!(
        script.requests().lock().unwrap().len(),
        1,
        "three short chunks, one call"
    );
    // Nothing left to read.
    assert_eq!(k.build_graph("kb", "script/m").await.unwrap().chunks, 0);

    let stats = k.store().graph_stats("kb").await.unwrap();
    assert_eq!((stats.nodes, stats.edges, stats.chunks_pending), (4, 2, 0));
    let nodes = k.store().graph_nodes("kb", "", 10).await.unwrap();
    let acme = nodes.iter().find(|n| n.name == "Acme Corporation").unwrap();
    assert_eq!(acme.mentions, 2);
    assert_eq!(
        nodes.iter().find(|n| n.name == "widgets").unwrap().kind,
        "other"
    );

    let g = k
        .search_graph(
            &["kb".into()],
            "Who leads the company Globex supplies?",
            2,
            None,
        )
        .await
        .unwrap();
    let sources: Vec<&str> = g.hits.iter().map(|h| h.source.as_str()).collect();
    assert!(sources.contains(&"b.md"), "{sources:?}");
    assert!(!sources.contains(&"c.md"), "{sources:?}");
    assert!(
        g.facts
            .iter()
            .any(|f| f == "Ada Lovelace —leads→ Acme Corporation"),
        "{:?}",
        g.facts
    );
    assert!(g.entities.iter().any(|e| e.name == "Globex Inc."));
    // The same through the search mode, and rag_search adds the facts.
    let hits = k
        .search(
            &["kb".into()],
            "Who leads the company Globex supplies?",
            2,
            SearchMode::Graph,
        )
        .await
        .unwrap();
    assert_eq!(hits.len(), 2);
    let tool = gai_memory::tools::rag_tool(k.clone(), vec![]);
    let out = tool
        .call(
            &gai_tools::ToolCtx::for_workspace(std::env::temp_dir()),
            json!({"query": "Who supplies Acme?"}),
        )
        .await
        .unwrap();
    let text = serde_json::to_string(&out.content).unwrap();
    assert!(
        text.contains("Connections in the knowledge graph"),
        "{text}"
    );

    // Re-ingesting a document drops what was learned from it, and the
    // next build reads it again.
    k.ingest(
        "kb",
        DocumentInput {
            source: "b.md".into(),
            text: "Grace Hopper now runs Acme Corporation.".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(k.store().graph_stats("kb").await.unwrap().chunks_pending, 1);
    assert!(
        k.store()
            .graph_nodes("kb", "lovelace", 5)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_bad_answer_skips_the_batch() {
    let script = Arc::new(MockModel::scripted(vec![text_response("I can't do that.")]));
    let k = knowledge(script).await;
    let built = k.build_graph("kb", "script/m").await.unwrap();
    assert_eq!((built.failed, built.entities), (1, 0));
    assert_eq!(k.store().graph_stats("kb").await.unwrap().chunks_pending, 0);
}
