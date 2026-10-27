//! Prompt templates and the marketplace over HTTP: one server publishes a
//! listing, another uses it as a registry to browse, rate and install.

use std::sync::Arc;

use gai::store::{NewKey, Scope, keys};
use gai::{AuthMode, Config, Gai};
use gai_server::{AuthPolicy, router};
use reqwest::Method;
use serde_json::{Value, json};

struct Server {
    url: String,
    admin: String,
    member: String,
    _dir: tempfile::TempDir,
}

async fn server() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        data_dir: Some(dir.path().join("data")),
        default_model: Some("mock/echo".into()),
        ..Default::default()
    };
    let gai = Arc::new(Gai::from_config(config, None).unwrap());
    let store = gai.runtime().await.unwrap().store().clone();
    let key = |name: &'static str, scopes: &'static [Scope]| {
        let store = store.clone();
        async move {
            let k = keys::generate();
            store
                .create_key(&NewKey::new(name, &k.prefix, &k.hash, scopes, None))
                .await
                .unwrap();
            k.secret
        }
    };
    let admin = key("admin", &[Scope::Admin]).await;
    let member = key("member", &[Scope::Run]).await;
    let app = router(gai, AuthPolicy::new(AuthMode::Required, true, 0), None)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server {
        url,
        admin,
        member,
        _dir: dir,
    }
}

impl Server {
    async fn call(&self, m: Method, path: &str, key: &str, body: Option<Value>) -> (u16, Value) {
        let mut rb = reqwest::Client::new()
            .request(m, format!("{}{path}", self.url))
            .bearer_auth(key);
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        let r = rb.send().await.unwrap();
        (
            r.status().as_u16(),
            r.json::<Value>().await.unwrap_or(Value::Null),
        )
    }

    async fn admin(&self, m: Method, path: &str, body: Option<Value>) -> (u16, Value) {
        self.call(m, path, &self.admin.clone(), body).await
    }
}

#[tokio::test]
async fn prompt_templates_are_versioned_and_used_by_agents() {
    let s = server().await;
    let (st, saved) = s
        .admin(
            Method::POST,
            "/v1/prompts",
            Some(json!({"name": "tone", "text": "Speak for {{ company }}.",
                "variables": {"company": {"default": "Acme"}}})),
        )
        .await;
    assert_eq!((st, saved["version"].clone()), (201, json!(1)));
    let (_, saved) = s
        .admin(
            Method::POST,
            "/v1/prompts?note=shorter",
            Some(json!({"name": "tone", "text": "For {{ company }}."})),
        )
        .await;
    assert_eq!(saved["version"], 2);
    let (st, _) = s
        .admin(
            Method::POST,
            "/v1/prompts/tone/labels/prod",
            Some(json!({"version": 1})),
        )
        .await;
    assert_eq!(st, 200);
    let (_, r) = s
        .admin(
            Method::POST,
            "/v1/prompts/render",
            Some(json!({"prompt": {"name": "x", "text": "Hi {{ who }}",
                "variables": {"who": {"default": "there"}}}})),
        )
        .await;
    assert_eq!(r["text"], "Hi there");
    let (st, _) = s
        .admin(
            Method::POST,
            "/v1/prompts",
            Some(json!({"name": "bad", "text": "{{ oops"})),
        )
        .await;
    assert_eq!(st, 400);
    // Members can't change prompts.
    let (st, _) = s
        .call(
            Method::POST,
            "/v1/prompts",
            &s.member.clone(),
            Some(json!({"name": "tone", "text": "x"})),
        )
        .await;
    assert_eq!(st, 403);

    let (st, _) = s
        .admin(
            Method::POST,
            "/v1/agents",
            Some(json!({"name": "rep", "model": "mock/echo", "prompt": "tone@prod"})),
        )
        .await;
    assert_eq!(st, 201);
    let (_, list) = s.admin(Method::GET, "/v1/prompts", None).await;
    assert_eq!(list[0]["used_by"], json!(["rep"]));
    assert_eq!(list[0]["summary"]["labels"]["prod"], 1);
    let (st, out) = s
        .admin(
            Method::POST,
            "/v1/runs",
            Some(json!({"agent": "rep", "input": "hi"})),
        )
        .await;
    assert_eq!(st, 200, "{out}");
    let (_, run) = s
        .admin(
            Method::GET,
            &format!("/v1/runs/{}", out["run_id"].as_str().unwrap()),
            None,
        )
        .await;
    assert_eq!(run["metadata"]["prompt"]["version"], 1, "{run}");

    // In use: not deleted.
    let (st, _) = s.admin(Method::DELETE, "/v1/prompts/tone", None).await;
    assert_eq!(st, 409);
    let (_, versions) = s
        .admin(Method::GET, "/v1/prompts/tone/versions", None)
        .await;
    assert_eq!(versions[0]["note"], "shorter");
    let (st, rolled) = s
        .admin(
            Method::POST,
            "/v1/prompts/tone/rollback",
            Some(json!({"version": 1})),
        )
        .await;
    assert_eq!((st, rolled["version"].clone()), (201, json!(3)));
}

