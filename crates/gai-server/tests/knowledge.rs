//! Knowledge collections, memory blocks, skills and background runs over
//! HTTP.

use std::sync::Arc;

use base64::Engine;
use gai::store::{Scope, keys};
use gai::{AuthMode, Config, Gai};
use gai_server::{AuthPolicy, router};
use reqwest::StatusCode;
use serde_json::{Value, json};

struct Server {
    url: String,
    gai: Arc<Gai>,
    http: reqwest::Client,
    _dir: tempfile::TempDir,
}

impl Server {
    async fn start(mode: AuthMode) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            data_dir: Some(dir.path().to_path_buf()),
            default_model: Some("mock/echo".into()),
            skills_dirs: vec![dir.path().join("skills")],
            ..Default::default()
        };
        let gai = Arc::new(Gai::from_config(config, None).unwrap());
        let app = router(gai.clone(), AuthPolicy::new(mode, true, 0), None)
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Server {
            url,
            gai,
            http: reqwest::Client::new(),
            _dir: dir,
        }
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        key: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut rb = self
            .http
            .request(method.parse().unwrap(), format!("{}{path}", self.url));
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        if let Some(k) = key {
            rb = rb.bearer_auth(k);
        }
        let r = rb.send().await.unwrap();
        let status = r.status();
        let text = r.text().await.unwrap();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }

    async fn get(&self, path: &str) -> (StatusCode, Value) {
        self.call("GET", path, None, None).await
    }

    async fn post(&self, path: &str, body: Value) -> (StatusCode, Value) {
        self.call("POST", path, Some(body), None).await
    }
}

