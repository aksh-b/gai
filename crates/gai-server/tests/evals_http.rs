//! Feedback and evaluations over HTTP, on the mock models.

use std::sync::Arc;
use std::time::Duration;

use gai::{AuthMode, Config, Gai};
use gai_server::{AuthPolicy, required_scope, router};
use reqwest::Method;
use serde_json::{Value, json};

async fn server() -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        data_dir: Some(dir.path().to_path_buf()),
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
    (url, dir)
}

async fn call(method: Method, url: &str, body: Option<Value>) -> (u16, Value) {
    let mut rb = reqwest::Client::new().request(method, url);
    if let Some(b) = body {
        rb = rb.json(&b);
    }
    let r = rb.send().await.unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn feedback_is_saved_listed_and_counted() {
    let (u, _d) = server().await;
    let (_, out) = call(
        Method::POST,
        &format!("{u}/v1/runs"),
        Some(json!({"input": "hi"})),
    )
    .await;
    let run = out["run_id"].as_str().unwrap().to_owned();
    let (s, f) = call(
        Method::POST,
        &format!("{u}/v1/runs/{run}/feedback"),
        Some(json!({"rating": -1, "comment": "too short", "tags": ["incomplete", " "]})),
    )
    .await;
    assert_eq!(s, 200, "{f}");
    assert_eq!(
        (f["rating"].as_i64(), f["tags"].clone()),
        (Some(-1), json!(["incomplete"]))
    );
    let (_, f) = call(
        Method::POST,
        &format!("{u}/v1/runs/{run}/feedback"),
        Some(json!({"rating": 1})),
    )
    .await;
    assert_eq!(f["rating"], 1);
    let (_, list) = call(Method::GET, &format!("{u}/v1/runs/{run}/feedback"), None).await;
    assert_eq!(list.as_array().unwrap().len(), 1, "rating again replaces");
    let (_, sum) = call(Method::GET, &format!("{u}/v1/feedback/summary"), None).await;
    assert_eq!(
        (sum[0]["good"].as_u64(), sum[0]["bad"].as_u64()),
        (Some(1), Some(0))
    );
    let (s, _) = call(
        Method::POST,
        &format!("{u}/v1/runs/nope/feedback"),
        Some(json!({"rating": 1})),
    )
    .await;
    assert_eq!(s, 404);
    let (s, _) = call(Method::DELETE, &format!("{u}/v1/runs/{run}/feedback"), None).await;
    assert_eq!(s, 200);
}

#[tokio::test]
async fn eval_sets_run_in_the_background_and_compare() {
    let (u, _d) = server().await;
    call(
        Method::POST,
        &format!("{u}/v1/agents"),
        Some(json!({"name": "echoer", "system": "Echo."})),
    )
    .await;
    let yaml = "name: basics\nagent: echoer\nthreshold: 0.5\ncases:\n  - {id: hi, input: hello there, expected: hello}\n  - {id: no, input: nothing, expected: banana}\n";
    let (s, saved) = call(
        Method::POST,
        &format!("{u}/v1/evals"),
        Some(json!({"yaml": yaml})),
    )
    .await;
    assert_eq!(s, 200, "{saved}");
    assert_eq!(saved, json!({"name": "basics", "version": 1}));
    let (s, bad) = call(
        Method::POST,
        &format!("{u}/v1/evals"),
        Some(json!({"name": "x", "cases": []})),
    )
    .await;
    assert_eq!(s, 400, "{bad}");

    // Two models at once: one evaluation each.
    let (s, started) = call(
        Method::POST,
        &format!("{u}/v1/evals/basics/runs"),
        Some(json!({"models": ["mock/echo", "mock/echo"]})),
    )
    .await;
    assert_eq!(s, 202, "{started}");
    let ids: Vec<String> = started["eval_run_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids.len(), 2);
    let mut report = Value::Null;
    for _ in 0..100 {
        report = call(Method::GET, &format!("{u}/v1/eval-runs/{}", ids[0]), None)
            .await
            .1;
        if report["run"]["status"] == "completed" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(report["run"]["status"], "completed", "{report}");
    assert_eq!(report["summary"]["passed"], 1);
    assert_eq!(report["summary"]["ok"], true);
    for _ in 0..100 {
        if call(Method::GET, &format!("{u}/v1/eval-runs/{}", ids[1]), None)
            .await
            .1["run"]["status"]
            == "completed"
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (_, cmp) = call(
        Method::GET,
        &format!(
            "{u}/v1/eval-runs/compare?baseline={}&candidate={}",
            ids[0], ids[1]
        ),
        None,
    )
    .await;
    assert_eq!(cmp["score_delta"], 0.0, "{cmp}");
    assert_eq!(cmp["verdict"]["ok"], true);
    let (_, runs) = call(Method::GET, &format!("{u}/v1/evals/basics/runs"), None).await;
    assert_eq!(runs.as_array().unwrap().len(), 2);

    let (s, _) = call(
        Method::POST,
        &format!("{u}/v1/evals/missing/runs"),
        Some(json!({})),
    )
    .await;
    assert_eq!(s, 404);
    let (s, _) = call(
        Method::POST,
        &format!("{u}/v1/evals/basics/runs"),
        Some(json!({"agent": "ghost"})),
    )
    .await;
    assert_eq!(s, 404);
}

#[test]
fn scopes() {
    use gai::store::Scope;
    assert_eq!(required_scope(&Method::POST, "/v1/evals"), Scope::Build);
    assert_eq!(
        required_scope(&Method::DELETE, "/v1/evals/basics"),
        Scope::Build
    );
    assert_eq!(
        required_scope(&Method::POST, "/v1/evals/basics/runs"),
        Scope::Run
    );
    assert_eq!(
        required_scope(&Method::DELETE, "/v1/eval-runs/e1"),
        Scope::Build
    );
    assert_eq!(
        required_scope(&Method::POST, "/v1/runs/r1/feedback"),
        Scope::Run
    );
    assert_eq!(required_scope(&Method::GET, "/v1/evals"), Scope::Read);
}
