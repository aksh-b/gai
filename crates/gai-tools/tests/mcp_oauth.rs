//! MCP OAuth sign-in against a fake authorization server, including a
//! provider that hits the redirect URI twice at once.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use gai_tools::mcp::{McpCredentials, McpManager, McpServer, McpServerConfig};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Default)]
struct Creds(Mutex<Option<Value>>);

#[async_trait]
impl McpCredentials for Creds {
    async fn load(&self, _server: &str) -> Option<Value> {
        self.0.lock().unwrap().clone()
    }
    async fn save(&self, _server: &str, c: Value) {
        *self.0.lock().unwrap() = Some(c);
    }
    async fn clear(&self, _server: &str) {
        *self.0.lock().unwrap() = None;
    }
}

async fn fake_provider() -> MockServer {
    let s = MockServer::start().await;
    let base = s.uri();
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-protected-resource/mcp"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "resource": format!("{base}/mcp"), "authorization_servers": [base]
        })))
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-authorization-server"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": base,
            "authorization_endpoint": format!("{base}/authorize"),
            "token_endpoint": format!("{base}/token"),
            "registration_endpoint": format!("{base}/register"),
            "response_types_supported": ["code"],
            "code_challenge_methods_supported": ["S256"],
            "authorization_response_iss_parameter_supported": true
        })))
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/register"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "client_id": "client-1", "redirect_uris": ["http://localhost/cb"]
        })))
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(300))
                .set_body_json(json!({"access_token": "at", "token_type": "Bearer", "expires_in": 3600, "refresh_token": "rt"})),
        )
        .expect(1)
        .mount(&s)
        .await;
    s
}

#[tokio::test]
async fn concurrent_callbacks_share_one_exchange() {
    let provider = fake_provider().await;
    let creds = Arc::new(Creds::default());
    let manager = Arc::new(McpManager::default());
    manager.set_credentials(creds.clone());
    manager.add(McpServer::new(
        "fake",
        McpServerConfig {
            url: Some(format!("{}/mcp", provider.uri())),
            ..Default::default()
        },
        "test",
    ));

    let auth_url = manager
        .begin_sign_in("fake", "http://localhost/cb")
        .await
        .unwrap();
    let state = reqwest::Url::parse(&auth_url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .into_owned();
    let callback = format!(
        "http://localhost/cb?code=c0de&state={state}&iss={}",
        provider.uri()
    );

    // The provider's page hits the callback twice at once; the browser
    // gives up on the first request.
    let first = {
        let m = manager.clone();
        let cb = callback.clone();
        tokio::spawn(async move { m.finish_sign_in(&cb).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    first.abort();
    let second = manager.finish_sign_in(&callback).await;
    assert_eq!(second, Ok("fake".to_owned()));
    // A later repeat sees the same outcome.
    assert_eq!(
        manager.finish_sign_in(&callback).await,
        Ok("fake".to_owned())
    );

    let saved = creds.0.lock().unwrap().clone().expect("tokens saved");
    assert_eq!(saved["client_id"], "client-1");
    assert_eq!(saved["token_response"]["access_token"], "at");

    assert!(
        manager
            .finish_sign_in("http://localhost/cb?code=x&state=nope")
            .await
            .is_err()
    );
}
