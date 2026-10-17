//! Triggers over HTTP: managing them, delivering webhooks, and auth.

use std::sync::Arc;
use std::time::Duration;

use gai::agent::AgentSpec;
use gai::store::{Scope, keys};
use gai::{AuthMode, Config, Gai};
use gai_server::{AuthPolicy, router};
use reqwest::StatusCode;
use serde_json::{Value, json};

#[tokio::test]
async fn webhook_triggers_start_runs_and_need_their_secret() {
    let dir = tempfile::tempdir().unwrap();
    let gai = Arc::new(
        Gai::from_config(
            Config {
                data_dir: Some(dir.path().to_path_buf()),
                default_model: Some("mock/echo".into()),
                ..Default::default()
            },
            None,
        )
        .unwrap(),
    );
    let rt = gai.runtime().await.unwrap();
    rt.save_agent(&AgentSpec::new("triage").model("mock/echo"), None)
        .await
        .unwrap();
    let app = router(
        gai.clone(),
        AuthPolicy::new(AuthMode::Required, false, 0),
        None,
    )
    .await
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let store = rt.store().clone();
    let make = |scopes: &'static [Scope]| {
        let store = store.clone();
        async move {
            let k = keys::generate();
            store
                .create_key("t", &k.prefix, &k.hash, scopes, None)
                .await
                .unwrap();
            k.secret
        }
    };
    let admin = make(&[Scope::Admin]).await;
    let reader = make(&[Scope::Read]).await;
    let http = reqwest::Client::new();

    // Only admins manage triggers.
    let body = json!({
        "name": "issues",
        "kind": "webhook",
        "target": {"agent": "triage"},
        "input": "New issue: {{ payload.title }}"
    });
    let r = http
        .post(format!("{url}/v1/triggers"))
        .bearer_auth(&reader)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let created: Value = http
        .post(format!("{url}/v1/triggers"))
        .bearer_auth(&admin)
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let secret = created["secret"].as_str().unwrap().to_owned();
    let path = created["trigger"]["webhook_path"]
        .as_str()
        .unwrap()
        .to_owned();
    // The secret never shows in listings.
    let listed: Value = http
        .get(format!("{url}/v1/triggers"))
        .bearer_auth(&reader)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!listed.to_string().contains(&secret));

    // Webhooks need no API key, but do need the secret.
    let hook = |token: Option<&str>| {
        let mut rb = http
            .post(format!("{url}{path}"))
            .json(&json!({"title": "Login is broken"}));
        if let Some(t) = token {
            rb = rb.header("x-gai-token", t);
        }
        rb.send()
    };
    assert_eq!(hook(None).await.unwrap().status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        hook(Some("wrong")).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let r = hook(Some(&secret)).await.unwrap();
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let fired: Value = r.json().await.unwrap();
    let run_id = fired["run_id"].as_str().unwrap().to_owned();
    let mut run = None;
    for _ in 0..100 {
        let r = store.get_run(&run_id).await.unwrap().unwrap();
        if r.status.is_terminal() {
            run = Some(r);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let run = run.expect("the queued run finishes");
    assert_eq!(run.output.unwrap(), json!("New issue: Login is broken"));
    let events: Value = http
        .get(format!("{url}/v1/triggers/issues/events"))
        .bearer_auth(&reader)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(events[0]["reason"], "webhook");
    assert_eq!(events[0]["payload"]["title"], "Login is broken");

    // A disabled webhook looks like no webhook at all.
    http.post(format!("{url}/v1/triggers/issues/disable"))
        .bearer_auth(&admin)
        .send()
        .await
        .unwrap();
    assert_eq!(
        hook(Some(&secret)).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );

    // The MCP endpoint is behind the same keys as everything else.
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}});
    let r = http
        .post(format!("{url}/mcp"))
        .header("accept", "application/json, text/event-stream")
        .json(&init)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
}
