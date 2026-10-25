//! Knowledge with a separate vector index: Qdrant when `GAI_TEST_QDRANT_URL`
//! is set, pgvector when `GAI_TEST_PGVECTOR_URL` is (`scripts/vector-test.sh`
//! starts both). Embeddings are `mock/hash`, so no model is needed.

use std::sync::Arc;

use gai_llm::{LlmConfig, ModelRegistry};
use gai_memory::{
    DocumentInput, Knowledge, KnowledgeSettings, NewCollection, SearchMode, VectorBackend,
    VectorIndexSettings,
};
use gai_store::{SharedStore, SharedVectorIndex, SqliteStore};
use serde_json::json;

async fn knowledge(settings: KnowledgeSettings, index: Option<SharedVectorIndex>) -> Knowledge {
    let store: SharedStore = Arc::new(SqliteStore::temporary().await.unwrap());
    let models = Arc::new(
        ModelRegistry::builder()
            .config(LlmConfig::default())
            .build()
            .unwrap(),
    );
    let k = Knowledge::new(store, models, settings);
    if index.is_some() {
        k.set_index(index);
    }
    k
}

fn doc(source: &str, text: &str) -> DocumentInput {
    DocumentInput {
        source: source.into(),
        title: None,
        text: text.into(),
        metadata: json!({}),
    }
}

/// The same checks for any index.
async fn exercise(k: Knowledge, backend: &str) {
    assert_eq!(k.index_backend(), backend);
    let name = format!("t{}", gai_core::new_id("c").to_lowercase().replace('_', ""));
    k.create_collection(NewCollection {
        name: name.clone(),
        ..Default::default()
    })
    .await
    .unwrap();
    let a = k
        .ingest(
            &name,
            doc(
                "leave.md",
                "Everyone gets twenty five days of annual leave each year.",
            ),
        )
        .await
        .unwrap();
    k.ingest(
        &name,
        doc(
            "pay.md",
            "Salaries are paid on the last working day of the month.",
        ),
    )
    .await
    .unwrap();
    let names = [name.clone()];
    let hits = k
        .search(&names, "annual leave days", 3, SearchMode::Vector)
        .await
        .unwrap();
    assert_eq!(hits[0].source, "leave.md", "{hits:?}");
    assert!(hits[0].vector_score.unwrap() > 0.0);

    // Replacing a document swaps its vectors.
    k.ingest(
        &name,
        doc(
            "leave.md",
            "Holidays: thirty days of vacation for everyone.",
        ),
    )
    .await
    .unwrap();
    let hits = k
        .search(&names, "vacation holidays", 3, SearchMode::Vector)
        .await
        .unwrap();
    assert_eq!(hits[0].source, "leave.md");
    assert!(hits[0].text.contains("thirty"), "{hits:?}");
    assert!(
        hits.iter().all(|h| !h.text.contains("twenty five")),
        "old vectors gone: {hits:?}"
    );

    // Deleting a document, reindexing, dropping the collection.
    let pay = k
        .store()
        .find_document(&name, "pay.md")
        .await
        .unwrap()
        .unwrap();
    assert!(k.delete_document(&pay.id).await.unwrap());
    let hits = k
        .search(&names, "salaries paid month", 5, SearchMode::Vector)
        .await
        .unwrap();
    assert!(hits.iter().all(|h| h.source != "pay.md"), "{hits:?}");
    assert_eq!(k.reindex(&name).await.unwrap(), 1);
    let hits = k
        .search(&names, "vacation", 3, SearchMode::Hybrid)
        .await
        .unwrap();
    assert_eq!(hits[0].source, "leave.md");
    let _ = a;
    assert!(k.delete_collection(&name).await.unwrap());
}

#[tokio::test]
async fn qdrant_index() {
    let Ok(url) = std::env::var("GAI_TEST_QDRANT_URL") else {
        eprintln!("skipped: set GAI_TEST_QDRANT_URL");
        return;
    };
    let settings = KnowledgeSettings {
        vector_index: VectorIndexSettings {
            backend: VectorBackend::Qdrant,
            url: Some(url),
            prefix: Some("gaitest_".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    exercise(knowledge(settings, None).await, "qdrant").await;
}

#[tokio::test]
async fn pgvector_index() {
    let Ok(url) = std::env::var("GAI_TEST_PGVECTOR_URL") else {
        eprintln!("skipped: set GAI_TEST_PGVECTOR_URL");
        return;
    };
    let index: SharedVectorIndex = Arc::new(gai_store::PgVectorIndex::connect(&url).await.unwrap());
    exercise(
        knowledge(KnowledgeSettings::default(), Some(index)).await,
        "pgvector",
    )
    .await;
}

#[tokio::test]
async fn builtin_by_default_and_a_dead_index_falls_back() {
    let k = knowledge(KnowledgeSettings::default(), None).await;
    assert_eq!(k.index_backend(), "builtin");
    // Qdrant on a port nobody listens on: ingesting fails loudly, but
    // searching falls back to the store.
    let dead = knowledge(
        KnowledgeSettings {
            vector_index: VectorIndexSettings {
                backend: VectorBackend::Qdrant,
                url: Some("http://127.0.0.1:9".into()),
                ..Default::default()
            },
            ..Default::default()
        },
        None,
    )
    .await;
    dead.create_collection(NewCollection {
        name: "kb".into(),
        ..Default::default()
    })
    .await
    .unwrap();
    assert!(dead.ingest("kb", doc("a.md", "some text")).await.is_err());
    let hits = dead
        .search(&["kb".into()], "text", 3, SearchMode::Vector)
        .await
        .unwrap();
    assert!(hits.iter().all(|h| h.source == "a.md"));
}
