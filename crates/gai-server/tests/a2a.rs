//! A2A: gai serving its agents, and gai calling them back as a remote.

use std::sync::Arc;

use gai::agent::{AgentSpec, DelegateRef, RemoteDelegate, RunRequest};
use gai::store::RunStatus;
use gai::tools::ToolCtx;
use gai::tools::a2a::A2aAgentConfig;
use gai::{AuthMode, Config, Gai};
use gai_server::{AuthPolicy, router};
use serde_json::{Value, json};

async fn serve(dir: &tempfile::TempDir) -> String {
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
    gai.runtime()
        .await
        .unwrap()
        .save_agent(
            &AgentSpec {
                description: Some("Repeats things and does sums.".into()),
                ..AgentSpec::new("helper")
                    .model("mock/echo")
                    .tools(["calculator"])
            },
            None,
        )
        .await
        .unwrap();
    let app = router(gai, AuthPolicy::new(AuthMode::Off, true, 0), None)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

async fn rpc(url: &str, method: &str, params: Value) -> Value {
    reqwest::Client::new()
        .post(url)
        .json(&json!({"jsonrpc": "2.0", "id": 7, "method": method, "params": params}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

fn message(text: &str) -> Value {
    json!({"message": {"kind": "message", "role": "user", "messageId": "m1", "parts": [{"kind": "text", "text": text}]}})
}

#[tokio::test]
async fn cards_and_json_rpc() {
    let dir = tempfile::tempdir().unwrap();
    let base = serve(&dir).await;
    let http = reqwest::Client::new();

    let card: Value = http
        .get(format!("{base}/.well-known/agent-card.json"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(card["url"], format!("{base}/a2a"));
    assert_eq!(card["skills"][0]["id"], "helper");
    let card: Value = http
        .get(format!("{base}/a2a/helper/.well-known/agent-card.json"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(card["url"], format!("{base}/a2a/helper"));
    assert_eq!(card["description"], "Repeats things and does sums.");
    assert_eq!(
        http.get(format!("{base}/a2a/nobody/.well-known/agent-card.json"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );

    let endpoint = format!("{base}/a2a/helper");
    let r = rpc(
        &endpoint,
        "message/send",
        message("call:calculator {\"expression\": \"6 * 7\"}"),
    )
    .await;
    let task = &r["result"];
    assert_eq!(task["kind"], "task");
    assert_eq!(task["status"]["state"], "completed", "{r}");
    assert_eq!(
        task["artifacts"][0]["parts"][0]["text"],
        "The tool returned: 42"
    );
    let (task_id, context) = (task["id"].clone(), task["contextId"].clone());

    // The same context continues the conversation.
    let mut follow = message("and again");
    follow["message"]["contextId"] = context.clone();
    let r = rpc(&endpoint, "message/send", follow).await;
    assert_eq!(r["result"]["contextId"], context);

    let got = rpc(&endpoint, "tasks/get", json!({"id": task_id})).await;
    assert_eq!(got["result"]["id"], task_id);
    assert_eq!(
        rpc(&endpoint, "tasks/get", json!({"id": "run_nope"})).await["error"]["code"],
        -32001
    );
    assert_eq!(
        rpc(&endpoint, "tasks/cancel", json!({"id": task_id})).await["error"]["code"],
        -32002
    );
    assert_eq!(
        rpc(&endpoint, "nope/nope", json!({})).await["error"]["code"],
        -32601
    );
    // The catalog endpoint needs the agent named.
    let r = rpc(&format!("{base}/a2a"), "message/send", message("hi")).await;
    assert_eq!(r["error"]["code"], -32602);
    let mut named = message("hi there");
    named["message"]["metadata"] = json!({"agent": "helper"});
    let r = rpc(&format!("{base}/a2a"), "message/send", named).await;
    assert_eq!(r["result"]["status"]["state"], "completed");

    // Streaming: the task first, the answer streamed, a final status last.
    let body = http
        .post(&endpoint)
        .json(&json!({"jsonrpc": "2.0", "id": 9, "method": "message/stream", "params": message("streamed reply")}))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let events: Vec<Value> = body
        .lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .map(|d| serde_json::from_str(d.trim()).unwrap())
        .collect();
    assert_eq!(events[0]["result"]["kind"], "task");
    let streamed: String = events
        .iter()
        .filter(|e| e["result"]["kind"] == "artifact-update" && e["result"]["lastChunk"] == false)
        .map(|e| {
            e["result"]["artifact"]["parts"][0]["text"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(streamed, "streamed reply");
    let last = &events.last().unwrap()["result"];
    assert_eq!(
        (last["kind"].as_str(), last["final"].as_bool()),
        (Some("status-update"), Some(true))
    );
    assert_eq!(last["status"]["state"], "completed");
}

#[tokio::test]
async fn gai_calls_a_remote_gai_agent() {
    let remote_dir = tempfile::tempdir().unwrap();
    let base = serve(&remote_dir).await;

    let dir = tempfile::tempdir().unwrap();
    let mut config = Config {
        data_dir: Some(dir.path().to_path_buf()),
        default_model: Some("mock/echo".into()),
        ..Default::default()
    };
    config.a2a_agents.insert(
        "far".into(),
        A2aAgentConfig {
            // The card URL; the endpoint comes from the card.
            url: format!("{base}/a2a/helper/.well-known/agent-card.json"),
            ..Default::default()
        },
    );
    let local = Gai::from_config(config, None).unwrap();
    let rt = local.runtime().await.unwrap();

    // Declared remotes are tools.
    let tool = rt.tool("a2a__far").expect("a2a__far is registered");
    let out = tool
        .call(
            &ToolCtx::for_workspace(dir.path()),
            json!({"task": "call:calculator {\"expression\": \"2 + 3\"}"}),
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{out:?}");
    assert!(
        out.as_text().starts_with("The tool returned: 5"),
        "{}",
        out.as_text()
    );
    assert!(out.as_text().contains("context_id"));

    // As a delegate inside an agent. Private addresses need permission...
    let boss = AgentSpec {
        delegates: vec![DelegateRef::Remote(RemoteDelegate {
            a2a: format!("{base}/a2a/helper"),
            name: "far".into(),
            description: None,
            headers: Default::default(),
        })],
        ..AgentSpec::new("boss").model("mock/echo")
    };
    let err = rt
        .start(RunRequest::new(boss.clone(), "x"))
        .await
        .err()
        .expect("refused")
        .to_string();
    assert!(err.contains("private network"), "{err}");
    // ...which a local config can give.
    let mut config = Config {
        data_dir: Some(dir.path().join("b")),
        default_model: Some("mock/echo".into()),
        ..Default::default()
    };
    config.tools.allow_private_network = true;
    let local = Gai::from_config(config, None).unwrap();
    let out = local
        .runtime()
        .await
        .unwrap()
        .run(RunRequest::new(
            boss,
            "call:ask_far {\"task\": \"hello from far away\"}",
        ))
        .await
        .unwrap();
    assert_eq!(out.status, RunStatus::Completed, "{:?}", out.error);
    assert!(out.text.contains("hello from far away"), "{}", out.text);
}