#[tokio::test]
async fn listings_are_published_rated_and_installed_from_another_server() {
    let a = server().await;
    let b = server().await;
    a.admin(
        Method::POST,
        "/v1/prompts",
        Some(json!({"name": "tone", "text": "Be kind."})),
    )
    .await;
    let (st, _) = a
        .admin(
            Method::POST,
            "/v1/agents",
            Some(
                json!({"name": "support", "model": "mock/echo", "prompt": "tone",
                "description": "Answers customers"}),
            ),
        )
        .await;
    assert_eq!(st, 201);
    let publish = json!({
        "name": "support-kit",
        "title": "Support kit",
        "description": "An assistant for customer questions",
        "tags": ["customers"],
        "items": {"agents": ["support"]},
    });
    let (st, listing) = a
        .admin(
            Method::POST,
            "/v1/marketplace/listings",
            Some(publish.clone()),
        )
        .await;
    assert_eq!(st, 201, "{listing}");
    assert_eq!(listing["version"], 1);
    assert_eq!(listing["manifest"]["contents"]["prompts"], json!(["tone"]));
    // Members can't publish, but can rate.
    let (st, _) = a
        .call(
            Method::POST,
            "/v1/marketplace/listings",
            &a.member.clone(),
            Some(publish.clone()),
        )
        .await;
    assert_eq!(st, 403);
    let mut again = publish.clone();
    again["note"] = json!("second");
    let (_, listing) = a
        .admin(Method::POST, "/v1/marketplace/listings", Some(again))
        .await;
    assert_eq!(listing["version"], 2);
    let (st, _) = a
        .call(
            Method::POST,
            "/v1/marketplace/listings/support-kit/ratings",
            &a.member.clone(),
            Some(json!({"stars": 4, "comment": "handy"})),
        )
        .await;
    assert_eq!(st, 200);
    let (st, _) = a
        .admin(
            Method::POST,
            "/v1/marketplace/listings/support-kit/ratings",
            Some(json!({"stars": 9})),
        )
        .await;
    assert_eq!(st, 400);
    let (_, found) = a
        .admin(Method::GET, "/v1/marketplace/browse?q=customer+kit", None)
        .await;
    assert_eq!(found.as_array().unwrap().len(), 1);
    let (_, none) = a
        .admin(Method::GET, "/v1/marketplace/browse?q=billing", None)
        .await;
    assert!(none.as_array().unwrap().is_empty());

    // B uses A as a registry, with A's member key.
    let (st, regs) = b
        .admin(
            Method::PUT,
            "/v1/marketplace/registries",
            Some(json!([{"name": "team", "url": a.url, "token": a.member}])),
        )
        .await;
    assert_eq!(st, 200, "{regs}");
    assert_eq!(regs[0]["has_token"], true);
    assert!(regs[0].get("token").is_none(), "tokens are never shown");
    let (st, _) = b
        .admin(
            Method::PUT,
            "/v1/marketplace/registries",
            Some(json!([{"name": "local", "url": a.url}])),
        )
        .await;
    assert_eq!(st, 400);

    let (st, list) = b
        .admin(Method::GET, "/v1/marketplace/browse?registry=team", None)
        .await;
    assert_eq!(st, 200, "{list}");
    assert_eq!(list[0]["registry"], "team");
    assert_eq!(list[0]["rating"], 4.0);
    let (_, d) = b
        .admin(Method::GET, "/v1/marketplace/browse/team/support-kit", None)
        .await;
    assert_eq!(d["versions"][0]["note"], "second");
    assert_eq!(d["reviews"][0]["comment"], "handy");

    let (st, peek) = b
        .admin(
            Method::POST,
            "/v1/marketplace/install",
            Some(json!({"registry": "team", "name": "support-kit", "dry_run": true})),
        )
        .await;
    assert_eq!(st, 200, "{peek}");
    assert!(peek["installed"].as_array().unwrap().is_empty());
    let (_, agents) = b.admin(Method::GET, "/v1/agents", None).await;
    assert!(!agents.to_string().contains("\"support\""));

    let (st, done) = b
        .admin(
            Method::POST,
            "/v1/marketplace/install",
            Some(json!({"registry": "team", "name": "support-kit", "version": 1})),
        )
        .await;
    assert_eq!(st, 200, "{done}");
    let installed = done["installed"].to_string();
    assert!(
        installed.contains("agent/support") && installed.contains("prompt/tone"),
        "{installed}"
    );
    let (st, _) = b
        .admin(
            Method::POST,
            "/v1/runs",
            Some(json!({"agent": "support", "input": "hi"})),
        )
        .await;
    assert_eq!(st, 200, "the installed agent finds its prompt");

    // Rating through B lands in A, as A's member key.
    let (st, _) = b
        .call(
            Method::POST,
            "/v1/marketplace/browse/team/support-kit/ratings",
            &b.member.clone(),
            Some(json!({"stars": 2})),
        )
        .await;
    assert_eq!(st, 200);
    let (_, d) = a
        .admin(Method::GET, "/v1/marketplace/listings/support-kit", None)
        .await;
    assert_eq!(d["listing"]["ratings"], 1, "one rating per rater: {d}");
    assert_eq!(d["listing"]["rating"], 2.0);

    let (st, _) = b
        .admin(Method::GET, "/v1/marketplace/browse?registry=nope", None)
        .await;
    assert_eq!(st, 404);
    let (st, _) = a
        .admin(Method::DELETE, "/v1/marketplace/listings/support-kit", None)
        .await;
    assert_eq!(st, 204);
    let (st, _) = b
        .admin(
            Method::POST,
            "/v1/marketplace/install",
            Some(json!({"registry": "team", "name": "support-kit"})),
        )
        .await;
    assert_eq!(st, 404);
}
