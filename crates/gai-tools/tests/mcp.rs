//! MCP client against real MCP servers, over stdio and streamable HTTP.
//!
//! This test target has no libtest harness: when started with
//! `GAI_MCP_FIXTURE=stdio` the binary *is* the stdio MCP server, so the stdio
//! test can spawn its own executable without libtest output corrupting the
//! protocol.

use std::sync::Arc;

use gai_tools::mcp::{McpServer, McpServerConfig, McpStatus};
use gai_tools::{SharedTool, ToolCtx, ToolPolicy};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerConfig};
use rmcp::{ServerHandler, ServiceExt, schemars, tool, tool_handler, tool_router};
use serde_json::json;

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AddRequest {
    /// Left operand
    a: i64,
    b: i64,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct EchoRequest {
    text: String,
}

#[derive(Clone)]
struct Fixture {
    // Read by the code `#[tool_router]` generates, which dead-code
    // analysis does not see.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Fixture {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Add two integers", annotations(read_only_hint = true))]
    fn add(&self, Parameters(AddRequest { a, b }): Parameters<AddRequest>) -> String {
        (a + b).to_string()
    }

    #[tool(description = "Echo text back")]
    fn echo(&self, Parameters(EchoRequest { text }): Parameters<EchoRequest>) -> String {
        format!("echo: {text}")
    }

    #[tool(description = "Always fails")]
    fn boom(&self) -> Result<String, String> {
        Err("kaboom".into())
    }

    #[tool(
        description = "Pretends to delete everything",
        annotations(destructive_hint = true)
    )]
    fn wipe(&self) -> String {
        "wiped".into()
    }
}

#[tool_handler]
impl ServerHandler for Fixture {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
}

fn main() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    if std::env::var("GAI_MCP_FIXTURE").as_deref() == Ok("stdio") {
        rt.block_on(async {
            let service = Fixture::new()
                .serve(rmcp::transport::stdio())
                .await
                .unwrap();
            service.waiting().await.unwrap();
        });
        return;
    }
    rt.block_on(async {
        stdio_server().await;
        println!("ok - stdio server");
        http_server().await;
        println!("ok - streamable http server");
        unreachable_server().await;
        println!("ok - unreachable server");
    });
}

async fn exercise(tools: Vec<SharedTool>) {
    let by_name = |n: &str| {
        tools
            .iter()
            .find(|t| t.spec().name == n)
            .unwrap_or_else(|| panic!("no tool {n}"))
            .clone()
    };
    let names: Vec<String> = tools.iter().map(|t| t.spec().name).collect();
    assert_eq!(names.len(), 4, "{names:?}");

    let ctx = ToolCtx::for_workspace(".");
    let add = by_name("mcp__fix__add");
    assert_eq!(
        add.spec().input_schema["properties"]["a"]["description"],
        "Left operand"
    );
    assert!(add.idempotent(), "read-only tools are idempotent");
    assert_eq!(add.default_policy(), ToolPolicy::Auto);
    assert_eq!(
        add.call(&ctx, json!({"a": 2, "b": 40}))
            .await
            .unwrap()
            .as_text(),
        "42"
    );
    assert_eq!(
        by_name("mcp__fix__echo")
            .call(&ctx, json!({"text": "hi"}))
            .await
            .unwrap()
            .as_text(),
        "echo: hi"
    );
    let boom = by_name("mcp__fix__boom")
        .call(&ctx, json!({}))
        .await
        .unwrap();
    assert!(boom.is_error);
    assert!(boom.as_text().contains("kaboom"));
    assert_eq!(
        by_name("mcp__fix__wipe").default_policy(),
        ToolPolicy::Approve,
        "destructive tools need approval"
    );
}

async fn stdio_server() {
    let config = McpServerConfig {
        command: Some(std::env::current_exe().unwrap().display().to_string()),
        env: [("GAI_MCP_FIXTURE".to_owned(), "stdio".to_owned())].into(),
        ..Default::default()
    };
    let server = McpServer::new("fix", config, "test");
    let tools = server.connect().await.expect("connects over stdio");
    assert_eq!(server.info().status, McpStatus::Ready);
    exercise(tools.clone()).await;

    // The child dies; the next call reconnects.
    server.close().await;
    let ctx = ToolCtx::for_workspace(".");
    let add = tools
        .iter()
        .find(|t| t.spec().name == "mcp__fix__add")
        .unwrap();
    assert_eq!(
        add.call(&ctx, json!({"a": 1, "b": 1}))
            .await
            .unwrap()
            .as_text(),
        "2"
    );
}

async fn http_server() {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    };
    let service: StreamableHttpService<Fixture, LocalSessionManager> = StreamableHttpService::new(
        || Ok(Fixture::new()),
        Default::default(),
        StreamableHttpServerConfig::default(),
    );
    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let config = McpServerConfig {
        url: Some(format!("http://{addr}/mcp")),
        headers: [(
            "x-trace".to_owned(),
            "${GAI_MCP_TEST_UNSET_VAR}abc".to_owned(),
        )]
        .into(),
        include: vec![],
        ..Default::default()
    };
    let server = McpServer::new("fix", config, "test");
    let tools = server.connect().await.expect("connects over HTTP");
    exercise(tools).await;

    // `include` narrows the exposed tools; `policy` overrides defaults.
    let narrowed = McpServer::new(
        "fix",
        McpServerConfig {
            url: Some(format!("http://{addr}/mcp")),
            include: vec!["add".into()],
            policy: Some(ToolPolicy::Approve),
            ..Default::default()
        },
        "test",
    );
    let tools = narrowed.connect().await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].default_policy(), ToolPolicy::Approve);
}

async fn unreachable_server() {
    let server = McpServer::new(
        "gone",
        McpServerConfig {
            command: Some("/definitely/not/a/binary".into()),
            ..Default::default()
        },
        "test",
    );
    let err = server.connect().await.err().expect("fails");
    assert!(err.contains("could not start"), "{err}");
    assert!(matches!(server.info().status, McpStatus::Failed(_)));
    let _ = Arc::strong_count(&server);
}
