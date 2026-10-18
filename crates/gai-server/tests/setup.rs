//! First-run setup over HTTP: claiming a key from the dashboard, provider
//! keys and the default model saved from the dashboard, and templates.

use std::path::Path;
use std::sync::Arc;

use gai::{AuthMode, Config, Gai};
use gai_server::{AuthPolicy, router};
use reqwest::StatusCode;
use serde_json::{Value, json};

struct Server {
    url: String,
}

impl Server {
    /// A server over the data in `dir`, as if a new process started there.
    async fn start(dir: &Path, local: bool) -> Self {
        let config = Config {
            data_dir: Some(dir.to_path_buf()),
            default_model: Some("mock/echo".into()),
            ..Default::default()
        };
        let gai = Arc::new(Gai::from_config(config, None).unwrap());
        let app = router(gai, AuthPolicy::new(AuthMode::Auto, local, 0), None)
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Server { url }
    }

    fn req(
        &self,
        method: reqwest::Method,
        path: &str,
        key: Option<&str>,
    ) -> reqwest::RequestBuilder {
        let rb = reqwest::Client::new().request(method, format!("{}{path}", self.url));
        match key {
            Some(k) => rb.bearer_auth(k),
            None => rb,
        }
    }

    async fn json(&self, rb: reqwest::RequestBuilder) -> Value {
        let r = rb.send().await.unwrap();
        assert!(r.status().is_success(), "{}", r.status());
        r.json().await.unwrap()
    }

    async fn claim(&self, origin: Option<&str>) -> reqwest::Response {
        let mut rb = self.req(reqwest::Method::POST, "/v1/setup/claim", None);
        if let Some(o) = origin {
            rb = rb.header("origin", o);
        }
        rb.send().await.unwrap()
    }
}

#[tokio::test]
async fn the_dashboard_on_this_computer_can_claim_a_key() {
    let dir = tempfile::tempdir().unwrap();
    let s = Server::start(dir.path(), true).await;

    // Other websites, and requests without an Origin, get nothing.
    assert_eq!(
        s.claim(Some("http://evil.example")).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(s.claim(None).await.status(), StatusCode::FORBIDDEN);

    let r = s.claim(Some(&s.url)).await;
    assert_eq!(r.status(), StatusCode::CREATED);
    let claimed: Value = r.json().await.unwrap();
    let secret = claimed["secret"].as_str().unwrap().to_owned();

    // Keys exist now: requests need one, and the claimed key is an admin key.
    let anon = s
        .req(reqwest::Method::GET, "/v1/keys", None)
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), StatusCode::UNAUTHORIZED);
    s.json(s.req(reqwest::Method::GET, "/v1/keys", Some(&secret)))
        .await;

    // Browsers send no Origin on a same-origin GET.
    let status = s
        .json(s.req(reqwest::Method::GET, "/v1/setup/status", None))
        .await;
    assert_eq!(status["needs_key"], true);
    assert_eq!(status["can_claim"], true);
    let status = s
        .json(s.req(reqwest::Method::GET, "/v1/setup/status", Some(&secret)))
        .await;
    assert_eq!(status["needs_key"], false);
    // Only the demo model is configured.
    assert_eq!(status["has_chat_model"], false);
}

#[tokio::test]
async fn servers_reachable_from_elsewhere_never_hand_out_keys() {
    let dir = tempfile::tempdir().unwrap();
    let s = Server::start(dir.path(), false).await;
    assert_eq!(s.claim(Some(&s.url)).await.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn provider_keys_and_the_default_model_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let s = Server::start(dir.path(), true).await;
    let put = reqwest::Method::PUT;

    let key = s
        .json(
            s.req(put.clone(), "/v1/providers/anthropic/key", None)
                .json(&json!({"key": "sk-ant-secret-1234"})),
        )
        .await;
    assert_eq!(
        key,
        json!({"provider": "anthropic", "connected": true, "saved_key": "1234"})
    );
    let unknown = s
        .req(put.clone(), "/v1/providers/nope/key", None)
        .json(&json!({"key": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);

    let settings = s
        .json(
            s.req(put.clone(), "/v1/settings", None)
                .json(&json!({"default_model": "mock/other"})),
        )
        .await;
    assert_eq!(settings["default_model"], "mock/other");
    assert_eq!(settings["default_model_source"], "settings");
    let bad = s
        .req(put.clone(), "/v1/settings", None)
        .json(&json!({"default_model": "nope/x"}))
        .send()
        .await
        .unwrap();
    assert!(bad.status().is_client_error() || bad.status().is_server_error());

    // A new process over the same data.
    let s = Server::start(dir.path(), true).await;
    let status = s
        .json(s.req(reqwest::Method::GET, "/v1/setup/status", None))
        .await;
    let text = status.to_string();
    assert!(
        !text.contains("sk-ant-secret"),
        "the key must never be sent back"
    );
    let or = status["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "anthropic")
        .unwrap();
    assert_eq!(or["connected"], true);
    assert_eq!(or["saved_key"], "1234");
    assert_eq!(status["has_chat_model"], true);
    assert_eq!(status["default_model"], "mock/other");
    let health = s
        .json(s.req(reqwest::Method::GET, "/v1/health", None))
        .await;
    assert_eq!(health["default_model"], "mock/other");
    let models = s
        .json(s.req(reqwest::Method::GET, "/v1/models", None))
        .await;
    assert!(
        models
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["provider"] == "anthropic" && m["key"]["status"] == "present")
    );

    // Back to the configured default and key.
    let settings = s
        .json(
            s.req(put, "/v1/settings", None)
                .json(&json!({"default_model": null})),
        )
        .await;
    assert_eq!(settings["default_model"], "mock/echo");
    assert_eq!(settings["default_model_source"], "config");
    let key = s
        .json(s.req(reqwest::Method::DELETE, "/v1/providers/anthropic/key", None))
        .await;
    assert_eq!(key["saved_key"], Value::Null);
}

#[tokio::test]
async fn templates_install_with_the_agents_they_use() {
    let dir = tempfile::tempdir().unwrap();
    let s = Server::start(dir.path(), true).await;
    let list = s
        .json(s.req(reqwest::Method::GET, "/v1/templates", None))
        .await;
    assert!(
        list.as_array()
            .unwrap()
            .iter()
            .all(|t| t["installed"] == false)
    );

    let done = s
        .json(s.req(
            reqwest::Method::POST,
            "/v1/templates/research-brief/install",
            None,
        ))
        .await;
    assert_eq!(done["kind"], "workflow");
    assert_eq!(done["also"], json!(["researcher", "writer"]));
    s.json(s.req(reqwest::Method::GET, "/v1/agents/researcher", None))
        .await;
    s.json(s.req(reqwest::Method::GET, "/v1/workflows/research-brief", None))
        .await;

    // Installing again keeps what is there.
    let again = s
        .json(s.req(
            reqwest::Method::POST,
            "/v1/templates/supervisor/install",
            None,
        ))
        .await;
    assert_eq!(again["also"], json!([]));
    let missing = s
        .req(reqwest::Method::POST, "/v1/templates/nope/install", None)
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}
