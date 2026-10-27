//! The prompt optimizer over HTTP, on the mock models: started in the
//! background, polled, and applied as the agent's next version.

use std::sync::Arc;
use std::time::Duration;

use gai::{AuthMode, Config, Gai};
use gai_server::{AuthPolicy, router};
use reqwest::Method;
use serde_json::{Value, json};

async fn call(url: &str, m: Method, path: &str, body: Option<Value>) -> (u16, Value) {
    let mut rb = reqwest::Client::new().request(m, format!("{url}{path}"));
    if let Some(b) = body {
        rb = rb.json(&b);
    }
    let r = rb.send().await.unwrap();
    (
        r.status().as_u16(),
        r.json::<Value>().await.unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn optimizes_and_applies_the_best_instructions() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        data_dir: Some(dir.path().join("data")),
        default_model: Some("mock/echo".into()),
        ..Default::default()
    };
    let gai = Arc::new(Gai::from_config(config, None).unwrap());
    let app = router(gai, AuthPolicy::new(AuthMode::Off, true, 0), None)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let (st, _) = call(
        &url,
        Method::POST,
        "/v1/agents",
        Some(json!({"name": "support", "model": "mock/obey", "system": "You answer customers."})),
    )
    .await;
    assert_eq!(st, 201);
    let yaml = "name: refunds\nagent: support\ncases:\n  - input: How long is the refund window?\n    expected: 30 days\n  - input: hello\n    expected: hello\n";
    let (st, _) = call(&url, Method::POST, "/v1/evals", Some(json!({"yaml": yaml}))).await;
    assert_eq!(st, 200);

    let (st, _) = call(
        &url,
        Method::POST,
        "/v1/evals/missing/optimize",
        Some(json!({})),
    )
    .await;
    assert_eq!(st, 404);
    let (st, started) = call(
        &url,
        Method::POST,
        "/v1/evals/refunds/optimize",
        // `model` is what the assistant runs on, `writer_model` what rewrites.
        Some(json!({"rounds": 2, "candidates": 1, "model": "mock/obey",
            "writer_model": "mock/rewriter"})),
    )
    .await;
    assert_eq!(st, 202, "{started}");
    let id = started["id"].as_str().unwrap().to_owned();

    let mut o = Value::Null;
    for _ in 0..100 {
        (_, o) = call(&url, Method::GET, &format!("/v1/optimizations/{id}"), None).await;
        if o["status"] != "running" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(o["status"], "done", "{o}");
    let report = &o["report"];
    assert_eq!(report["baseline"]["passed"], 1);
    assert_eq!(report["best"]["passed"], 2);
    assert_eq!(report["improved"], true);

    let (_, list) = call(&url, Method::GET, "/v1/optimizations", None).await;
    assert_eq!(list[0]["id"], json!(id));

    let (st, saved) = call(
        &url,
        Method::POST,
        &format!("/v1/optimizations/{id}/apply"),
        Some(json!({})),
    )
    .await;
    assert_eq!((st, saved["version"].clone()), (201, json!(2)), "{saved}");
    let (_, agent) = call(&url, Method::GET, "/v1/agents/support", None).await;
    let system = agent["spec"]["system"].as_str().unwrap_or_default();
    assert!(system.contains("answer: 30 days"), "{agent}");
    let (_, versions) = call(&url, Method::GET, "/v1/agents/support/versions", None).await;
    assert!(
        versions.to_string().contains("score 0.50 → 1.00"),
        "{versions}"
    );
    let (st, _) = call(
        &url,
        Method::POST,
        &format!("/v1/optimizations/{id}/apply"),
        Some(json!({})),
    )
    .await;
    assert_eq!(st, 409, "applied once");
    let (_, o) = call(&url, Method::GET, &format!("/v1/optimizations/{id}"), None).await;
    assert_eq!(o["applied"]["version"], 2);
}
