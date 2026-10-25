//! Behaviour every store backend must share. Each check takes a fresh,
//! empty store and panics on a difference; backends run them in their own
//! tests (SQLite always, Postgres when `GAI_TEST_POSTGRES_URL` is set).

use serde_json::json;

use crate::{AgentStore, McpStore, SettingsStore, StoreError};

/// Versions, dedup of identical saves, labels and deletes.
pub async fn agents(s: &dyn AgentStore) {
    let a = json!({"name": "a", "model": "mock/echo"});
    assert_eq!(s.put_agent("a", &a, Some("first")).await.unwrap(), 1);
    // Saving the same spec again is not a new version.
    assert_eq!(s.put_agent("a", &a, None).await.unwrap(), 1);
    let b = json!({"name": "a", "model": "mock/other"});
    assert_eq!(s.put_agent("a", &b, None).await.unwrap(), 2);
    assert_eq!(s.get_agent("a", None).await.unwrap().unwrap().spec, b);
    let v1 = s.get_agent("a", Some(1)).await.unwrap().unwrap();
    assert_eq!((v1.spec, v1.note.as_deref()), (a, Some("first")));
    assert!(s.get_agent("a", Some(9)).await.unwrap().is_none());
    assert!(s.get_agent("nope", None).await.unwrap().is_none());
    let versions: Vec<u32> = s
        .list_agent_versions("a")
        .await
        .unwrap()
        .iter()
        .map(|v| v.version)
        .collect();
    assert_eq!(versions, [2, 1]);

    s.set_agent_label("a", "prod", Some(1)).await.unwrap();
    assert!(matches!(
        s.set_agent_label("a", "prod", Some(7)).await,
        Err(StoreError::NotFound(_))
    ));
    assert!(matches!(
        s.set_agent_label("nope", "prod", Some(1)).await,
        Err(StoreError::NotFound(_))
    ));
    s.put_agent("b", &json!({"name": "b"}), None).await.unwrap();
    let list = s.list_agents().await.unwrap();
    assert_eq!(
        list.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(list[0].latest_version, 2);
    assert_eq!(list[0].labels.get("prod"), Some(&1));
    s.set_agent_label("a", "prod", None).await.unwrap();
    assert!(s.list_agents().await.unwrap()[0].labels.is_empty());

    assert!(s.delete_agent("a").await.unwrap());
    assert!(!s.delete_agent("a").await.unwrap());
    assert!(s.get_agent("a", Some(1)).await.unwrap().is_none());
}

/// Get, overwrite, list by prefix, delete.
pub async fn settings(s: &dyn SettingsStore) {
    assert_eq!(s.get_setting("default_model").await.unwrap(), None);
    s.put_setting("default_model", "mock/echo").await.unwrap();
    s.put_setting("default_model", "ollama/x").await.unwrap();
    s.put_setting("provider_key:openai", "sk-1").await.unwrap();
    s.put_setting("provider_key:anthropic", "sk-2")
        .await
        .unwrap();
    // A prefix is literal, not a pattern.
    s.put_setting("provider_keyX", "no").await.unwrap();
    assert_eq!(
        s.get_setting("default_model").await.unwrap().as_deref(),
        Some("ollama/x")
    );
    assert_eq!(
        s.list_settings("provider_key:").await.unwrap(),
        [
            ("provider_key:anthropic".to_owned(), "sk-2".to_owned()),
            ("provider_key:openai".to_owned(), "sk-1".to_owned()),
        ]
    );
    assert!(s.delete_setting("provider_key:openai").await.unwrap());
    assert!(!s.delete_setting("provider_key:openai").await.unwrap());
}

/// Server configs and OAuth tokens.
pub async fn mcp(s: &dyn McpStore) {
    s.put_mcp_server("b", &json!({"url": "http://b"}))
        .await
        .unwrap();
    s.put_mcp_server("a", &json!({"command": "x"}))
        .await
        .unwrap();
    s.put_mcp_server("a", &json!({"command": "y"}))
        .await
        .unwrap();
    let all = s.list_mcp_servers().await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0], ("a".to_owned(), json!({"command": "y"})));
    assert!(s.delete_mcp_server("b").await.unwrap());
    assert!(!s.delete_mcp_server("b").await.unwrap());

    assert_eq!(s.get_mcp_oauth("a").await.unwrap(), None);
    s.put_mcp_oauth("a", &json!({"token": 1})).await.unwrap();
    s.put_mcp_oauth("a", &json!({"token": 2})).await.unwrap();
    assert_eq!(
        s.get_mcp_oauth("a").await.unwrap(),
        Some(json!({"token": 2}))
    );
    s.delete_mcp_oauth("a").await.unwrap();
    assert_eq!(s.get_mcp_oauth("a").await.unwrap(), None);
}
