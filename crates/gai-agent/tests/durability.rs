//! Runs that outlive their process, hooks, and MCP registration.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use gai_agent::hooks::{DenyShell, RedactSecrets};
use gai_agent::{
    AgentSpec, Blocked, Decision, HookCtx, RunEvent, RunHook, RunRequest, Runtime, RuntimeConfig,
};
use gai_core::{ContentBlock, Message, Role};
use gai_llm::{
    ChatRequest, ChatResponse, LlmConfig, LlmError, MockModel, ModelInfo, ModelRegistry, Provider,
    SharedChatModel, text_response,
};
use gai_store::{NewRun, NewSession, RunStatus, SharedStore, SqliteStore, StepKind, StepStatus};
use gai_tools::mcp::{McpServer, McpServerConfig, McpStatus};
use gai_tools::{ToolOutput, ToolPolicy, ToolRegistry, typed_tool};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

struct Scripted(Arc<MockModel>);

#[async_trait]
impl Provider for Scripted {
    fn name(&self) -> &str {
        "script"
    }
    fn chat(&self, _info: ModelInfo) -> Result<SharedChatModel, LlmError> {
        Ok(self.0.clone())
    }
}

#[derive(Deserialize, JsonSchema)]
struct Target {
    target: String,
}

/// A runtime over `store`. Each call is like a fresh process.
fn runtime(store: SharedStore, dir: &std::path::Path, script: Option<Arc<MockModel>>) -> Runtime {
    let mut b = ModelRegistry::builder().config(LlmConfig::default());
    if let Some(m) = script {
        b = b.provider("script", Arc::new(Scripted(m)));
    }
    let mut tools = ToolRegistry::with_builtins();
    tools.register(Arc::new(
        typed_tool("danger", "Dangerous", |_c, a: Target| async move {
            Ok(ToolOutput::text(format!("destroyed {}", a.target)))
        })
        .policy(ToolPolicy::Approve),
    ));
    tools.register(Arc::new(typed_tool(
        "send_email",
        "Not idempotent",
        |_c, a: Target| async move { Ok(ToolOutput::text(format!("sent to {}", a.target))) },
    )));
    let config = RuntimeConfig {
        default_model: Some("mock/echo".into()),
        data_dir: dir.to_path_buf(),
        ..Default::default()
    };
    Runtime::new(Arc::new(b.build().unwrap()), tools, store, config)
}

async fn first_approval(h: &mut gai_agent::RunHandle) -> String {
    loop {
        if let RunEvent::ApprovalRequired { id, .. } =
            h.next_event().await.expect("run still going").event
        {
            return id;
        }
    }
}

