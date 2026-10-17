//! gai as an MCP server, driven by gai's own MCP client and by rmcp's.

use std::sync::Arc;

use gai::agent::AgentSpec;
use gai::memory::{DocumentInput, NewCollection};
use gai::orchestra::WorkflowSpec;
use gai::tools::ToolCtx;
use gai::tools::mcp::{McpServer, McpServerConfig};
use gai::{AuthMode, Config, Gai};
use gai_server::{AuthPolicy, router};
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, ReadResourceRequestParams, ResourceContents};
use serde_json::json;

async fn server(config: Config) -> (String, Arc<Gai>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        data_dir: Some(dir.path().to_path_buf()),
        default_model: Some("mock/echo".into()),
        agents_dirs: vec![dir.path().join("agents")],
        workflows_dirs: vec![dir.path().join("workflows")],
        ..config
    };
    let gai = Arc::new(Gai::from_config(config, None).unwrap());
    let rt = gai.runtime().await.unwrap();
    rt.save_agent(
        &AgentSpec::new("helper")
            .model("mock/echo")
            .tools(["calculator"])
            .system("You help."),
        None,
    )
    .await
    .unwrap();
    rt.save_agent(&AgentSpec::new("secret").model("mock/echo"), None)
        .await
        .unwrap();
    let wf = gai.workflows().await.unwrap();
    wf.save(
        &WorkflowSpec::parse(
            "name: greet\ndescription: Says hello.\ninputs:\n  who: {description: Whom to greet}\nnodes:\n  - id: hi\n    template: \"Hello, {{ inputs.who }}!\"\n",
            "yaml",
        )
        .unwrap(),
        None,
    )
    .await
    .unwrap();
    let k = rt.knowledge();
    k.create_collection(NewCollection {
        name: "handbook".into(),
        description: Some("Team handbook".into()),
        ..Default::default()
    })
    .await
    .unwrap();
    k.ingest(
        "handbook",
        DocumentInput {
            source: "leave.md".into(),
            title: Some("Leave policy".into()),
            text: "Everyone gets 25 days of paid leave per year.".into(),
            metadata: json!({}),
        },
    )
    .await
    .unwrap();

    let app = router(gai.clone(), AuthPolicy::new(AuthMode::Off, true, 0), None)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://localhost:{}/mcp",
        listener.local_addr().unwrap().port()
    );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, gai, dir)
}

#[tokio::test]
async fn gai_agents_and_workflows_are_mcp_tools() {
    let mut cfg = Config::default();
    cfg.server.mcp.agents = vec!["help*".into()];
    let (url, gai, _dir) = server(cfg).await;

    // gai's own MCP client talking to gai.
    let client = McpServer::new(
        "self",
        McpServerConfig {
            url: Some(url.clone()),
            ..Default::default()
        },
        "test",
    );
    let tools = client.connect().await.expect("connects");
    let mut names: Vec<String> = tools.iter().map(|t| t.spec().name).collect();
    names.sort();
    // Only agents matching `help*`; the workflow; knowledge search.
    assert_eq!(
        names,
        [
            "mcp__self__agent_helper",
            "mcp__self__search_knowledge",
            "mcp__self__workflow_greet"
        ]
    );
    let by = |n: &str| {
        tools
            .iter()
            .find(|t| t.spec().name.ends_with(n))
            .unwrap()
            .clone()
    };
    let ctx = ToolCtx::for_workspace(std::env::temp_dir());

    let out = by("agent_helper")
        .call(
            &ctx,
            json!({"task": "call:calculator {\"expression\": \"6 * 7\"}"}),
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{out:?}");
    assert_eq!(out.as_text(), "The tool returned: 42");
    // The run is recorded as coming from MCP.
    let store = gai.runtime().await.unwrap().store().clone();
    let runs = store
        .list_runs(gai::store::RunFilter {
            agent: Some("helper".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(runs[0].metadata["source"], "mcp");

    let out = by("workflow_greet")
        .call(&ctx, json!({"who": "Ada"}))
        .await
        .unwrap();
    assert!(!out.is_error, "{out:?}");
    assert!(out.as_text().contains("Hello, Ada!"), "{}", out.as_text());

    let out = by("search_knowledge")
        .call(&ctx, json!({"query": "paid leave days"}))
        .await
        .unwrap();
    assert!(out.as_text().contains("25 days"), "{}", out.as_text());
}

#[tokio::test]
async fn collections_are_resources_and_errors_are_clear() {
    let (url, _gai, _dir) = server(Config::default()).await;
    let transport = rmcp::transport::StreamableHttpClientTransport::from_uri(url.as_str());
    let client = ().serve(transport).await.expect("connects");

    let resources = client.list_all_resources().await.unwrap();
    let uris: Vec<&str> = resources.iter().map(|r| r.uri.as_str()).collect();
    assert!(uris.contains(&"gai://collections/handbook"), "{uris:?}");
    let doc = resources
        .iter()
        .find(|r| r.uri.contains("/documents/"))
        .expect("a document resource");
    assert_eq!(doc.name, "Leave policy");

    let read = client
        .read_resource(ReadResourceRequestParams::new(doc.uri.clone()))
        .await
        .unwrap();
    let text = match &read.contents[0] {
        ResourceContents::TextResourceContents { text, .. } => text.clone(),
        other => panic!("{other:?}"),
    };
    assert!(text.contains("25 days"));
    let listing = client
        .read_resource(ReadResourceRequestParams::new("gai://collections/handbook"))
        .await
        .unwrap();
    assert!(format!("{listing:?}").contains("Leave policy"));
    assert!(
        client
            .read_resource(ReadResourceRequestParams::new("gai://collections/nope"))
            .await
            .is_err()
    );

    // Every stored agent is offered by default.
    let tools = client.list_all_tools().await.unwrap();
    assert!(tools.iter().any(|t| t.name == "agent_secret"));
    let r = client
        .call_tool(CallToolRequestParams::new("agent_helper"))
        .await;
    assert!(r.is_err(), "a missing task is an error");
    let r = client
        .call_tool(
            CallToolRequestParams::new("agent_helper")
                .with_arguments(json!({"task": "hello"}).as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    let structured = r.structured_content.unwrap();
    assert_eq!(structured["status"], "completed");
    assert!(
        structured["session_id"]
            .as_str()
            .unwrap()
            .starts_with("ses_")
    );
    client.cancel().await.ok();
}
