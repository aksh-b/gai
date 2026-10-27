//! Apps over HTTP: builders publish an assistant or workflow as a form and
//! an endpoint; people use it with the app's own key, or with nothing when
//! it is public.

use std::sync::Arc;
use std::time::Duration;

use gai::store::{NewKey, Scope, keys};
use gai::{AuthMode, Config, Gai};
use gai_server::{AuthPolicy, router};
use reqwest::Method;
use serde_json::{Value, json};

struct Server {
    url: String,
    admin: String,
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
    let k = keys::generate();
    store
        .create_key(&NewKey::new(
            "admin",
            &k.prefix,
            &k.hash,
            &[Scope::Admin],
            None,
        ))
        .await
        .unwrap();
    let app = router(gai, AuthPolicy::new(AuthMode::Required, true, 0), None)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server {
        url,
        admin: k.secret,
        _dir: dir,
    }
}

impl Server {
    async fn call(
        &self,
        m: Method,
        path: &str,
        key: Option<&str>,
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut rb = reqwest::Client::new().request(m, format!("{}{path}", self.url));
        if let Some(k) = key {
            rb = rb.bearer_auth(k);
        }
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
        self.call(m, path, Some(&self.admin), body).await
    }
}

#[tokio::test]
async fn an_assistant_app_with_its_own_key() {
    let s = server().await;
    let (st, _) = s
        .admin(
            Method::POST,
            "/v1/agents",
            Some(json!({"name": "echoer", "model": "mock/echo"})),
        )
        .await;
    assert_eq!(st, 201);
    let spec = json!({
        "name": "scorer",
        "title": "Score a thing",
        "agent": "echoer",
        "fields": [
            {"name": "thing", "label": "What to score"},
            {"name": "n", "type": "number"},
        ],
        // The echo model answers with the task, so this is its JSON answer.
        "prompt": "{\"thing\": \"{{ thing }}\", \"score\": {{ n }}}",
        "output_schema": {
            "type": "object",
            "required": ["score"],
            "properties": {"score": {"type": "number"}, "thing": {"type": "string"}},
        },
    });
    let (st, saved) = s.admin(Method::POST, "/v1/apps", Some(spec)).await;
    assert_eq!(st, 201, "{saved}");
    assert_eq!(saved["key_prefix"], Value::Null);

    // Without a key, a private app is closed.
    let (st, _) = s
        .call(Method::GET, "/v1/apps/scorer/form", None, None)
        .await;
    assert_eq!(st, 401);
    let (st, key) = s.admin(Method::POST, "/v1/apps/scorer/key", None).await;
    assert_eq!(st, 200);
    let key = key["secret"].as_str().unwrap().to_owned();
    assert!(key.starts_with("gai_app_"));

    let (st, form) = s
        .call(Method::GET, "/v1/apps/scorer/form", Some(&key), None)
        .await;
    assert_eq!(st, 200, "{form}");
    assert_eq!(form["title"], "Score a thing");
    assert_eq!(form["structured"], true);
    assert_eq!(form["fields"][1]["type"], "number");
    assert!(
        form.get("agent").is_none(),
        "the page doesn't learn what runs"
    );

    let (st, r) = s
        .call(
            Method::POST,
            "/v1/apps/scorer/run",
            Some(&key),
            Some(json!({"thing": "tea", "n": "7"})),
        )
        .await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["status"], "completed", "{r}");
    assert_eq!(r["output"], json!({"thing": "tea", "score": 7.0}));

    // Inputs are checked against the fields.
    let (st, r) = s
        .call(
            Method::POST,
            "/v1/apps/scorer/run",
            Some(&key),
            Some(json!({"thing": "tea"})),
        )
        .await;
    assert_eq!(
        (st, r["error"]["message"].as_str()),
        (400, Some("n is required"))
    );
    let (st, _) = s
        .call(
            Method::POST,
            "/v1/apps/scorer/run",
            Some(&key),
            Some(json!({"thing": "tea", "n": 1, "extra": 2})),
        )
        .await;
    assert_eq!(st, 400);

    // In the background, then polled.
    let (st, started) = s
        .call(
            Method::POST,
            "/v1/apps/scorer/run?mode=background",
            Some(&key),
            Some(json!({"thing": "jam", "n": 2})),
        )
        .await;
    assert_eq!(st, 202, "{started}");
    let id = started["run_id"].as_str().unwrap();
    let mut done = Value::Null;
    for _ in 0..100 {
        let (_, r) = s
            .call(
                Method::GET,
                &format!("/v1/apps/scorer/runs/{id}"),
                Some(&key),
                None,
            )
            .await;
        if r["done"] == true {
            done = r;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(done["output"]["score"], 2.0, "{done}");

    // The app's key runs this app and nothing else.
    let (st, _) = s.call(Method::GET, "/v1/agents", Some(&key), None).await;
    assert_eq!(st, 401);
    let (st, _) = s
        .call(
            Method::POST,
            "/v1/apps",
            Some(&key),
            Some(json!({"name": "x", "agent": "echoer"})),
        )
        .await;
    assert_eq!(st, 401);

    // Its own OpenAPI document describes the input and output.
    let (st, doc) = s
        .call(
            Method::GET,
            "/v1/apps/scorer/openapi.json",
            Some(&key),
            None,
        )
        .await;
    assert_eq!(st, 200);
    let post = &doc["paths"]["/v1/apps/scorer/run"]["post"];
    assert_eq!(
        post["requestBody"]["content"]["application/json"]["schema"]["required"],
        json!(["thing", "n"])
    );
    assert_eq!(
        post["responses"]["200"]["content"]["application/json"]["schema"]["properties"]["output"]["required"],
        json!(["score"])
    );

    // A new key replaces the old one; removing it closes the app again.
    let (_, fresh) = s.admin(Method::POST, "/v1/apps/scorer/key", None).await;
    let (st, _) = s
        .call(Method::GET, "/v1/apps/scorer/form", Some(&key), None)
        .await;
    assert_eq!(st, 401);
    let fresh = fresh["secret"].as_str().unwrap();
    let (st, _) = s
        .call(Method::GET, "/v1/apps/scorer/form", Some(fresh), None)
        .await;
    assert_eq!(st, 200);
    let (st, _) = s.admin(Method::DELETE, "/v1/apps/scorer/key", None).await;
    assert_eq!(st, 204);
    let (st, _) = s
        .call(Method::GET, "/v1/apps/scorer/form", Some(fresh), None)
        .await;
    assert_eq!(st, 401);

    // Signed-in callers use it with their own key.
    let (st, _) = s.admin(Method::GET, "/v1/apps/scorer/form", None).await;
    assert_eq!(st, 200);
}

#[tokio::test]
async fn a_public_workflow_app() {
    let s = server().await;
    let wf = json!({
        "name": "greet",
        "inputs": {
            "who": {"description": "Who to greet"},
            "greeting": {"default": "Hello"},
            "also": {"default": ["Grace"]},
        },
        "nodes": [{"id": "say", "template": "{{ inputs.greeting }}, {{ inputs.who }}{% for a in inputs.also %} and {{ a }}{% endfor %}!"}],
    });
    let (st, _) = s.admin(Method::POST, "/v1/workflows", Some(wf)).await;
    assert_eq!(st, 201);

    // Fields must be inputs of the workflow, and cover the required ones.
    let (st, e) = s
        .admin(
            Method::POST,
            "/v1/apps",
            Some(json!({"name": "hi", "workflow": "greet", "fields": [{"name": "nope"}]})),
        )
        .await;
    assert_eq!(st, 400);
    assert!(e["error"]["message"].as_str().unwrap().contains("no input"));
    let (st, _) = s
        .admin(
            Method::POST,
            "/v1/apps",
            Some(json!({"name": "hi", "workflow": "greet", "fields": [{"name": "greeting"}]})),
        )
        .await;
    assert_eq!(st, 400, "who is required by the workflow");
    let (st, _) = s
        .admin(
            Method::POST,
            "/v1/apps",
            Some(json!({"name": "hi", "workflow": "missing"})),
        )
        .await;
    assert_eq!(st, 404);

    let (st, saved) = s
        .admin(
            Method::POST,
            "/v1/apps",
            Some(json!({"name": "hi", "workflow": "greet", "public": true})),
        )
        .await;
    assert_eq!(st, 201, "{saved}");

    // Without fields of its own, the form asks for the workflow's inputs.
    let (st, form) = s.call(Method::GET, "/v1/apps/hi/form", None, None).await;
    assert_eq!(st, 200, "{form}");
    let names: Vec<&str> = form["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["also", "greeting", "who"]);
    assert_eq!(form["fields"][0]["type"], "list");
    assert_eq!(form["fields"][1]["required"], false);
    assert_eq!(form["fields"][2]["help"], "Who to greet");

    let (st, r) = s
        .call(
            Method::POST,
            "/v1/apps/hi/run",
            None,
            Some(json!({"who": "Ada"})),
        )
        .await;
    assert_eq!(st, 200, "{r}");
    assert_eq!(r["output"], "Hello, Ada and Grace!");
    let (_, r) = s
        .call(
            Method::POST,
            "/v1/apps/hi/run",
            None,
            Some(json!({"who": "Ada", "also": "Alan\nLinus"})),
        )
        .await;
    assert_eq!(r["output"], "Hello, Ada and Alan and Linus!");

    // A result that doesn't follow the output schema is reported.
    let (st, _) = s
        .admin(
            Method::POST,
            "/v1/apps",
            Some(json!({"name": "hi", "workflow": "greet", "public": true,
                "output_schema": {"type": "object"}})),
        )
        .await;
    assert_eq!(st, 201);
    let (_, r) = s
        .call(
            Method::POST,
            "/v1/apps/hi/run",
            None,
            Some(json!({"who": "Ada"})),
        )
        .await;
    assert_eq!(r["status"], "invalid_output", "{r}");

    // Runs of other apps (or none) aren't found through this one.
    let (st, _) = s
        .call(Method::GET, "/v1/apps/hi/runs/run_nope", None, None)
        .await;
    assert_eq!(st, 404);

    let (st, list) = s.admin(Method::GET, "/v1/apps", None).await;
    assert_eq!((st, list.as_array().unwrap().len()), (200, 1));
    let (st, _) = s.admin(Method::DELETE, "/v1/apps/hi", None).await;
    assert_eq!(st, 204);
    let (st, _) = s.call(Method::GET, "/v1/apps/hi/form", None, None).await;
    assert_eq!(st, 404);
}
