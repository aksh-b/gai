//! The SDK surface: the builder, agent handles, and tools written with
//! `#[gai::tool]`. Runs offline with the `mock/echo` model, which calls a
//! tool when the input is `call:<tool> <json>`.

use gai::agent::AgentSpec;
use gai::tools::{ToolCtx, ToolPolicy};
use serde_json::json;

/// Add two numbers
/// and say the sum.
#[gai::tool]
async fn add(#[arg(desc = "The first number")] a: f64, b: f64) -> f64 {
    a + b
}

/// Greet someone, politely or not.
#[gai::tool(name = "say_hello", idempotent)]
async fn greet(name: String, #[arg(default)] shout: bool, title: Option<String>) -> String {
    let who = match title {
        Some(t) => format!("{t} {name}"),
        None => name,
    };
    if shout {
        format!("HELLO, {}!", who.to_uppercase())
    } else {
        format!("Hello, {who}.")
    }
}

/// Fail on purpose.
#[gai::tool(policy = "approve")]
async fn broken(ctx: &ToolCtx) -> Result<String, String> {
    Err(format!("nothing works in {}", ctx.agent))
}

#[tokio::test]
async fn tool_macro_builds_schemas_and_calls() {
    let t = add_tool();
    let spec = t.spec();
    assert_eq!(spec.name, "add");
    assert_eq!(spec.description, "Add two numbers and say the sum.");
    let schema = &spec.input_schema;
    assert_eq!(schema["properties"]["a"]["description"], "The first number");
    assert_eq!(schema["required"], json!(["a", "b"]));
    let ctx = ToolCtx::for_workspace(std::env::temp_dir());
    let out = t.call(&ctx, json!({"a": 2, "b": 3.5})).await.unwrap();
    assert_eq!(out.as_text(), "5.5");
    // Unknown and missing arguments are refused.
    assert!(t.call(&ctx, json!({"a": 2, "b": 1, "c": 0})).await.is_err());
    assert!(t.call(&ctx, json!({"a": 2})).await.is_err());

    let g = greet_tool();
    assert_eq!(g.spec().name, "say_hello");
    assert!(g.idempotent());
    assert_eq!(g.spec().input_schema["required"], json!(["name"]));
    let out = g.call(&ctx, json!({"name": "ada"})).await.unwrap();
    assert_eq!(out.as_text(), "Hello, ada.");
    let out = g
        .call(&ctx, json!({"name": "ada", "shout": true, "title": "Dr"}))
        .await
        .unwrap();
    assert_eq!(out.as_text(), "HELLO, DR ADA!");

    let b = broken_tool();
    assert_eq!(b.default_policy(), ToolPolicy::Approve);
    assert!(
        b.spec().input_schema["properties"]
            .as_object()
            .is_none_or(|p| p.is_empty())
    );
    let err = b.call(&ctx, json!({})).await.unwrap_err();
    assert!(err.to_string().contains("nothing works in test"), "{err}");
}

#[tokio::test]
async fn builder_and_agent_handles_run_agents() {
    let dir = tempfile::tempdir().unwrap();
    let gai = gai::Gai::builder()
        .with_config(gai::Config::default())
        .data_dir(dir.path())
        .default_model("mock/echo")
        .tool(add_tool())
        .build()
        .unwrap();

    // An agent written in code, using the macro tool.
    let spec = AgentSpec::new("adder").tools(["add"]);
    let out = gai
        .agent_spec(spec.clone())
        .run(r#"call:add {"a": 40, "b": 2}"#)
        .await
        .unwrap();
    assert!(out.text.contains("42"), "{}", out.text);

    // Streaming: events arrive, then the outcome.
    let mut run = gai.agent_spec(spec).stream("hello there").await.unwrap();
    let mut kinds = Vec::new();
    while let Some(e) = run.next_event().await {
        kinds.push(
            serde_json::to_value(&e.event).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    let outcome = run.wait().await;
    assert!(outcome.text.contains("hello there"), "{}", outcome.text);
    assert!(
        kinds.iter().any(|k| k == "run_finished" || k == "finished"),
        "{kinds:?}"
    );

    // A stored agent by name, continuing a session.
    let rt = gai.runtime().await.unwrap();
    rt.save_agent(&AgentSpec::new("echoer").system("Echo."), None)
        .await
        .unwrap();
    let first = gai.agent("echoer").run("one").await.unwrap();
    let second = gai
        .agent("echoer")
        .session(&first.session_id)
        .run("two")
        .await
        .unwrap();
    assert_eq!(first.session_id, second.session_id);
    assert_eq!(
        gai.agent("echoer")
            .model("mock/other")
            .spec()
            .await
            .unwrap()
            .model
            .as_deref(),
        Some("mock/other")
    );
    assert!(gai.agent("missing").run("x").await.is_err());
}
