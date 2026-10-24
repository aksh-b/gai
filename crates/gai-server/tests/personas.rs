//! Personas over HTTP: saving and versioning them, the default, templates,
//! and picking one for a run.

use std::sync::Arc;

use gai::{AuthMode, Config, Gai};
use gai_server::{AuthPolicy, router};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

struct Server {
    url: String,
    _dir: tempfile::TempDir,
}

impl Server {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            data_dir: Some(dir.path().to_path_buf()),
            default_model: Some("mock/echo".into()),
            ..Default::default()
        };
        let gai = Arc::new(Gai::from_config(config, None).unwrap());
        let app = router(gai, AuthPolicy::new(AuthMode::Auto, true, 0), None)
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Server { url, _dir: dir }
    }

    async fn call(&self, method: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut rb = reqwest::Client::new().request(method, format!("{}{path}", self.url));
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        let r = rb.send().await.unwrap();
        let status = r.status();
        let text = r.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn ok(&self, method: Method, path: &str, body: Option<Value>) -> Value {
        let (status, v) = self.call(method, path, body).await;
        assert!(status.is_success(), "{path}: {status} {v}");
        v
    }
}

#[tokio::test]
async fn personas_are_versioned_labelled_and_rolled_back() {
    let s = Server::start().await;
    let v1 = json!({"name": "tutor", "display_name": "Tutor", "avatar": "🦉", "tone": "warm"});
    let saved = s.ok(Method::POST, "/v1/personas", Some(v1)).await;
    assert_eq!(saved, json!({"name": "tutor", "version": 1}));
    let v2 = json!({"name": "tutor", "display_name": "Tutor", "avatar": "🦉", "tone": "strict"});
    let saved = s
        .ok(Method::POST, "/v1/personas?note=stricter", Some(v2))
        .await;
    assert_eq!(saved["version"], 2);

    let list = s.ok(Method::GET, "/v1/personas", None).await;
    assert_eq!(list[0]["summary"]["avatar"], "🦉");
    assert_eq!(list[0]["summary"]["latest_version"], 2);
    assert_eq!(list[0]["spec"]["tone"], "strict");
    assert_eq!(list[0]["used_by"], json!([]));

    let old = s
        .ok(Method::GET, "/v1/personas/tutor?version=1", None)
        .await;
    assert_eq!(old["spec"]["tone"], "warm");
    let versions = s.ok(Method::GET, "/v1/personas/tutor/versions", None).await;
    assert_eq!(versions.as_array().unwrap().len(), 2);
    let diff = s.ok(Method::GET, "/v1/personas/tutor/diff", None).await;
    assert!(diff["diff"].as_str().unwrap().contains("+tone: strict"));

    s.ok(
        Method::POST,
        "/v1/personas/tutor/labels/prod",
        Some(json!({"version": 1})),
    )
    .await;
    let back = s
        .ok(
            Method::POST,
            "/v1/personas/tutor/rollback",
            Some(json!({"version": 1})),
        )
        .await;
    assert_eq!(back["version"], 3);
    let latest = s.ok(Method::GET, "/v1/personas/tutor", None).await;
    assert_eq!(latest["spec"]["tone"], "warm");

    let rendered = s
        .ok(
            Method::POST,
            "/v1/personas/render",
            Some(json!({"name": "x", "tone": "calm"})),
        )
        .await;
    assert!(rendered["text"].as_str().unwrap().contains("Tone: calm."));

    let (status, _) = s
        .call(
            Method::POST,
            "/v1/personas",
            Some(json!({"name": "bad name"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn runs_speak_as_the_chosen_persona() {
    let s = Server::start().await;
    s.ok(
        Method::POST,
        "/v1/personas",
        Some(json!({"name": "tutor", "tone": "warm"})),
    )
    .await;
    s.ok(
        Method::POST,
        "/v1/agents",
        Some(json!({"name": "helper", "system": "You help.", "persona": "tutor"})),
    )
    .await;

    // The agent's default persona.
    let out = s
        .ok(
            Method::POST,
            "/v1/runs",
            Some(json!({"agent": "helper", "input": "hi"})),
        )
        .await;
    let run = s
        .ok(
            Method::GET,
            &format!("/v1/runs/{}", out["run_id"].as_str().unwrap()),
            None,
        )
        .await;
    assert_eq!(run["metadata"]["persona"]["name"], "tutor");
    assert_eq!(run["metadata"]["persona"]["version"], 1);

    // An inline draft, as the dashboard's live preview sends it.
    let out = s
        .ok(
            Method::POST,
            "/v1/runs",
            Some(json!({"input": "hi", "persona": {"name": "draft", "tone": "dry"}})),
        )
        .await;
    let run = s
        .ok(
            Method::GET,
            &format!("/v1/runs/{}", out["run_id"].as_str().unwrap()),
            None,
        )
        .await;
    assert_eq!(run["metadata"]["persona"]["spec"]["tone"], "dry");
    assert!(run["metadata"]["persona"]["version"].is_null());
    // Lists keep which persona it was, not the whole spec.
    let runs = s.ok(Method::GET, "/v1/runs?limit=1", None).await;
    assert_eq!(runs[0]["metadata"]["persona"]["name"], "draft");
    assert!(runs[0]["metadata"]["persona"]["spec"].is_null());

    // Turned off for one run.
    let out = s
        .ok(
            Method::POST,
            "/v1/runs",
            Some(json!({"agent": "helper", "input": "hi", "persona": "none"})),
        )
        .await;
    let run = s
        .ok(
            Method::GET,
            &format!("/v1/runs/{}", out["run_id"].as_str().unwrap()),
            None,
        )
        .await;
    assert!(run["metadata"]["persona"].is_null());

    // A persona in use can't be deleted.
    let usage = s.ok(Method::GET, "/v1/personas/tutor/usage", None).await;
    assert_eq!(usage, json!(["helper"]));
    let (status, body) = s.call(Method::DELETE, "/v1/personas/tutor", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    s.ok(Method::DELETE, "/v1/agents/helper", None).await;
    let (status, _) = s.call(Method::DELETE, "/v1/personas/tutor", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn the_default_persona_applies_to_plain_chat() {
    let s = Server::start().await;
    let d = s.ok(Method::GET, "/v1/personas/defaults", None).await;
    assert_eq!(
        d,
        json!({"persona": null, "placement": "prepend", "source": "config"})
    );

    let (status, _) = s
        .call(
            Method::PUT,
            "/v1/personas/defaults",
            Some(json!({"persona": "ghost"})),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let installed = s
        .ok(Method::POST, "/v1/templates/friendly-tutor/install", None)
        .await;
    assert_eq!(installed["kind"], "persona");
    let templates = s.ok(Method::GET, "/v1/templates", None).await;
    let tutor = templates
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == "friendly-tutor")
        .unwrap();
    assert_eq!(tutor["installed"], true);

    let d = s
        .ok(
            Method::PUT,
            "/v1/personas/defaults",
            Some(json!({"persona": "friendly-tutor", "placement": "append"})),
        )
        .await;
    assert_eq!(d["persona"], "friendly-tutor");
    assert_eq!(d["source"], "settings");

    // The built-in assistant (no agent named) speaks as the default.
    let out = s
        .ok(Method::POST, "/v1/runs", Some(json!({"input": "hi"})))
        .await;
    let run = s
        .ok(
            Method::GET,
            &format!("/v1/runs/{}", out["run_id"].as_str().unwrap()),
            None,
        )
        .await;
    assert_eq!(run["metadata"]["persona"]["name"], "friendly-tutor");
    assert_eq!(run["metadata"]["persona"]["placement"], "append");

    let (status, _) = s
        .call(Method::DELETE, "/v1/personas/friendly-tutor", None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let d = s
        .ok(
            Method::PUT,
            "/v1/personas/defaults",
            Some(json!({"reset": true})),
        )
        .await;
    assert_eq!(d["source"], "config");
    assert!(d["persona"].is_null());
}