#[tokio::test]
async fn a_pending_approval_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let store: SharedStore = Arc::new(SqliteStore::temporary().await.unwrap());
    let agent = AgentSpec::new("ops").model("mock/echo").tools(["danger"]);

    // Process 1 asks for approval, then dies.
    let rt1 = runtime(store.clone(), dir.path(), None);
    let mut h = rt1
        .start(RunRequest::new(agent, r#"call:danger {"target": "prod"}"#))
        .await
        .unwrap();
    first_approval(&mut h).await;
    let run_id = h.run_id.clone();
    h.abort();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        store.get_run(&run_id).await.unwrap().unwrap().status,
        RunStatus::AwaitingApproval
    );

    // Process 2 recovers it, asks again, and finishes once approved.
    let rt2 = runtime(store.clone(), dir.path(), None);
    assert_eq!(rt2.recover().await, vec![run_id.clone()]);
    let mut events = rt2.subscribe(&run_id).unwrap();
    let pending = loop {
        let p = rt2.approvals().pending(Some(&run_id));
        if !p.is_empty() {
            break p;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(pending[0].tool, "danger");
    rt2.resolve_approval(&run_id, &pending[0].call_id, Decision::approve())
        .unwrap();
    loop {
        if let RunEvent::RunFinished { status, .. } = events.recv().await.unwrap().event {
            assert_eq!(status, RunStatus::Completed);
            break;
        }
    }
    let run = store.get_run(&run_id).await.unwrap().unwrap();
    assert_eq!(run.output, Some(json!("The tool returned: destroyed prod")));
    // Event numbering carried on from before the restart.
    let seqs: Vec<u64> = store
        .list_events(&run_id, None, 1000)
        .await
        .unwrap()
        .iter()
        .map(|e| e.seq)
        .collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{seqs:?}");
    assert!(rt2.recover().await.is_empty(), "nothing left to recover");
}

/// Build a run that stopped right after the model asked for `calls`.
async fn interrupted_run(
    store: &SharedStore,
    spec: &AgentSpec,
    calls: Vec<(&str, &str, serde_json::Value)>,
) -> String {
    let ses = store.create_session(NewSession::default()).await.unwrap();
    let assistant = Message {
        role: Role::Assistant,
        content: calls
            .iter()
            .map(|(id, name, input)| ContentBlock::ToolUse {
                id: (*id).into(),
                name: (*name).into(),
                input: input.clone(),
            })
            .collect(),
    };
    store
        .append_messages(&ses.id, None, &[Message::user("go"), assistant])
        .await
        .unwrap();
    let run = store
        .create_run(NewRun {
            session_id: ses.id.clone(),
            agent: spec.name.clone(),
            input: json!([{"type": "text", "text": "go"}]),
            metadata: json!({"spec": spec, "model": "script/x"}),
            ..Default::default()
        })
        .await
        .unwrap();
    store
        .update_run(
            &run.id,
            gai_store::RunUpdate {
                status: Some(RunStatus::Running),
                steps: Some(1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    run.id
}

#[tokio::test]
async fn finished_tool_calls_are_not_repeated_and_unsafe_ones_need_approval() {
    let dir = tempfile::tempdir().unwrap();
    let store: SharedStore = Arc::new(SqliteStore::temporary().await.unwrap());
    let mock = Arc::new(MockModel::scripted(vec![text_response("all done")]));
    let spec = AgentSpec::new("worker")
        .model("script/x")
        .tools(["calculator", "send_email"]);
    let run_id = interrupted_run(
        &store,
        &spec,
        vec![
            ("c1", "calculator", json!({"expression": "1+1"})),
            ("c2", "send_email", json!({"target": "bob"})),
        ],
    )
    .await;
    // c1 finished before the crash; the journal has its (deliberately odd) result.
    store
        .start_step(
            &run_id,
            1,
            StepKind::Tool,
            Some("calculator"),
            Some(&json!({"call_id": "c1", "input": {}})),
        )
        .await
        .unwrap();
    let done = serde_json::to_value(ToolOutput::text("99 (from the journal)")).unwrap();
    store
        .finish_step(&run_id, 1, StepStatus::Completed, Some(&done), None, None)
        .await
        .unwrap();

    let rt = runtime(store.clone(), dir.path(), Some(mock.clone()));
    let mut h = rt.resume(&run_id).await.unwrap();
    // send_email is not idempotent and was in flight: it must be approved again.
    let call = first_approval(&mut h).await;
    assert_eq!(call, "c2");
    rt.resolve_approval(&run_id, &call, Decision::approve())
        .unwrap();
    let out = h.wait().await;
    assert_eq!(out.status, RunStatus::Completed, "{:?}", out.error);
    assert_eq!(out.text, "all done");

    let reqs = mock.requests();
    let sent = &reqs.lock().unwrap()[0].messages;
    let results: Vec<String> = sent
        .last()
        .unwrap()
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolResult { content, .. } => {
                Some(content.iter().filter_map(|c| c.as_text()).collect())
            }
            _ => None,
        })
        .collect();
    assert_eq!(results, ["99 (from the journal)", "sent to bob"]);
    assert_eq!(
        out.steps, 2,
        "step count continues from the interrupted run"
    );
}

#[tokio::test]
async fn ended_runs_cannot_resume() {
    let dir = tempfile::tempdir().unwrap();
    let store: SharedStore = Arc::new(SqliteStore::temporary().await.unwrap());
    let rt = runtime(store.clone(), dir.path(), None);
    let out = rt
        .run(RunRequest::new(
            AgentSpec::new("a").model("mock/echo"),
            "hi",
        ))
        .await
        .unwrap();
    assert!(rt.resume(&out.run_id).await.is_err());
}

struct NoSecretsPlease;

#[async_trait]
impl RunHook for NoSecretsPlease {
    fn name(&self) -> &str {
        "policy"
    }
    async fn before_model(&self, _ctx: &HookCtx, req: &mut ChatRequest) -> Result<(), Blocked> {
        let last = req.messages.last().map(Message::text).unwrap_or_default();
        if last.contains("forbidden") {
            return Err(Blocked("this topic is off limits".into()));
        }
        req.system = Some(format!(
            "{} [checked]",
            req.system.clone().unwrap_or_default()
        ));
        Ok(())
    }
    async fn after_model(&self, _ctx: &HookCtx, resp: &mut ChatResponse) -> Result<(), Blocked> {
        for b in &mut resp.content {
            if let ContentBlock::Text { text } = b {
                *text = text.replace("darn", "d**n");
            }
        }
        Ok(())
    }
}

#[tokio::test]
async fn hooks_guard_model_and_tool_calls() {
    let dir = tempfile::tempdir().unwrap();
    let store: SharedStore = Arc::new(SqliteStore::temporary().await.unwrap());
    let rt = runtime(store.clone(), dir.path(), None);
    rt.add_hook(Arc::new(NoSecretsPlease));
    rt.add_hook(Arc::new(RedactSecrets::new(&[]).unwrap()));
    rt.add_hook(Arc::new(DenyShell::new(&[r"rm\s+-rf".into()]).unwrap()));
    // The built-in safety hook comes first.
    assert_eq!(
        rt.hook_names(),
        ["safety", "policy", "redact_secrets", "deny_shell"]
    );
    let agent = AgentSpec::new("h")
        .model("mock/echo")
        .tools(["shell", "fs_write", "fs_read"]);

    let blocked = rt
        .run(RunRequest::new(
            agent.clone(),
            "tell me the forbidden thing",
        ))
        .await
        .unwrap();
    assert_eq!(blocked.status, RunStatus::Failed);
    assert!(blocked.error.unwrap().contains("off limits"));

    let edited = rt
        .run(RunRequest::new(agent.clone(), "oh darn"))
        .await
        .unwrap();
    assert_eq!(edited.text, "oh d**n");

    let mut req = RunRequest::new(agent.clone(), r#"call:shell {"command": "rm -rf /tmp/x"}"#);
    req.auto_approve = true;
    let out = rt.run(req).await.unwrap();
    assert!(
        out.text.contains("Blocked by the deny_shell hook"),
        "{}",
        out.text
    );

    let mut req = RunRequest::new(
        agent.clone(),
        r#"call:fs_write {"path": "k.txt", "content": "token=ghp_abcdefghijklmnopqrstuvwxyz0123456789"}"#,
    );
    req.session_id = None;
    let w = rt.run(req).await.unwrap();
    let read = rt
        .run(RunRequest::new(agent, r#"call:fs_read {"path": "k.txt"}"#).session(w.session_id))
        .await
        .unwrap();
    assert!(
        read.text.contains("[REDACTED]") && !read.text.contains("ghp_"),
        "{}",
        read.text
    );
}

#[tokio::test]
async fn mcp_servers_register_and_unregister_tools() {
    let dir = tempfile::tempdir().unwrap();
    let store: SharedStore = Arc::new(SqliteStore::temporary().await.unwrap());
    let rt = runtime(store, dir.path(), None);
    let broken = McpServer::new(
        "broken",
        McpServerConfig {
            command: Some("/no/such/server".into()),
            ..Default::default()
        },
        "test",
    );
    assert!(rt.connect_mcp(broken).await.is_err());
    let info = rt.mcp_servers();
    assert_eq!(info.len(), 1);
    assert!(matches!(info[0].status, McpStatus::Failed(_)));
    assert!(
        rt.tool_specs()
            .iter()
            .all(|(s, _)| !s.name.starts_with("mcp__"))
    );
    rt.mcp().mark_settled();
    // An agent naming tools of a dead server fails clearly.
    let agent = AgentSpec::new("m")
        .model("mock/echo")
        .tools(["mcp__broken__*"]);
    let err = rt.start(RunRequest::new(agent, "x")).await.err().unwrap();
    assert!(err.to_string().contains("mcp__broken__*"), "{err}");
    assert!(rt.remove_mcp("broken").await);
    assert!(rt.mcp_servers().is_empty());
}
