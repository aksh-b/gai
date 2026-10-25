//! The server over real HTTP: auth modes, scopes, budgets, rate limits, runs,
//! SSE, the OpenAI-compatible endpoint and the OpenAPI document.

use std::sync::Arc;

use gai::store::{Scope, keys};
use gai::{AuthMode, Config, Gai};
use gai_server::{AuthPolicy, router};
use reqwest::StatusCode;
use serde_json::{Value, json};

struct Server {
    url: String,
    gai: Arc<Gai>,
    _dir: tempfile::TempDir,
}

impl Server {
    async fn start(mode: AuthMode, local: bool, per_minute: u32) -> Self {
        Self::start_in(tempfile::tempdir().unwrap(), mode, local, per_minute).await
    }

    /// A server over the data in `dir`, as if a new process started there.
    async fn start_in(
        dir: tempfile::TempDir,
        mode: AuthMode,
        local: bool,
        per_minute: u32,
    ) -> Self {
        let config = Config {
            data_dir: Some(dir.path().to_path_buf()),
            default_model: Some("mock/echo".into()),
            agents_dirs: vec![dir.path().join("agents")],
            ..Default::default()
        };
        let gai = Arc::new(Gai::from_config(config, None).unwrap());
        let app = router(gai.clone(), AuthPolicy::new(mode, local, per_minute), None)
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Server {
            url,
            gai,
            _dir: dir,
        }
    }

    async fn key(&self, scopes: &[Scope], budget: Option<f64>) -> (String, String) {
        let k = keys::generate();
        let store = self.gai.runtime().await.unwrap().store().clone();
        let rec = store
            .create_key(&gai::store::NewKey::new(
                "test", &k.prefix, &k.hash, scopes, budget,
            ))
            .await
            .unwrap();
        (k.secret, rec.id)
    }

    fn get(&self, path: &str, key: Option<&str>) -> reqwest::RequestBuilder {
        auth(
            reqwest::Client::new().get(format!("{}{path}", self.url)),
            key,
        )
    }

    fn post(&self, path: &str, key: Option<&str>, body: Value) -> reqwest::RequestBuilder {
        auth(
            reqwest::Client::new()
                .post(format!("{}{path}", self.url))
                .json(&body),
            key,
        )
    }
}

fn auth(rb: reqwest::RequestBuilder, key: Option<&str>) -> reqwest::RequestBuilder {
    match key {
        Some(k) => rb.bearer_auth(k),
        None => rb,
    }
}

