use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use gai_agent::{
    AgentSpec, ContextConfig, Decision, Limits, RunEvent, RunRequest, Runtime, RuntimeConfig,
};
use gai_core::{ContentBlock, Message, Role};
use gai_llm::{
    ChatResponse, LlmConfig, LlmError, MockModel, ModelInfo, ModelRegistry, Provider,
    SharedChatModel, text_response, tool_response,
};
use gai_store::{RunStatus, SqliteStore, StepKind};
use gai_tools::{ToolOutput, ToolPolicy, ToolRegistry, typed_tool};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

/// Serves one shared model under any name, so tests can script it.
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
struct SleepArgs {
    secs: u64,
}

#[derive(Deserialize, JsonSchema)]
struct DangerArgs {
    target: String,
}

async fn runtime_with(
    script: Option<Vec<ChatResponse>>,
) -> (Runtime, Option<Arc<MockModel>>, tempfile::TempDir) {
    let mock = script.map(|s| Arc::new(MockModel::scripted(s)));
    let mut b = ModelRegistry::builder().config(LlmConfig::default());
    if let Some(m) = &mock {
        b = b.provider("script", Arc::new(Scripted(m.clone())));
    }
    let models = Arc::new(b.build().unwrap());
    let mut tools = ToolRegistry::with_builtins();
    tools.register(Arc::new(typed_tool("sleep", "Sleep", |ctx, a: SleepArgs| async move {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(a.secs)) => Ok(ToolOutput::text("slept")),
            _ = ctx.cancel.cancelled() => Ok(ToolOutput::error("cancelled")),
        }
    })));
    tools.register(Arc::new(
        typed_tool("danger", "Dangerous", |_ctx, a: DangerArgs| async move {
            Ok(ToolOutput::text(format!("destroyed {}", a.target)))
        })
        .policy(ToolPolicy::Approve),
    ));
    let store = Arc::new(SqliteStore::temporary().await.unwrap());
    let dir = tempfile::tempdir().unwrap();
    let config = RuntimeConfig {
        default_model: Some("mock/echo".into()),
        data_dir: dir.path().to_path_buf(),
        ..Default::default()
    };
    (Runtime::new(models, tools, store, config), mock, dir)
}

fn agent(model: &str, tools: &[&str]) -> AgentSpec {
    AgentSpec::new("tester")
        .model(model)
        .tools(tools.iter().copied())
        .system("You are {{ agent }}. Today is {{ date }}.")
}