#[tokio::test]
async fn collections_ingest_and_search() {
    let s = Server::start(AuthMode::Off).await;
    let (st, c) = s
        .post(
            "/v1/collections",
            json!({"name": "handbook", "chunk_tokens": 100}),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{c}");
    assert_eq!(c["embedding_model"], "mock/hash");
    let (st, _) = s.post("/v1/collections", json!({"name": "handbook"})).await;
    assert_eq!(st, StatusCode::CONFLICT);
    let (st, _) = s
        .post("/v1/collections", json!({"name": "no spaces"}))
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    let (st, d) = s
        .post(
            "/v1/collections/handbook/documents",
            json!({"source": "leave", "title": "Leave policy", "text": "Staff get 25 days of annual leave."}),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{d}");
    let (st, again) = s
        .post(
            "/v1/collections/handbook/documents",
            json!({"source": "leave", "title": "Leave policy", "text": "Staff get 25 days of annual leave."}),
        )
        .await;
    assert_eq!(
        (st, again["unchanged"].as_bool()),
        (StatusCode::OK, Some(true))
    );

    let md = "# Expenses\nClaims over 500 euros need a manager's sign-off.\n";
    let (st, file) = s
        .post(
            "/v1/collections/handbook/documents",
            json!({"filename": "expenses.md",
                   "content_base64": base64::engine::general_purpose::STANDARD.encode(md)}),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{file}");
    assert_eq!(file["document"]["title"], "Expenses");
    assert_eq!(file["document"]["metadata"]["format"], "markdown");
    let (st, _) = s
        .post(
            "/v1/collections/handbook/documents",
            json!({"text": "x", "url": "http://a"}),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "exactly one input");
    let (st, _) = s
        .post(
            "/v1/collections/handbook/documents",
            json!({"url": "http://127.0.0.1:9/private"}),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "private URLs are refused");

    let (_, docs) = s.get("/v1/collections/handbook/documents").await;
    assert_eq!(docs.as_array().unwrap().len(), 2);
    let (_, hits) = s
        .post(
            "/v1/collections/handbook/search",
            json!({"query": "manager sign-off", "k": 1}),
        )
        .await;
    assert_eq!(hits[0]["source"], "expenses.md", "{hits}");
    let (_, hits) = s
        .post(
            "/v1/collections/handbook/search",
            json!({"query": "annual leave", "mode": "text"}),
        )
        .await;
    assert_eq!(hits[0]["title"], "Leave policy");

    let id = d["document"]["id"].as_str().unwrap();
    let (st, view) = s
        .get(&format!("/v1/collections/handbook/documents/{id}"))
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(view["chunks"].as_array().unwrap().len(), 1);
    let (st, _) = s
        .call(
            "DELETE",
            &format!("/v1/collections/handbook/documents/{id}"),
            None,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (_, c) = s.get("/v1/collections/handbook").await;
    assert_eq!(
        (c["documents"].as_u64(), c["chunks"].as_u64()),
        (Some(1), Some(1))
    );

    // Archival memory collections are hidden unless asked for.
    let rt = s.gai.runtime().await.unwrap();
    rt.knowledge()
        .ensure_collection("memory:agent:x", "Archival memory")
        .await
        .unwrap();
    let (_, list) = s.get("/v1/collections").await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    let (_, list) = s.get("/v1/collections?all=true").await;
    assert_eq!(list.as_array().unwrap().len(), 2);
    let (st, _) = s
        .call("DELETE", "/v1/collections/handbook", None, None)
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn memory_blocks_and_skills() {
    let s = Server::start(AuthMode::Required).await;
    let admin = {
        let k = keys::generate();
        let store = s.gai.runtime().await.unwrap().store().clone();
        store
            .create_key("a", &k.prefix, &k.hash, &[Scope::Admin], None)
            .await
            .unwrap();
        k.secret
    };
    let runner = {
        let k = keys::generate();
        let store = s.gai.runtime().await.unwrap().store().clone();
        store
            .create_key("r", &k.prefix, &k.hash, &[Scope::Run], None)
            .await
            .unwrap();
        k.secret
    };
    let a = Some(admin.as_str());

    let (st, b) = s
        .call(
            "POST",
            "/v1/memory/blocks",
            Some(json!({"scope": "user:ann", "label": "human", "value": "Ann.", "limit": 20})),
            a,
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{b}");
    let id = b["id"].as_str().unwrap().to_owned();
    let (st, _) = s
        .call(
            "POST",
            "/v1/memory/blocks",
            Some(json!({"scope": "team", "label": "x", "limit": 5})),
            a,
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, b) = s
        .call(
            "PATCH",
            &format!("/v1/memory/blocks/{id}"),
            Some(json!({"value": "Ann. Likes tea."})),
            a,
        )
        .await;
    assert_eq!((st, b["version"].as_u64()), (StatusCode::OK, Some(2)));
    let (st, _) = s
        .call(
            "PATCH",
            &format!("/v1/memory/blocks/{id}"),
            Some(json!({"value": "x".repeat(21)})),
            a,
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (_, h) = s
        .call("GET", &format!("/v1/memory/blocks/{id}/history"), None, a)
        .await;
    assert_eq!(h[0]["op"], "edit");
    assert!(
        h[0]["actor"].as_str().unwrap().starts_with("api:key_"),
        "{h}"
    );
    let (_, list) = s
        .call("GET", "/v1/memory/blocks?scope=user:ann", None, a)
        .await;
    assert_eq!(list.as_array().unwrap().len(), 1);

    let skill = "---\nname: tidy\ndescription: Tidy up notes.\n---\nSort, then dedupe.";
    let (st, _) = s
        .call(
            "POST",
            "/v1/skills",
            Some(json!({"content": skill})),
            Some(&runner),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "uploading skills needs admin");
    let (st, saved) = s
        .call(
            "POST",
            "/v1/skills",
            Some(json!({"content": skill, "files": {"ref/a.md": "A"}})),
            a,
        )
        .await;
    assert_eq!(
        (st, saved["version"].as_u64()),
        (StatusCode::CREATED, Some(1))
    );
    let (_, list) = s.call("GET", "/v1/skills", None, Some(&runner)).await;
    assert_eq!(list[0]["source"]["kind"], "stored");
    assert_eq!(list[0]["files"], json!(["ref/a.md"]));
    let (_, one) = s.call("GET", "/v1/skills/tidy", None, a).await;
    assert_eq!(one["instructions"], "Sort, then dedupe.");
    let (st, _) = s
        .call(
            "POST",
            "/v1/skills",
            Some(json!({"content": "no front matter"})),
            a,
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = s.call("DELETE", "/v1/skills/tidy", None, a).await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    // A run for a user creates that user's profile block from the agent.
    let agent = json!({"name": "pal", "memory": {"blocks": [{"label": "human", "scope": "user"}]}});
    let (st, out) = s
        .call(
            "POST",
            "/v1/runs",
            Some(json!({"agent": agent, "input": "hi", "user_id": "bob"})),
            Some(&runner),
        )
        .await;
    assert_eq!(
        (st, out["status"].as_str()),
        (StatusCode::OK, Some("completed")),
        "{out}"
    );
    let (_, list) = s
        .call("GET", "/v1/memory/blocks?scope=user:bob", None, a)
        .await;
    assert_eq!(list[0]["label"], "human");
}

#[tokio::test]
async fn background_runs_are_queued_and_streamable() {
    let s = Server::start(AuthMode::Off).await;
    let (st, ids) = s
        .post(
            "/v1/runs",
            json!({"input": "queued hello", "mode": "background"}),
        )
        .await;
    assert_eq!(st, StatusCode::ACCEPTED, "{ids}");
    let id = ids["run_id"].as_str().unwrap();
    // The events stream waits for the queue to start the run, then follows it.
    let body = s
        .http
        .get(format!("{}/v1/runs/{id}/events", s.url))
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
    assert_eq!(events.first(), Some(&"run_started"), "{body}");
    assert_eq!(events.last(), Some(&"run_finished"));
    let (_, run) = s.get(&format!("/v1/runs/{id}")).await;
    assert_eq!(run["status"], "completed");
    assert_eq!(run["output"], "queued hello");
    assert_eq!(run["metadata"]["background"], true);

    let (st, e) = s
        .post(
            "/v1/runs",
            json!({"input": "x", "mode": "background", "callback_url": "http://127.0.0.1:1/hook"}),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{e}");
}

#[tokio::test]
async fn workflows_are_saved_validated_and_run() {
    let s = Server::start(AuthMode::Required).await;
    let store = s.gai.runtime().await.unwrap().store().clone();
    let mut keys_by_scope = Vec::new();
    for scope in [Scope::Admin, Scope::Run] {
        let k = keys::generate();
        store
            .create_key("k", &k.prefix, &k.hash, &[scope], None)
            .await
            .unwrap();
        keys_by_scope.push(k.secret);
    }
    let (admin, runner) = (
        Some(keys_by_scope[0].as_str()),
        Some(keys_by_scope[1].as_str()),
    );
    let wf = json!({
        "name": "greet",
        "description": "Say hello",
        "inputs": {"who": {}},
        "nodes": [
            {"id": "hello", "agent": {"name": "e", "model": "mock/echo"}, "input": "Hello, {{ inputs.who }}!"},
            {"id": "loud", "template": "{{ nodes.hello.output | upper }}"}
        ]
    });
    let (st, _) = s
        .call("POST", "/v1/workflows", Some(wf.clone()), runner)
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "saving needs admin");
    let (st, saved) = s
        .call("POST", "/v1/workflows", Some(wf.clone()), admin)
        .await;
    assert_eq!(
        (st, saved["version"].as_u64()),
        (StatusCode::CREATED, Some(1)),
        "{saved}"
    );

    let (_, v) = s
        .call(
            "POST",
            "/v1/workflows/validate",
            Some(json!({"nodes": [{"id": "a", "template": "{{ nodes.b.output }}"}]})),
            runner,
        )
        .await;
    assert_eq!(v["ok"], false);
    assert!(v["error"].as_str().unwrap().contains("unknown node b"));

    let (_, view) = s.call("GET", "/v1/workflows/greet", None, runner).await;
    assert_eq!(view["graph"][1]["deps"], json!(["hello"]));
    let (_, list) = s.call("GET", "/v1/workflows", None, runner).await;
    assert_eq!(list[0]["source"], "stored");

    let (st, out) = s
        .call(
            "POST",
            "/v1/workflows/greet/runs",
            Some(json!({"inputs": {"who": "ann"}})),
            runner,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{out}");
    assert_eq!(out["status"], "completed");
    assert_eq!(out["output"], "HELLO, ANN!");
    let (st, e) = s
        .call("POST", "/v1/workflows/greet/runs", Some(json!({})), runner)
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(
        e["error"]["message"]
            .as_str()
            .unwrap()
            .contains("missing input")
    );

    // Inline, in the background, followed over SSE.
    let (st, ids) = s
        .call("POST", "/v1/workflows/run",
              Some(json!({"workflow": {"nodes": [{"id": "x", "value": {"n": 1}}]}, "mode": "background"})), runner)
        .await;
    assert_eq!(st, StatusCode::ACCEPTED, "{ids}");
    let id = ids["run_id"].as_str().unwrap();
    let body = s
        .http
        .get(format!("{}/v1/runs/{id}/events", s.url))
        .bearer_auth(&keys_by_scope[1])
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
    assert_eq!(
        events,
        [
            "workflow_started",
            "node_started",
            "node_finished",
            "run_finished"
        ],
        "{body}"
    );
    let (_, trace) = s
        .call(
            "GET",
            &format!("/v1/runs/{}/trace", out["run_id"].as_str().unwrap()),
            None,
            runner,
        )
        .await;
    assert_eq!(trace["children"].as_array().unwrap().len(), 1);
    let (st, _) = s.call("DELETE", "/v1/workflows/greet", None, admin).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
}
