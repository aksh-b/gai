//! Memory and knowledge for gai agents.
//!
//! - **Memory blocks** ([`RunMemory`]): named, size-limited text an agent
//!   always sees in its prompt and edits with `memory_append`,
//!   `memory_replace` and `memory_rethink`. Blocks belong to an agent, to a
//!   user (a profile every agent shares) or to a named group of agents, and
//!   every edit is versioned.
//! - **Archival memory**: facts an agent saves with `remember` and finds
//!   with `memory_search`, kept in a collection of its own.
//! - **Knowledge** ([`Knowledge`]): collections of documents that are
//!   loaded (text, Markdown, HTML, PDF), chunked, embedded and searched by
//!   meaning and keywords together, through `rag_search`.

pub mod blocks;
pub mod chunk;
pub mod knowledge;
pub mod load;
pub mod tools;

pub use blocks::{ArchivalScope, BlockSpec, MemoryConfig, RunMemory, archival_collection};
pub use knowledge::{
    DocumentInput, FALLBACK_EMBEDDING_MODEL, Ingested, Knowledge, KnowledgeSettings, NewCollection,
    SearchHit, SearchMode, format_hits,
};

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error(transparent)]
    Store(#[from] gai_store::StoreError),
    #[error(transparent)]
    Llm(#[from] gai_llm::LlmError),
    #[error("{0}")]
    Invalid(String),
    #[error("not found: {0}")]
    NotFound(String),
}

pub type Result<T, E = MemoryError> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gai_llm::ModelRegistry;
    use gai_store::{SharedStore, SqliteStore};
    use gai_tools::ToolCtx;
    use serde_json::json;

    use super::*;

    async fn knowledge() -> Arc<Knowledge> {
        let store: SharedStore = Arc::new(SqliteStore::temporary().await.unwrap());
        let models = Arc::new(ModelRegistry::from_config(Default::default()).unwrap());
        Arc::new(Knowledge::new(store, models, KnowledgeSettings::default()))
    }

    fn doc(source: &str, text: &str) -> DocumentInput {
        DocumentInput {
            source: source.into(),
            title: None,
            text: text.into(),
            metadata: json!({}),
        }
    }

    #[tokio::test]
    async fn ingest_dedupes_replaces_and_searches() {
        let k = knowledge().await;
        let c = k
            .create_collection(NewCollection {
                name: "handbook".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(c.embedding_model, "mock/hash");
        assert!(
            k.create_collection(NewCollection {
                name: "bad name".into(),
                ..Default::default()
            })
            .await
            .is_err()
        );
        let a = k
            .ingest(
                "handbook",
                doc(
                    "leave.md",
                    "# Leave\nEmployees get 25 days of annual leave.\n\n## Sick leave\nSick leave needs a doctor's note after 3 days.",
                ),
            )
            .await
            .unwrap();
        assert!(!a.unchanged);
        assert_eq!(a.document.chunks, 2);
        k.ingest(
            "handbook",
            doc(
                "expenses.md",
                "Expenses over 500 euros need a manager's approval.",
            ),
        )
        .await
        .unwrap();
        let again = k
            .ingest(
                "handbook",
                doc(
                    "leave.md",
                    "# Leave\nEmployees get 25 days of annual leave.\n\n## Sick leave\nSick leave needs a doctor's note after 3 days.",
                ),
            )
            .await
            .unwrap();
        assert!(again.unchanged);
        assert_eq!(again.document.id, a.document.id);
        let c = k.collection("handbook").await.unwrap();
        assert_eq!((c.documents, c.chunks, c.dimensions), (2, 3, Some(256)));

        let names = vec!["handbook".to_owned()];
        for mode in [SearchMode::Hybrid, SearchMode::Vector, SearchMode::Text] {
            let hits = k.search(&names, "doctor note sick", 2, mode).await.unwrap();
            assert_eq!(
                hits[0].heading.as_deref(),
                Some("Leave > Sick leave"),
                "{mode:?}"
            );
            assert_eq!(hits[0].source, "leave.md");
        }
        let hits = k
            .search(&names, "manager approval expenses", 1, SearchMode::Hybrid)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source, "expenses.md");
        assert!(hits[0].vector_score.is_some() && hits[0].text_score.is_some());
        let text = format_hits(&hits);
        assert!(
            text.starts_with("[1] expenses.md\nsource: expenses.md"),
            "{text}"
        );
        assert!(
            k.search(&["nope".into()], "x", 3, SearchMode::Hybrid)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn ingests_a_directory_of_mixed_files() {
        let k = knowledge().await;
        k.create_collection(NewCollection {
            name: "docs".into(),
            ..Default::default()
        })
        .await
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.md"),
            "# Alpha\nThe alpha service runs on port 9000.",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("b.html"),
            "<title>Beta</title><p>Beta handles billing.</p>",
        )
        .unwrap();
        std::fs::write(dir.path().join("c.png"), [0u8, 1, 2]).unwrap();
        std::fs::create_dir(dir.path().join("node_modules")).unwrap();
        std::fs::write(dir.path().join("node_modules/x.md"), "ignored").unwrap();
        std::fs::write(dir.path().join("empty.txt"), "   ").unwrap();
        let results = k.ingest_path("docs", dir.path()).await.unwrap();
        assert_eq!(results.len(), 3, "a.md, b.html, empty.txt");
        let ok: Vec<_> = results.iter().filter(|(_, r)| r.is_ok()).collect();
        assert_eq!(ok.len(), 2);
        let hits = k
            .search(&["docs".into()], "billing", 1, SearchMode::Text)
            .await
            .unwrap();
        assert_eq!(hits[0].title.as_deref(), Some("Beta"));
    }

    #[tokio::test]
    async fn tools_remember_search_and_rag() {
        let k = knowledge().await;
        let ctx = ToolCtx::for_workspace(".");
        let tools = tools::archival_tools(k.clone(), "memory:agent:a".into());
        let (remember, search) = (&tools[0], &tools[1]);
        let empty = search.call(&ctx, json!({"query": "cats"})).await.unwrap();
        assert_eq!(empty.as_text(), "Archival memory is empty.");
        remember
            .call(&ctx, json!({"text": "The user's cat is called Miso."}))
            .await
            .unwrap();
        remember
            .call(&ctx, json!({"text": "Deploys happen on Tuesdays."}))
            .await
            .unwrap();
        let found = search
            .call(&ctx, json!({"query": "cat name", "k": 1}))
            .await
            .unwrap();
        assert!(found.as_text().contains("Miso"), "{}", found.as_text());

        // rag_search without a list sees knowledge collections, not memory.
        let rag = tools::rag_tool(k.clone(), vec![]);
        let none = rag.call(&ctx, json!({"query": "cat"})).await.unwrap();
        assert_eq!(none.as_text(), "There are no knowledge collections yet.");
        k.create_collection(NewCollection {
            name: "kb".into(),
            ..Default::default()
        })
        .await
        .unwrap();
        k.ingest("kb", doc("faq", "Refunds take five working days."))
            .await
            .unwrap();
        let out = rag.call(&ctx, json!({"query": "refunds"})).await.unwrap();
        assert!(out.as_text().contains("five working days"));
        let limited = tools::rag_tool(k, vec!["kb".into()]);
        assert!(
            limited
                .call(&ctx, json!({"query": "x", "collection": "memory:agent:a"}))
                .await
                .is_err()
        );
    }
}