#[tokio::test]
async fn echo_agent_calls_a_tool_and_answers() {
    let (rt, _, _dir) = runtime_with(None).await;
    let mut h = rt
        .start(RunRequest::new(
            agent("mock/echo", &["calculator"]),
            r#"call:calculator {"expression": "6 * 7"}"#,
        ))
        .await
        .unwrap();
    let mut kinds = Vec::new();
    while let Some(e) = h.next_event().await {
        kinds.push(
            serde_json::to_value(&e.event).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    let out = h.wait().await;
    assert_eq!(out.status, RunStatus::Completed, "{:?}", out.error);
    assert_eq!(out.text, "The tool returned: 42");
    assert_eq!(out.steps, 2);
    for k in [
        "run_started",
        "tool_call",
        "tool_result",
        "usage",
        "run_finished",
    ] {
        assert!(kinds.iter().any(|x| x == k), "missing {k} in {kinds:?}");
    }
    assert_eq!(kinds.last().unwrap(), "run_finished");

    let store = rt.store();
    let steps = store.list_steps(&out.run_id).await.unwrap();
    let journal: Vec<StepKind> = steps.iter().map(|s| s.kind).collect();
    assert_eq!(journal, [StepKind::Model, StepKind::Tool, StepKind::Model]);
    let msgs = store.list_messages(&out.session_id).await.unwrap();
    assert_eq!(
        msgs.len(),
        4,
        "user, assistant(tool_use), user(tool_result), assistant"
    );
    let run = store.get_run(&out.run_id).await.unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Completed);
    assert_eq!(run.output, Some(json!("The tool returned: 42")));
    assert!(
        store
            .list_events(&out.run_id, None, 100)
            .await
            .unwrap()
            .len()
            >= 6
    );
}

#[tokio::test]
async fn system_prompt_is_rendered_and_history_carries_over() {
    let (rt, mock, _dir) =
        runtime_with(Some(vec![text_response("first"), text_response("second")])).await;
    let mock = mock.unwrap();
    let a = agent("script/x", &[]);
    let first = rt.run(RunRequest::new(a.clone(), "hello")).await.unwrap();
    let second = rt
        .run(RunRequest::new(a, "again").session(first.session_id.clone()))
        .await
        .unwrap();
    assert_eq!(second.text, "second");
    let reqs = mock.requests();
    let reqs = reqs.lock().unwrap();
    assert!(
        reqs[0]
            .system
            .as_deref()
            .unwrap()
            .starts_with("You are tester. Today is 20")
    );
    let texts: Vec<String> = reqs[1].messages.iter().map(Message::text).collect();
    assert_eq!(texts, ["hello", "first", "again"]);
}

#[tokio::test]
async fn approvals_gate_tools_and_rejections_reach_the_model() {
    let (rt, _, _dir) = runtime_with(None).await;
    let a = agent("mock/echo", &["danger"]);

    // Rejected: the model is told, and the tool never runs.
    let mut h = rt
        .start(RunRequest::new(
            a.clone(),
            r#"call:danger {"target": "prod"}"#,
        ))
        .await
        .unwrap();
    let call_id = loop {
        if let RunEvent::ApprovalRequired { id, .. } = h.next_event().await.unwrap().event {
            break id;
        }
    };
    assert_eq!(
        rt.store().get_run(&h.run_id).await.unwrap().unwrap().status,
        RunStatus::AwaitingApproval
    );
    assert_eq!(rt.approvals().pending(Some(&h.run_id)).len(), 1);
    rt.resolve_approval(&h.run_id, &call_id, Decision::reject("not today"))
        .unwrap();
    let out = h.wait().await;
    assert_eq!(
        out.text,
        "The tool returned: error: The user declined this danger call: not today"
    );

    // Approved with an edited input.
    let mut h = rt
        .start(RunRequest::new(a, r#"call:danger {"target": "prod"}"#))
        .await
        .unwrap();
    let call_id = loop {
        if let RunEvent::ApprovalRequired { id, .. } = h.next_event().await.unwrap().event {
            break id;
        }
    };
    rt.resolve_approval(
        &h.run_id,
        &call_id,
        Decision {
            approved: true,
            reason: None,
            input: Some(json!({"target": "staging"})),
        },
    )
    .unwrap();
    assert_eq!(h.wait().await.text, "The tool returned: destroyed staging");
}

#[tokio::test]
async fn auto_approve_and_deny_policies() {
    let (rt, _, _dir) = runtime_with(None).await;
    let mut req = RunRequest::new(
        agent("mock/echo", &["danger"]),
        r#"call:danger {"target": "x"}"#,
    );
    req.auto_approve = true;
    assert_eq!(
        rt.run(req).await.unwrap().text,
        "The tool returned: destroyed x"
    );

    let denied = agent("mock/echo", &["calculator"]).policy("calculator", ToolPolicy::Deny);
    let out = rt
        .run(RunRequest::new(
            denied,
            r#"call:calculator {"expression": "1"}"#,
        ))
        .await
        .unwrap();
    assert!(out.text.contains("not allowed"), "{}", out.text);
}

#[tokio::test]
async fn step_and_cost_budgets_stop_runs() {
    let (rt, _, _dir) = runtime_with(None).await;
    let a = agent("mock/echo", &["calculator"]).limits(Limits {
        max_steps: 1,
        ..Default::default()
    });
    let out = rt
        .run(RunRequest::new(
            a,
            r#"call:calculator {"expression": "1+1"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(out.status, RunStatus::BudgetExceeded);
    assert!(out.error.unwrap().contains("step limit"));

    let a = agent("mock/echo", &[]).limits(Limits {
        max_tokens: Some(5),
        ..Default::default()
    });
    let out = rt.run(RunRequest::new(a, "hi")).await.unwrap();
    assert_eq!(out.status, RunStatus::BudgetExceeded);
}

#[tokio::test]
async fn cancel_and_timeout_stop_running_tools() {
    let (rt, _, _dir) = runtime_with(None).await;
    let h = rt
        .start(RunRequest::new(
            agent("mock/echo", &["sleep"]),
            r#"call:sleep {"secs": 30}"#,
        ))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(rt.cancel(&h.run_id));
    let out = tokio::time::timeout(Duration::from_secs(5), h.wait())
        .await
        .expect("cancel is prompt");
    assert_eq!(out.status, RunStatus::Cancelled);

    let a = agent("mock/echo", &["sleep"]).limits(Limits {
        timeout_secs: Some(1),
        ..Default::default()
    });
    let out = tokio::time::timeout(
        Duration::from_secs(5),
        rt.run(RunRequest::new(a, r#"call:sleep {"secs": 30}"#)),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out.status, RunStatus::BudgetExceeded);
    assert!(out.error.unwrap().contains("time limit"));
}

#[tokio::test]
async fn interrupted_tool_calls_are_repaired_on_the_next_run() {
    let (rt, mock, _dir) = runtime_with(Some(vec![text_response("recovered")])).await;
    let store = rt.store();
    let ses = store.create_session(Default::default()).await.unwrap();
    store
        .append_messages(
            &ses.id,
            None,
            &[
                Message::user("do it"),
                Message {
                    role: Role::Assistant,
                    content: vec![ContentBlock::ToolUse {
                        id: "t1".into(),
                        name: "calculator".into(),
                        input: json!({}),
                    }],
                },
            ],
        )
        .await
        .unwrap();
    let mut h = rt
        .start(RunRequest::new(agent("script/x", &[]), "are you there?").session(ses.id.clone()))
        .await
        .unwrap();
    let mut warned = false;
    while let Some(e) = h.next_event().await {
        warned |= matches!(e.event, RunEvent::Warning { .. });
    }
    assert!(warned);
    assert_eq!(h.wait().await.text, "recovered");
    let reqs = mock.unwrap().requests();
    let sent = &reqs.lock().unwrap()[0].messages;
    assert_eq!(sent.len(), 3);
    assert!(matches!(
        &sent[2].content[0],
        ContentBlock::ToolResult { is_error: true, .. }
    ));
    assert_eq!(sent[2].text(), "are you there?");
}

#[tokio::test]
async fn unknown_tools_and_bad_json_are_reported_to_the_model() {
    let script = vec![
        tool_response("nonexistent", json!({})),
        tool_response("calculator", json!("{not json")),
        text_response("done"),
    ];
    let (rt, mock, _dir) = runtime_with(Some(script)).await;
    let out = rt
        .run(RunRequest::new(agent("script/x", &["calculator"]), "go"))
        .await
        .unwrap();
    assert_eq!(out.status, RunStatus::Completed);
    let reqs = mock.unwrap().requests();
    let reqs = reqs.lock().unwrap();
    let result_text = |i: usize| match &reqs[i].messages.last().unwrap().content[0] {
        ContentBlock::ToolResult {
            content, is_error, ..
        } => {
            assert!(is_error);
            content[0].as_text().unwrap().to_owned()
        }
        other => panic!("{other:?}"),
    };
    assert!(result_text(1).starts_with("Unknown tool \"nonexistent\""));
    assert!(result_text(2).contains("not valid JSON"));
}

#[tokio::test]
async fn compaction_summarizes_when_the_window_fills() {
    let long = "lorem ipsum ".repeat(600); // ~2k tokens per message
    let script = vec![
        text_response(&long),
        text_response(&long),
        text_response("SUMMARY: user asked twice"), // the compaction call
        text_response("third answer"),
    ];
    let (rt, mock, _dir) = runtime_with(Some(script)).await;
    let mut a = agent("script/x", &[]);
    a.context = ContextConfig {
        max_context_tokens: Some(4_000),
        keep_recent_messages: 2,
        ..Default::default()
    };
    let first = rt.run(RunRequest::new(a.clone(), "q1")).await.unwrap();
    rt.run(RunRequest::new(a.clone(), "q2").session(first.session_id.clone()))
        .await
        .unwrap();
    let mut h = rt
        .start(RunRequest::new(a, "q3").session(first.session_id.clone()))
        .await
        .unwrap();
    let mut compacted = false;
    while let Some(e) = h.next_event().await {
        compacted |= matches!(e.event, RunEvent::Compacted { .. });
    }
    assert!(compacted);
    assert_eq!(h.wait().await.text, "third answer");
    let reqs = mock.unwrap().requests();
    let last = reqs.lock().unwrap().last().unwrap().clone();
    assert!(last.system.unwrap().contains("SUMMARY: user asked twice"));
    // Kept verbatim: the last user turn before the cut point onwards.
    let kept: Vec<String> = last.messages.iter().map(Message::text).collect();
    assert_eq!(kept.first().unwrap(), "q2");
    assert_eq!(kept.last().unwrap(), "q3");
    let h = rt.store().load_history(&first.session_id).await.unwrap();
    assert_eq!(h.summary.as_deref(), Some("SUMMARY: user asked twice"));
}

#[tokio::test]
async fn structured_output_is_parsed() {
    let (rt, _, _dir) =
        runtime_with(Some(vec![text_response("```json\n{\"answer\": 42}\n```")])).await;
    let mut a = agent("script/x", &[]);
    a.output_schema = Some(json!({"type": "object", "properties": {"answer": {"type": "number"}}}));
    let out = rt.run(RunRequest::new(a, "q")).await.unwrap();
    assert_eq!(out.output, Some(json!({"answer": 42})));
}

#[tokio::test]
async fn bad_requests_fail_before_a_run_exists() {
    let (rt, _, _dir) = runtime_with(None).await;
    assert!(
        rt.start(RunRequest::new(agent("mock/echo", &["no_such_tool"]), "x"))
            .await
            .is_err()
    );
    assert!(
        rt.start(RunRequest::new(agent("nope/x", &[]), "x"))
            .await
            .is_err()
    );
    let mut bad_template = agent("mock/echo", &[]);
    bad_template.system = Some("{% if %}".into());
    assert!(rt.start(RunRequest::new(bad_template, "x")).await.is_err());
    assert!(
        rt.store()
            .list_runs(Default::default())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn agents_are_saved_and_loaded_by_version() {
    let (rt, _, _dir) = runtime_with(None).await;
    let a = agent("mock/echo", &["calculator"]);
    assert_eq!(rt.save_agent(&a, None).await.unwrap(), 1);
    let (loaded, v) = rt.load_agent("tester", None).await.unwrap();
    assert_eq!((loaded, v), (a, 1));
}

#[tokio::test]
async fn models_without_tool_support_run_without_tools() {
    let mut info = ModelInfo::new("script", "x");
    info.capabilities.tools = false;
    let mock = Arc::new(MockModel::scripted(vec![text_response("plain answer")]).with_info(info));
    let models = Arc::new(
        ModelRegistry::builder()
            .config(LlmConfig::default())
            .provider("script", Arc::new(Scripted(mock.clone())))
            .build()
            .unwrap(),
    );
    let store = Arc::new(SqliteStore::temporary().await.unwrap());
    let dir = tempfile::tempdir().unwrap();
    let rt = Runtime::new(
        models,
        ToolRegistry::with_builtins(),
        store,
        RuntimeConfig {
            data_dir: dir.path().into(),
            ..Default::default()
        },
    );
    let mut h = rt
        .start(RunRequest::new(agent("script/x", &["calculator"]), "hi"))
        .await
        .unwrap();
    let mut warned = false;
    while let Some(e) = h.next_event().await {
        warned |= matches!(&e.event, RunEvent::Warning { message } if message.contains("cannot call tools"));
    }
    assert!(warned);
    assert_eq!(h.wait().await.text, "plain answer");
    assert!(mock.requests().lock().unwrap()[0].tools.is_empty());
}
