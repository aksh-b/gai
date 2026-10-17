//! Replay: forks reproduce recorded steps exactly, without model calls, and
//! run the rest live with overrides.

use std::sync::Arc;

use async_trait::async_trait;
use gai_agent::{
    AgentSpec, Decision, ReplayOverrides, ReplayRequest, RunEvent, RunRequest, Runtime,
    RuntimeConfig, StepEdit,
};
use gai_core::ContentBlock;
use gai_llm::{
    ChatResponse, LlmConfig, LlmError, MockModel, ModelInfo, ModelRegistry, Provider,
    SharedChatModel, text_response, tool_response,
};
use gai_store::{RunFilter, RunStatus, SqliteStore, Step, StepKind};
use gai_tools::{ToolOutput, ToolPolicy, ToolRegistry, typed_tool};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// Serves one shared model under any name.
struct Scripted(&'static str, Arc<MockModel>);

#[async_trait]
impl Provider for Scripted {
    fn name(&self) -> &str {
        self.0
    }
    fn chat(&self, _info: ModelInfo) -> Result<SharedChatModel, LlmError> {
        Ok(self.1.clone())
    }
}

#[derive(Deserialize, JsonSchema)]
struct DangerArgs {
    target: String,
}

struct Harness {
    rt: Runtime,
    original: Arc<MockModel>,
    live: Arc<MockModel>,
    _dir: tempfile::TempDir,
}

/// `script/orig` plays `original`, `live/x` plays `live`.
async fn harness(original: Vec<ChatResponse>, live: Vec<ChatResponse>) -> Harness {
    let original =
        Arc::new(MockModel::scripted(original).with_info(ModelInfo::new("script", "orig")));
    let live = Arc::new(MockModel::scripted(live).with_info(ModelInfo::new("live", "x")));
    let models = ModelRegistry::builder()
        .config(LlmConfig::default())
        .provider("script", Arc::new(Scripted("script", original.clone())))
        .provider("live", Arc::new(Scripted("live", live.clone())))
        .build()
        .unwrap();
    let mut tools = ToolRegistry::with_builtins();
    tools.register(Arc::new(
        typed_tool("danger", "Dangerous", |_ctx, a: DangerArgs| async move {
            Ok(ToolOutput::text(format!("destroyed {}", a.target)))
        })
        .policy(ToolPolicy::Approve),
    ));
    let dir = tempfile::tempdir().unwrap();
    let config = RuntimeConfig {
        default_model: Some("mock/echo".into()),
        data_dir: dir.path().to_path_buf(),
        ..Default::default()
    };
    let store = Arc::new(SqliteStore::temporary().await.unwrap());
    Harness {
        rt: Runtime::new(Arc::new(models), tools, store, config),
        original,
        live,
        _dir: dir,
    }
}

fn agent(tools: &[&str]) -> AgentSpec {
    AgentSpec::new("tester")
        .model("script/orig")
        .tools(tools.iter().copied())
        .system("You are {{ agent }}.")
}

fn calls(m: &MockModel) -> usize {
    m.requests().lock().unwrap().len()
}

fn exact(v: &Option<Value>) -> String {
    serde_json::to_string(v).unwrap()
}

/// Model steps before `from_step`, plus the tool steps they led to.
fn replayed_part(steps: &[Step], from_step: u64) -> Vec<&Step> {
    steps
        .iter()
        .take_while(|s| {
            !(s.kind == StepKind::Model
                && s.input.as_ref().unwrap()["step"].as_u64().unwrap() >= from_step)
        })
        .collect()
}

fn replay(from_step: u32, overrides: ReplayOverrides) -> ReplayRequest {
    ReplayRequest {
        from_step,
        overrides,
    }
}

#[tokio::test]
async fn fork_reproduces_earlier_steps_exactly_without_model_calls() {
    let h = harness(
        vec![
            tool_response("calculator", json!({"expression": "6 * 7"})),
            tool_response("calculator", json!({"expression": "1 + 1"})),
            text_response("42 and 2"),
        ],
        vec![text_response("forty-two and two")],
    )
    .await;
    let store = h.rt.store().clone();

    // An earlier run in the same session, which the fork should keep.
    let first =
        h.rt.run(RunRequest::new(
            AgentSpec::new("tester").model("mock/echo"),
            "hello",
        ))
        .await
        .unwrap();
    let orig =
        h.rt.run(RunRequest::new(agent(&["calculator"]), "compute").session(&first.session_id))
            .await
            .unwrap();
    assert_eq!(orig.status, RunStatus::Completed, "{:?}", orig.error);
    assert_eq!(orig.steps, 3);
    assert_eq!(calls(&h.original), 3);

    // Each model step journaled its full request.
    let orig_steps = store.list_steps(&orig.run_id).await.unwrap();
    let first_model = &orig_steps[0];
    let req = &first_model.input.as_ref().unwrap()["request"];
    assert_eq!(req["system"], "You are tester.");
    assert_eq!(req["tools"][0]["name"], "calculator");
    assert_eq!(req["messages"].as_array().unwrap().len(), 3);

    let mut handle =
        h.rt.replay(
            &orig.run_id,
            replay(
                3,
                ReplayOverrides {
                    model: Some("live/x".into()),
                    ..Default::default()
                },
            ),
            Map::new(),
        )
        .await
        .unwrap();
    let mut replayed_events = Vec::new();
    while let Some(e) = handle.next_event().await {
        if let RunEvent::StepReplayed { step, from_run_id } = e.event {
            assert_eq!(from_run_id, orig.run_id);
            replayed_events.push(step);
        }
    }
    let fork = handle.wait().await;
    assert_eq!(fork.status, RunStatus::Completed, "{:?}", fork.error);
    assert_eq!(fork.text, "forty-two and two");
    assert_eq!(replayed_events, [1, 2]);
    // Steps 1 and 2 came from the recording: no new calls to the original
    // model, one live call to the new one.
    assert_eq!(calls(&h.original), 3);
    assert_eq!(calls(&h.live), 1);
    // Only the live step costs anything.
    assert_eq!(fork.usage.input_tokens, 10);

    // Byte-for-byte: the replayed part of the journal matches the original.
    let fork_steps = store.list_steps(&fork.run_id).await.unwrap();
    let (a, b) = (replayed_part(&orig_steps, 3), replayed_part(&fork_steps, 3));
    assert_eq!(a.len(), 4);
    assert_eq!(a.len(), b.len());
    for (o, f) in a.iter().zip(&b) {
        assert_eq!(o.kind, f.kind);
        assert_eq!(o.name, f.name);
        assert_eq!(o.status, f.status);
        assert_eq!(exact(&o.output), exact(&f.output));
        if o.kind == StepKind::Tool {
            assert_eq!(exact(&o.input), exact(&f.input));
        } else {
            assert_eq!(f.input.as_ref().unwrap()["replayed_from"], orig.run_id);
        }
    }
    // The live step saw the replayed conversation.
    let live_req = h.live.requests().lock().unwrap()[0].clone();
    assert_eq!(live_req.messages.len(), 7);

    // The fork lives in a copy of the session: the earlier run's messages,
    // then its own.
    let fork_run = store.get_run(&fork.run_id).await.unwrap().unwrap();
    assert_ne!(fork_run.session_id, orig.session_id);
    assert_eq!(fork_run.metadata["fork"]["run_id"], orig.run_id);
    assert_eq!(fork_run.metadata["fork"]["from_step"], 3);
    let orig_msgs = store.list_messages(&orig.session_id).await.unwrap();
    let fork_msgs = store.list_messages(&fork_run.session_id).await.unwrap();
    assert_eq!(orig_msgs.len(), fork_msgs.len());
    for (o, f) in orig_msgs.iter().zip(&fork_msgs).take(orig_msgs.len() - 1) {
        assert_eq!(
            serde_json::to_string(&o.message).unwrap(),
            serde_json::to_string(&f.message).unwrap()
        );
    }
    assert_eq!(fork_msgs[0].run_id.as_deref(), Some(first.run_id.as_str()));

    let forks = store
        .list_runs(RunFilter {
            fork_of: Some(orig.run_id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(forks.len(), 1);
    assert_eq!(forks[0].id, fork.run_id);
}

#[tokio::test]
async fn fork_from_step_one_reruns_with_another_agent_version() {
    let h = harness(
        vec![text_response("v1 answer")],
        vec![text_response("v2 answer")],
    )
    .await;
    let v1 = agent(&[]);
    let mut v2 = agent(&[]).system("You are version two.");
    v2.model = Some("live/x".into());
    assert_eq!(h.rt.save_agent(&v1, None).await.unwrap(), 1);
    assert_eq!(h.rt.save_agent(&v2, None).await.unwrap(), 2);

    let mut req = RunRequest::new(v1, "hi");
    req.agent_version = Some(1);
    let orig = h.rt.run(req).await.unwrap();
    assert_eq!(orig.text, "v1 answer");

    let fork =
        h.rt.replay(
            &orig.run_id,
            replay(
                1,
                ReplayOverrides {
                    agent_version: Some(2),
                    ..Default::default()
                },
            ),
            Map::new(),
        )
        .await
        .unwrap()
        .wait()
        .await;
    assert_eq!(fork.text, "v2 answer");
    let run = h.rt.store().get_run(&fork.run_id).await.unwrap().unwrap();
    assert_eq!(run.agent_version, Some(2));
    let sent = h.live.requests().lock().unwrap()[0].clone();
    assert_eq!(sent.system.as_deref(), Some("You are version two."));
    assert_eq!(sent.messages.len(), 1);
}

#[tokio::test]
async fn overrides_and_step_edits() {
    let h = harness(
        vec![
            tool_response("calculator", json!({"expression": "2 + 2"})),
            text_response("4"),
        ],
        vec![text_response("five, as you insisted")],
    )
    .await;
    let orig =
        h.rt.run(RunRequest::new(agent(&["calculator"]), "2+2?"))
            .await
            .unwrap();
    let steps = h.rt.store().list_steps(&orig.run_id).await.unwrap();
    let call_id = steps[1].input.as_ref().unwrap()["call_id"]
        .as_str()
        .unwrap()
        .to_owned();

    // Out of range, or editing a live step, is refused before anything runs.
    for (from, edits) in [
        (0, vec![]),
        (4, vec![]),
        (
            2,
            vec![StepEdit {
                step: 2,
                text: Some("x".into()),
                ..Default::default()
            }],
        ),
    ] {
        let r =
            h.rt.replay(
                &orig.run_id,
                replay(
                    from,
                    ReplayOverrides {
                        step_edits: edits,
                        ..Default::default()
                    },
                ),
                Map::new(),
            )
            .await;
        assert!(r.is_err(), "from_step {from} should be refused");
    }

    let mut results = std::collections::BTreeMap::new();
    results.insert(call_id.clone(), ToolOutput::text("5"));
    let fork =
        h.rt.replay(
            &orig.run_id,
            replay(
                2,
                ReplayOverrides {
                    model: Some("live/x".into()),
                    system_prompt: Some("Be brief, {{ agent }}.".into()),
                    tools: Some(vec![]),
                    step_edits: vec![StepEdit {
                        step: 1,
                        text: Some("Let me check.".into()),
                        tool_results: results,
                        ..Default::default()
                    }],
                    ..Default::default()
                },
            ),
            Map::new(),
        )
        .await
        .unwrap()
        .wait()
        .await;
    assert_eq!(fork.status, RunStatus::Completed, "{:?}", fork.error);
    assert_eq!(calls(&h.original), 2);

    let fork_steps = h.rt.store().list_steps(&fork.run_id).await.unwrap();
    let content = &fork_steps[0].output.as_ref().unwrap()["content"];
    assert_eq!(content[0]["text"], "Let me check.");
    assert_eq!(content[1]["type"], "tool_use");
    assert_eq!(
        fork_steps[1].output.as_ref().unwrap()["content"][0]["text"],
        "5"
    );

    let sent = h.live.requests().lock().unwrap()[0].clone();
    assert_eq!(sent.system.as_deref(), Some("Be brief, tester."));
    assert!(sent.tools.is_empty());
    let last = sent.messages.last().unwrap();
    assert!(matches!(
        &last.content[0],
        ContentBlock::ToolResult { content, .. } if content[0].as_text() == Some("5")
    ));
}

#[tokio::test]
async fn replayed_approvals_are_not_asked_again() {
    let h = harness(
        vec![
            tool_response("danger", json!({"target": "prod"})),
            text_response("done"),
        ],
        vec![text_response("done again")],
    )
    .await;
    let mut handle =
        h.rt.start(RunRequest::new(agent(&["danger"]), "go"))
            .await
            .unwrap();
    let run_id = handle.run_id.clone();
    while let Some(e) = handle.next_event().await {
        if let RunEvent::ApprovalRequired { id, .. } = e.event {
            h.rt.resolve_approval(&run_id, &id, Decision::approve())
                .unwrap();
        }
    }
    let orig = handle.wait().await;
    assert_eq!(orig.status, RunStatus::Completed, "{:?}", orig.error);

    let mut fork =
        h.rt.replay(
            &orig.run_id,
            replay(
                2,
                ReplayOverrides {
                    model: Some("live/x".into()),
                    ..Default::default()
                },
            ),
            Map::new(),
        )
        .await
        .unwrap();
    while let Some(e) = fork.next_event().await {
        assert!(
            !matches!(e.event, RunEvent::ApprovalRequired { .. }),
            "a replayed call asked for approval again"
        );
    }
    let fork = fork.wait().await;
    assert_eq!(fork.status, RunStatus::Completed, "{:?}", fork.error);
    assert_eq!(fork.text, "done again");
    let steps = h.rt.store().list_steps(&fork.run_id).await.unwrap();
    assert_eq!(
        steps[1].output.as_ref().unwrap()["content"][0]["text"],
        "destroyed prod"
    );
}