#[tokio::test]
async fn auto_mode_is_open_until_the_first_key() {
    let s = Server::start(AuthMode::Auto, true, 0).await;
    let h: Value = s
        .get("/v1/health", None)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(h["auth"], "open");
    assert_eq!(
        s.get("/v1/runs", None).send().await.unwrap().status(),
        StatusCode::OK
    );

    // Creating a key (anonymous callers are admin while open) closes the door.
    let created: Value = s
        .post("/v1/keys", None, json!({"name": "ci", "scopes": ["admin"]}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let secret = created["secret"].as_str().unwrap().to_owned();
    assert!(secret.starts_with("gai_"));
    assert_eq!(
        s.get("/v1/runs", None).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        s.get("/v1/runs", Some(&secret))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        s.get("/v1/runs", Some("gai_wrong"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let me: Value = s
        .get("/v1/me", Some(&secret))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(me["principal"]["name"], "ci");

    // x-api-key works too, and revoked keys stop working.
    let ok = reqwest::Client::new()
        .get(format!("{}/v1/runs", s.url))
        .header("x-api-key", &secret)
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    let id = created["key"]["id"].as_str().unwrap();
    let del = auth(
        reqwest::Client::new().delete(format!("{}/v1/keys/{id}", s.url)),
        Some(&secret),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(del.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        s.get("/v1/runs", Some(&secret))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn auto_mode_on_a_public_address_requires_keys() {
    let s = Server::start(AuthMode::Auto, false, 0).await;
    assert_eq!(
        s.get("/v1/runs", None).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    // Public: health, docs and the OpenAPI document.
    assert_eq!(
        s.get("/v1/health", None).send().await.unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(
        s.get("/openapi.json", None).send().await.unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(
        s.get("/docs", None).send().await.unwrap().status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn scopes_limit_what_keys_can_do() {
    let s = Server::start(AuthMode::Required, true, 0).await;
    let (read, _) = s.key(&[Scope::Read], None).await;
    let (run, _) = s.key(&[Scope::Run], None).await;
    assert_eq!(
        s.get("/v1/runs", Some(&read))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let body = json!({"input": "hi"});
    assert_eq!(
        s.post("/v1/runs", Some(&read), body.clone())
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        s.post("/v1/runs", Some(&run), body)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    // Agents and keys are admin-only to change.
    let agent = json!({"name": "x", "model": "mock/echo"});
    assert_eq!(
        s.post("/v1/agents", Some(&run), agent)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        s.get("/v1/keys", Some(&run)).send().await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn budgets_and_rate_limits() {
    let s = Server::start(AuthMode::Required, true, 3).await;
    let (key, id) = s.key(&[Scope::Run], Some(0.5)).await;
    let store = s.gai.runtime().await.unwrap().store().clone();
    assert_eq!(
        s.post("/v1/runs", Some(&key), json!({"input": "a"}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    store.add_key_spend(&id, 0.5).await.unwrap();
    let r = s
        .post("/v1/runs", Some(&key), json!({"input": "b"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::PAYMENT_REQUIRED);
    // Third request this minute is the limit; the fourth is refused.
    s.get("/v1/runs", Some(&key)).send().await.unwrap();
    let r = s.get("/v1/runs", Some(&key)).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(r.headers().contains_key("retry-after"));
}

#[tokio::test]
async fn runs_stream_and_trace() {
    let s = Server::start(AuthMode::Off, false, 0).await;
    let body = s
        .post(
            "/v1/runs",
            None,
            json!({"input": "call:calculator {\"expression\": \"6*7\"}", "mode": "stream"}),
        )
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let events: Vec<&str> = body
        .lines()
        .filter_map(|l| l.strip_prefix("event: "))
        .collect();
    assert_eq!(events.first(), Some(&"run_started"));
    assert!(events.contains(&"tool_result"));
    assert_eq!(events.last(), Some(&"run_finished"));

    let runs: Value = s
        .get("/v1/runs", None)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = runs[0]["id"].as_str().unwrap();
    assert!(
        runs[0]["metadata"].get("spec").is_none(),
        "lists leave the spec out"
    );
    let trace: Value = s
        .get(&format!("/v1/runs/{id}/trace"), None)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(trace["run"]["status"], "completed");
    assert_eq!(trace["steps"].as_array().unwrap().len(), 3);
    assert!(trace["children"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn openai_compatible_gateway_and_agents() {
    let s = Server::start(AuthMode::Off, false, 0).await;
    // Model passthrough.
    let r: Value = s
        .post(
            "/openai/v1/chat/completions",
            None,
            json!({"model": "mock/echo", "messages": [{"role": "user", "content": "ping"}]}),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["object"], "chat.completion");
    assert_eq!(r["choices"][0]["message"]["content"], "ping");
    assert_eq!(r["usage"]["total_tokens"], 15);

    // Tool calls come back in OpenAI form, and tool results go back in.
    let tools = json!([{"type": "function", "function": {"name": "lookup", "parameters": {"type": "object"}}}]);
    let r: Value = s
        .post(
            "/openai/v1/chat/completions",
            None,
            json!({"model": "mock/echo", "tools": tools, "messages": [{"role": "user", "content": "call:lookup {\"q\": 1}"}]}),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(r["choices"][0]["finish_reason"], "tool_calls");
    let call = &r["choices"][0]["message"]["tool_calls"][0];
    assert_eq!(call["function"]["name"], "lookup");
    assert_eq!(
        serde_json::from_str::<Value>(call["function"]["arguments"].as_str().unwrap()).unwrap(),
        json!({"q": 1})
    );
    let r: Value = s
        .post(
            "/openai/v1/chat/completions",
            None,
            json!({"model": "mock/echo", "tools": tools, "messages": [
                {"role": "user", "content": "call:lookup {\"q\": 1}"},
                {"role": "assistant", "content": null, "tool_calls": [call]},
                {"role": "tool", "tool_call_id": call["id"], "content": "found it"}
            ]}),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        r["choices"][0]["message"]["content"],
        "The tool returned: found it"
    );

    // Streaming, with usage and [DONE].
    let text = s
        .post(
            "/openai/v1/chat/completions",
            None,
            json!({"model": "mock/echo", "stream": true, "stream_options": {"include_usage": true}, "messages": [{"role": "user", "content": "stream me please"}]}),
        )
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let chunks: Vec<&str> = text
        .lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .collect();
    assert_eq!(chunks.last(), Some(&"[DONE]"));
    let content: String = chunks
        .iter()
        .filter_map(|c| serde_json::from_str::<Value>(c).ok())
        .filter_map(|v| {
            v["choices"][0]["delta"]["content"]
                .as_str()
                .map(str::to_owned)
        })
        .collect();
    assert_eq!(content, "stream me please");
    assert!(chunks.iter().any(|c| c.contains("\"usage\"")));

    // An agent, with its own tools, behind the same API.
    let r: Value = s
        .post(
            "/openai/v1/chat/completions",
            None,
            json!({"model": "agent:assistant", "messages": [{"role": "user", "content": "call:calculator {\"expression\": \"2+2\"}"}]}),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        r["choices"][0]["message"]["content"],
        "The tool returned: 4"
    );

    let models: Value = s
        .get("/openai/v1/models", None)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        models["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["id"] == "agent:assistant")
    );
}

#[tokio::test]
async fn openapi_document_describes_the_api() {
    let s = Server::start(AuthMode::Off, false, 0).await;
    let doc: Value = s
        .get("/openapi.json", None)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(doc["openapi"], "3.1.0");
    assert!(doc["paths"]["/v1/runs/{id}/events"]["get"]["responses"]["200"]["content"]["text/event-stream"].is_object());
    assert!(doc["components"]["schemas"]["RunOutcome"].is_object());
}

#[tokio::test]
async fn mcp_servers_are_managed_over_the_api() {
    let s = Server::start(AuthMode::Off, false, 0).await;
    let created = s
        .post(
            "/v1/mcp",
            None,
            json!({"name": "ghost", "config": {"command": "/no/such/mcp-server"}}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let info: Value = created.json().await.unwrap();
    assert_eq!(info["status"]["state"], "failed");
    let list: Value = s
        .get("/v1/mcp", None)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list[0]["name"], "ghost");
    assert_eq!(list[0]["source"], "api");

    let bad = s
        .post("/v1/mcp", None, json!({"name": "x", "config": {}}))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    let bad = s
        .post(
            "/v1/mcp",
            None,
            json!({"name": "no spaces", "config": {"url": "http://x"}}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);

    let del = reqwest::Client::new()
        .delete(format!("{}/v1/mcp/ghost", s.url))
        .send()
        .await
        .unwrap();
    assert_eq!(del.status(), StatusCode::NO_CONTENT);
    let list: Value = s
        .get("/v1/mcp", None)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(list.as_array().unwrap().is_empty());

    let hooks: Value = s
        .get("/v1/hooks", None)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(hooks, json!(["safety", "redact_secrets", "deny_shell"]));
}

#[tokio::test]
async fn a_restarted_server_resumes_runs_waiting_for_approval() {
    let first = Server::start(AuthMode::Off, false, 0).await;
    let started: Value = first
        .post(
            "/v1/runs",
            None,
            json!({"input": "call:shell {\"command\": \"echo resumed-ok\"}", "mode": "background"}),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let run_id = started["run_id"].as_str().unwrap().to_owned();
    for _ in 0..100 {
        let p: Value = first
            .get("/v1/approvals", None)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if !p.as_array().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    // A second process over the same data takes the run over.
    let Server { _dir: dir, .. } = first;
    let second = Server::start_in(dir, AuthMode::Off, false, 0).await;
    // The resumed run asks again as soon as it gets going.
    let mut call = None;
    for _ in 0..100 {
        let pending: Value = second
            .get("/v1/approvals", None)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        call = pending
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["run_id"] == run_id.as_str())
            .cloned();
        if call.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let call = call.expect("approval re-asked");
    let url = format!(
        "/v1/runs/{run_id}/approvals/{}",
        call["call_id"].as_str().unwrap()
    );
    assert_eq!(
        second
            .post(&url, None, json!({"approved": true}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let mut run = Value::Null;
    for _ in 0..200 {
        run = second
            .get(&format!("/v1/runs/{run_id}"), None)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if run["status"] == "completed" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(run["status"], "completed", "{run}");
    assert!(run["output"].as_str().unwrap().contains("resumed-ok"));
}
