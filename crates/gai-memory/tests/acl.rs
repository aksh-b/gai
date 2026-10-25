//! Documents with an `acl` are found only by who it names, in searches and
//! in `rag_search` inside runs (and their sub-runs).

use std::sync::Arc;

use gai_llm::{LlmConfig, ModelRegistry};
use gai_memory::{Access, DocumentInput, Knowledge, KnowledgeSettings, NewCollection, SearchMode};
use gai_store::{NewRun, NewSession, SharedStore, SqliteStore};
use gai_tools::ToolCtx;
use serde_json::json;

async fn knowledge() -> Arc<Knowledge> {
    let store: SharedStore = Arc::new(SqliteStore::temporary().await.unwrap());
    let models = Arc::new(
        ModelRegistry::builder()
            .config(LlmConfig::default())
            .build()
            .unwrap(),
    );
    let k = Arc::new(Knowledge::new(store, models, KnowledgeSettings::default()));
    k.create_collection(NewCollection {
        name: "kb".into(),
        ..Default::default()
    })
    .await
    .unwrap();
    for (source, text, acl) in [
        (
            "secret.md",
            "The vault combination is 31-07-99.",
            json!(["team:security", "role:admin"]),
        ),
        ("open.md", "The vault is in the basement.", json!([])),
    ] {
        k.ingest(
            "kb",
            DocumentInput {
                source: source.into(),
                text: text.into(),
                metadata: json!({"acl": acl}),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    k
}

fn sources(hits: &[gai_memory::SearchHit]) -> Vec<&str> {
    hits.iter().map(|h| h.source.as_str()).collect()
}

#[tokio::test]
async fn searches_leave_out_what_the_searcher_may_not_find() {
    let k = knowledge().await;
    let all = k
        .search(&["kb".into()], "vault", 10, SearchMode::Text)
        .await
        .unwrap();
    assert_eq!(all.len(), 2);
    let member = Access {
        identities: vec![
            "user:mo@example.com".into(),
            "role:viewer".into(),
            "role:member".into(),
        ],
    };
    let hits = k
        .search_as(&["kb".into()], "vault", 10, SearchMode::Text, Some(&member))
        .await
        .unwrap();
    assert_eq!(sources(&hits), ["open.md"]);
    let security = Access {
        identities: vec!["team:security".into()],
    };
    let hits = k
        .search_as(
            &["kb".into()],
            "vault",
            10,
            SearchMode::Hybrid,
            Some(&security),
        )
        .await
        .unwrap();
    assert_eq!(hits.len(), 2);
    assert!(Access::default().allows(&json!(null)));
    assert!(!Access::default().allows(&json!(["role:admin"])));
    assert!(gai_memory::validate_acl(&["team:x".into(), "role:builder".into()]).is_ok());
    assert!(gai_memory::validate_acl(&["role:boss".into()]).is_err());
}

#[tokio::test]
async fn rag_search_in_a_run_uses_who_started_it() {
    let k = knowledge().await;
    let store = k.store().clone();
    let ses = store.create_session(NewSession::default()).await.unwrap();
    let parent = store
        .create_run(NewRun {
            session_id: ses.id.clone(),
            agent: "helper".into(),
            input: json!("hi"),
            metadata: json!({"access": {"identities": ["role:member"]}}),
            ..Default::default()
        })
        .await
        .unwrap();
    // A delegate's run inherits its parent's access.
    let child = store
        .create_run(NewRun {
            session_id: ses.id.clone(),
            agent: "researcher".into(),
            parent_run_id: Some(parent.id.clone()),
            input: json!("hi"),
            ..Default::default()
        })
        .await
        .unwrap();
    let unrestricted = store
        .create_run(NewRun {
            session_id: ses.id.clone(),
            agent: "helper".into(),
            input: json!("hi"),
            ..Default::default()
        })
        .await
        .unwrap();
    let tool = gai_memory::tools::rag_tool(k.clone(), vec![]);
    let call = |run: String| {
        let tool = tool.clone();
        async move {
            let mut ctx = ToolCtx::for_workspace(std::env::temp_dir());
            ctx.run_id = run;
            let out = tool
                .call(&ctx, json!({"query": "vault combination"}))
                .await
                .unwrap();
            serde_json::to_string(&out.content).unwrap()
        }
    };
    let text = call(child.id).await;
    assert!(!text.contains("31-07-99"), "{text}");
    assert!(text.contains("basement"), "{text}");
    let text = call(unrestricted.id).await;
    assert!(text.contains("31-07-99"), "{text}");
}
