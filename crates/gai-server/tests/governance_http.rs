//! The audit log, retention and forgetting a user, over HTTP.

use std::sync::Arc;

use gai::{AuthMode, Config, Gai};
use gai_server::{AuthPolicy, required_scope, router};
use reqwest::Method;
use serde_json::{Value, json};

async fn server(mode: AuthMode) -> (String, Arc<Gai>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        data_dir: Some(dir.path().to_path_buf()),
        default_model: Some("mock/echo".into()),
        ..Default::default()
    };
    let gai = Arc::new(Gai::from_config(config, None).unwrap());
    let app = router(gai.clone(), AuthPolicy::new(mode, true, 0), None)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, gai, dir)
}

async fn call(method: Method, url: &str, key: Option<&str>, body: Option<Value>) -> (u16, String) {
    let mut rb = reqwest::Client::new().request(method, url);
    if let Some(k) = key {
        rb = rb.bearer_auth(k);
    }
    if let Some(b) = body {
        rb = rb.json(&b);
    }
    let r = rb.send().await.unwrap();
    (r.status().as_u16(), r.text().await.unwrap())
}

#[tokio::test]
async fn changes_are_audited_with_who_did_them() {
    let (u, gai, _d) = server(AuthMode::Required).await;
    // An admin key, made the way `gai keys create` does.
    let key = gai::store::keys::generate();
    gai.runtime()
        .await
        .unwrap()
        .store()
        .create_key(
            "ada",
            &key.prefix,
            &key.hash,
            &[gai::store::Scope::Admin],
            None,
        )
        .await
        .unwrap();
    let k = Some(key.secret.as_str());

    let (s, _) = call(
        Method::POST,
        &format!("{u}/v1/agents"),
        k,
        Some(json!({"name": "poet", "system": "Rhyme."})),
    )
    .await;
    assert_eq!(s, 201);
    call(
        Method::POST,
        &format!("{u}/v1/keys"),
        k,
        Some(json!({"name": "ci", "scopes": ["run"]})),
    )
    .await;
    // Reads and runs are not audited.
    call(Method::GET, &format!("{u}/v1/agents"), k, None).await;
    call(
        Method::POST,
        &format!("{u}/v1/runs"),
        k,
        Some(json!({"input": "hi"})),
    )
    .await;
    // A failed change is, with its status.
    call(Method::DELETE, &format!("{u}/v1/agents/ghost"), k, None).await;

    let (s, body) = call(Method::GET, &format!("{u}/v1/audit"), k, None).await;
    assert_eq!(s, 200, "{body}");
    let rows: Vec<Value> = serde_json::from_str(&body).unwrap();
    let seen: Vec<(String, String, u64)> = rows
        .iter()
        .map(|r| {
            (
                r["action"].as_str().unwrap().to_owned(),
                r["target"].as_str().unwrap().to_owned(),
                r["status"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        seen,
        [
            ("DELETE /v1/agents/ghost".into(), "agents/ghost".into(), 404),
            ("POST /v1/keys".into(), "keys/ci".into(), 201),
            ("POST /v1/agents".into(), "agents/poet".into(), 201),
        ],
        "{body}"
    );
    assert!(rows.iter().all(|r| r["actor"] == "ada"));
    assert_eq!(rows[1]["detail"], json!({"scopes": ["run"]}));
    assert!(!body.contains(&key.secret));

    let (_, csv) = call(
        Method::GET,
        &format!("{u}/v1/audit?format=csv&target=agents/"),
        k,
        None,
    )
    .await;
    assert!(csv.starts_with("id,at,actor"), "{csv}");
    assert_eq!(csv.lines().count(), 3, "{csv}");
}

#[tokio::test]
async fn retention_and_forgetting_users() {
    let (u, _gai, _d) = server(AuthMode::Off).await;
    // A spec with zero-day retention: everything is old enough.
    call(
        Method::POST,
        &format!("{u}/v1/agents"),
        None,
        Some(json!({"name": "brief", "safety": {"retention_days": 0}})),
    )
    .await;
    call(
        Method::POST,
        &format!("{u}/v1/agents"),
        None,
        Some(json!({"name": "keeper"})),
    )
    .await;
    for agent in ["brief", "keeper"] {
        let (s, b) = call(
            Method::POST,
            &format!("{u}/v1/runs"),
            None,
            Some(json!({"agent": agent, "input": "hello", "user_id": "ada"})),
        )
        .await;
        assert_eq!(s, 200, "{b}");
    }
    let (_, swept) = call(Method::POST, &format!("{u}/v1/retention/sweep"), None, None).await;
    let swept: Value = serde_json::from_str(&swept).unwrap();
    assert_eq!(swept["agents"][0]["agent"], "brief", "{swept}");
    assert_eq!(swept["agents"][0]["sessions"], 1);
    assert_eq!(
        swept["agents"].as_array().unwrap().len(),
        1,
        "keeper has no retention"
    );

    let (s, gone) = call(Method::DELETE, &format!("{u}/v1/users/ada"), None, None).await;
    assert_eq!(s, 200, "{gone}");
    let gone: Value = serde_json::from_str(&gone).unwrap();
    assert_eq!(gone["sessions"], 1, "{gone}");
    let (_, sessions) = call(Method::GET, &format!("{u}/v1/sessions"), None, None).await;
    assert_eq!(sessions, "[]");
}

#[test]
fn governance_needs_admin() {
    use gai::store::Scope;
    assert_eq!(required_scope(&Method::GET, "/v1/audit"), Scope::Admin);
    assert_eq!(
        required_scope(&Method::DELETE, "/v1/users/ada"),
        Scope::Admin
    );
    assert_eq!(
        required_scope(&Method::POST, "/v1/retention/sweep"),
        Scope::Admin
    );
}
