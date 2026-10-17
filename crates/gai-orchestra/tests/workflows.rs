//! Workflows and delegation end to end over a real runtime and store.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use gai_agent::{AgentSpec, Decision, RunEvent, RunRequest, Runtime, RuntimeConfig};
use gai_core::ContentBlock;
use gai_llm::{
    ChatResponse, LlmConfig, LlmError, MockModel, ModelInfo, ModelRegistry, Provider,
    SharedChatModel, text_response, tool_response,
};
use gai_orchestra::{WorkflowRequest, WorkflowSpec, Workflows};
use gai_store::{RunFilter, RunStatus, SharedStore, SqliteStore, StepKind};
use gai_tools::{ToolError, ToolOutput, ToolRegistry, typed_tool};
use serde_json::{Value, json};

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

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct Empty {}

struct Env {
    store: SharedStore,
    dir: tempfile::TempDir,
    counter: Arc<AtomicUsize>,
    flaky: Arc<AtomicUsize>,
}

impl Env {
    async fn new() -> Self {
        Self {
            store: Arc::new(SqliteStore::temporary().await.unwrap()),
            dir: tempfile::tempdir().unwrap(),
            counter: Arc::default(),
            flaky: Arc::default(),
        }
    }

    /// A fresh runtime and engine over the shared store: a new "process".
    fn engine(&self, script: Option<Vec<ChatResponse>>) -> Workflows {
        let mut b = ModelRegistry::builder().config(LlmConfig::default());
        if let Some(s) = script {
            b = b.provider(
                "script",
                Arc::new(Scripted(Arc::new(MockModel::scripted(s)))),
            );
        }
        let mut tools = ToolRegistry::with_builtins();
        let c = self.counter.clone();
        tools.register(Arc::new(typed_tool(
            "count",
            "Count calls",
            move |_ctx, _a: Empty| {
                let c = c.clone();
                async move {
                    Ok(ToolOutput::text(
                        (c.fetch_add(1, Ordering::SeqCst) + 1).to_string(),
                    ))
                }
            },
        )));
        let f = self.flaky.clone();
        tools.register(Arc::new(typed_tool(
            "flaky",
            "Fails twice",
            move |_ctx, _a: Empty| {
                let f = f.clone();
                async move {
                    if f.fetch_add(1, Ordering::SeqCst) < 2 {
                        Err(ToolError::Failed("not yet".into()))
                    } else {
                        Ok(ToolOutput::text("finally"))
                    }
                }
            },
        )));
        let config = RuntimeConfig {
            default_model: Some("mock/echo".into()),
            data_dir: self.dir.path().to_path_buf(),
            agents_dirs: vec![self.dir.path().join("agents")],
            ..Default::default()
        };
        let rt = Runtime::new(
            Arc::new(b.build().unwrap()),
            tools,
            self.store.clone(),
            config,
        );
        Workflows::new(rt, vec![self.dir.path().join("workflows")])
    }
}

fn spec(yaml: &str) -> WorkflowSpec {
    WorkflowSpec::parse(yaml, "yaml").unwrap()
}

fn inputs(v: Value) -> serde_json::Map<String, Value> {
    v.as_object().unwrap().clone()
}

#[tokio::test]
async fn a_dag_runs_agents_maps_tools_and_transforms() {
    let env = Env::new().await;
    let w = env.engine(None);
    let s = spec(
        r#"
name: demo
inputs:
  topics: {}
  n: { default: 7 }
nodes:
  - id: research
    agent: { name: echoer, model: mock/echo }
    input: "{{ inputs.topics }}"
  - id: sections
    map: "nodes.research.output | lines"
    agent: { name: echoer, model: mock/echo }
    input: "Section: {{ item }}"
    concurrency: 2
  - id: joined
    template: "{{ nodes.sections.output | join(' + ') }}"
  - id: calc
    tool: calculator
    args: { expression: "{{ inputs.n }} * 2" }
  - id: never
    if: "inputs.n > 100"
    template: unreachable
  - id: after_never
    template: "{{ nodes.never.output }}"
  - id: rescue
    needs: [never]
    if: "nodes.never.status == 'skipped'"
    value: { rescued: true, items: "{{ nodes.sections.output | length }}" }
output:
  joined: "{{ nodes.joined.output }}"
  calc: "{{ nodes.calc.output }}"
  rescue: "{{ nodes.rescue.output }}"
"#,
    );
    let mut h = w
        .start(WorkflowRequest::new(
            s,
            inputs(json!({"topics": "- alpha\n- beta"})),
        ))
        .await
        .unwrap();
    let mut kinds = Vec::new();
    while let Some(e) = h.next_event().await {
        if let RunEvent::NodeFinished { node, status, .. } = &e.event {
            kinds.push(format!("{node}:{status}"));
        }
    }
    let out = h.wait().await;
    assert_eq!(out.status, RunStatus::Completed, "{:?}", out.error);
    assert_eq!(
        out.output.unwrap(),
        json!({"joined": "Section: alpha + Section: beta", "calc": 14,
               "rescue": {"rescued": true, "items": 2}})
    );
    for k in [
        "never:skipped",
        "after_never:skipped",
        "rescue:completed",
        "sections:completed",
    ] {
        assert!(kinds.contains(&k.to_owned()), "{k} in {kinds:?}");
    }
    let store = w.runtime().store();
    let children = store
        .list_runs(RunFilter {
            parent_run_id: Some(out.run_id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(children.len(), 3, "research + two sections");
    assert!(
        children
            .iter()
            .any(|c| c.metadata["child_key"] == "sections[1]")
    );
    let steps = store.list_steps(&out.run_id).await.unwrap();
    assert_eq!(steps.iter().filter(|s| s.kind == StepKind::Node).count(), 7);
    let run = store.get_run(&out.run_id).await.unwrap().unwrap();
    assert_eq!(
        (run.agent.as_str(), run.metadata["kind"].as_str()),
        ("workflow:demo", Some("workflow"))
    );
}

#[tokio::test]
async fn failures_retries_and_continue_on_error() {
    let env = Env::new().await;
    let w = env.engine(None);
    let out = w
        .run(WorkflowRequest::new(
            spec("nodes:\n  - id: f\n    tool: flaky\n    retries: 2\n  - id: g\n    template: '{{ nodes.f.output }}!'\n"),
            Default::default(),
        ))
        .await
        .unwrap();
    assert_eq!(out.status, RunStatus::Completed, "{:?}", out.error);
    assert_eq!(out.output, Some(json!("finally!")));
    assert_eq!(env.flaky.load(Ordering::SeqCst), 3);

    let out = w
        .run(WorkflowRequest::new(
            spec("nodes:\n  - id: bad\n    tool: nope\n  - id: later\n    template: x\n    needs: [bad]\n"),
            Default::default(),
        ))
        .await
        .unwrap();
    assert_eq!(out.status, RunStatus::Failed);
    assert!(
        out.error
            .unwrap()
            .contains("node bad failed: unknown tool nope")
    );

    let out = w
        .run(WorkflowRequest::new(
            spec("nodes:\n  - id: bad\n    tool: nope\n    continue_on_error: true\n  - id: later\n    if: \"nodes.bad.status == 'failed'\"\n    template: handled\n"),
            Default::default(),
        ))
        .await
        .unwrap();
    assert_eq!(
        (out.status, out.output),
        (RunStatus::Completed, Some(json!("handled")))
    );

    let missing = w
        .start(WorkflowRequest::new(
            spec("inputs:\n  q: {}\nnodes:\n  - id: a\n    template: '{{ inputs.q }}'\n"),
            Default::default(),
        ))
        .await
        .err()
        .unwrap();
    assert!(missing.to_string().contains("missing input"));
}

#[tokio::test]
async fn a_restart_resumes_without_redoing_finished_nodes() {
    let env = Env::new().await;
    let s = spec(
        r#"
name: approve
nodes:
  - id: counted
    tool: count
  - id: ok
    human: "Ship build {{ nodes.counted.output }}?"
  - id: done
    template: "shipped {{ nodes.counted.output }} by {{ nodes.ok.output.response.who }}"
"#,
    );
    let first = env.engine(None);
    let h = first
        .start(WorkflowRequest::new(s, Default::default()))
        .await
        .unwrap();
    let run_id = h.run_id.clone();
    let rt1 = first.runtime().clone();
    for _ in 0..100 {
        if !rt1.approvals().pending(Some(&run_id)).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let pending = rt1.approvals().pending(Some(&run_id));
    assert_eq!(pending[0].input["prompt"], "Ship build 1?");
    h.abort(); // the process dies while waiting for a person
    tokio::time::sleep(Duration::from_millis(50)).await;
    drop(first);

    let second = env.engine(None);
    let rt2 = second.runtime().clone();
    assert_eq!(rt2.recover().await, vec![run_id.clone()]);
    let pending = loop {
        let p = rt2.approvals().pending(Some(&run_id));
        if !p.is_empty() {
            break p;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(pending[0].call_id, "ok");
    rt2.resolve_approval(
        &run_id,
        "ok",
        Decision {
            approved: true,
            reason: None,
            input: Some(json!({"who": "ana"})),
        },
    )
    .unwrap();
    let out = rt2.wait_for(&run_id).await.unwrap();
    assert_eq!(out.status, RunStatus::Completed, "{:?}", out.error);
    assert_eq!(out.output, Some(json!("shipped 1 by ana")));
    assert_eq!(
        env.counter.load(Ordering::SeqCst),
        1,
        "the finished tool node was not repeated"
    );
}

#[tokio::test]
async fn agents_delegate_to_agents_as_child_runs() {
    let env = Env::new().await;
    std::fs::create_dir_all(env.dir.path().join("agents")).unwrap();
    std::fs::write(
        env.dir.path().join("agents/helper.yaml"),
        "model: mock/echo\ndescription: Repeats things back.\n",
    )
    .unwrap();
    let w = env.engine(Some(vec![
        tool_response(
            "ask_helper",
            json!({"task": "say hi", "context": "be brief"}),
        ),
        text_response("The helper answered."),
    ]));
    let rt = w.runtime().clone();
    let boss: AgentSpec = serde_json::from_value(
        json!({"name": "boss", "model": "script/x", "delegates": ["helper"]}),
    )
    .unwrap();
    let out = rt
        .run(RunRequest::new(boss, "delegate please"))
        .await
        .unwrap();
    assert_eq!(out.status, RunStatus::Completed, "{:?}", out.error);

    let kids = rt
        .store()
        .list_runs(RunFilter {
            parent_run_id: Some(out.run_id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(kids.len(), 1);
    assert_eq!(kids[0].agent, "helper");
    assert_eq!(kids[0].metadata["depth"], 1);
    let answer = kids[0].output.clone().unwrap();
    assert!(answer.as_str().unwrap().starts_with("say hi"), "{answer}");
    // The tool result the boss saw is the helper's answer.
    let msgs = rt.store().list_messages(&out.session_id).await.unwrap();
    let result: String = msgs
        .iter()
        .flat_map(|m| m.message.content.iter())
        .find_map(|b| match b {
            ContentBlock::ToolResult { content, .. } => {
                Some(content.iter().filter_map(ContentBlock::as_text).collect())
            }
            _ => None,
        })
        .unwrap();
    assert!(result.starts_with("say hi"), "{result}");

    // Asking again with the same key reuses the child instead of re-running it.
    let key = kids[0].metadata["child_key"].as_str().unwrap().to_owned();
    let (helper, _) = rt.resolve_agent("helper").await.unwrap();
    let again = rt
        .run_child(
            &out.run_id,
            &key,
            RunRequest::new(helper, "say hi"),
            &Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(again.run_id, kids[0].id);

    let bad: AgentSpec =
        serde_json::from_value(json!({"name": "x", "delegates": ["ghost"]})).unwrap();
    assert!(rt.start(RunRequest::new(bad, "hi")).await.is_err());
}

#[tokio::test]
async fn workflow_runs_can_be_queued() {
    let env = Env::new().await;
    let w = env.engine(None);
    let (run_id, _) = w
        .enqueue(WorkflowRequest::new(
            spec("nodes:\n  - id: a\n    value: { ok: true }\n"),
            Default::default(),
        ))
        .await
        .unwrap();
    let rt = w.runtime().clone();
    assert_eq!(
        rt.store().get_run(&run_id).await.unwrap().unwrap().status,
        RunStatus::Queued
    );
    rt.start_queue(1);
    let out = tokio::time::timeout(Duration::from_secs(10), rt.wait_for(&run_id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (out.status, out.output),
        (RunStatus::Completed, Some(json!({"ok": true})))
    );
}

#[tokio::test]
async fn the_examples_are_valid_and_hello_runs() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    for e in std::fs::read_dir(root.join("workflows")).unwrap().flatten() {
        WorkflowSpec::load(&e.path()).unwrap_or_else(|err| panic!("{err}"));
    }
    for e in std::fs::read_dir(root.join("agents")).unwrap().flatten() {
        AgentSpec::load(&e.path()).unwrap_or_else(|err| panic!("{err}"));
    }
    let env = Env::new().await;
    let w = env.engine(None);
    let (spec, _) = w
        .resolve(root.join("workflows/hello.yaml").to_str().unwrap())
        .await
        .unwrap();
    let out = w
        .run(WorkflowRequest::new(
            spec,
            inputs(json!({"names": ["ada", "alan", "grace"]})),
        ))
        .await
        .unwrap();
    assert_eq!(out.status, RunStatus::Completed, "{:?}", out.error);
    assert_eq!(
        out.output.unwrap(),
        json!({"greetings": ["Hello, ada!", "Hello, alan!", "Hello, grace!"], "score": 30, "crowd": false})
    );
}

#[tokio::test]
async fn stored_workflows_resolve_by_version_and_label() {
    let env = Env::new().await;
    let w = env.engine(None);
    let one = spec("name: lab\ndescription: first\nnodes:\n  - {id: a, value: 1}\n");
    let two = spec("name: lab\ndescription: second\nnodes:\n  - {id: a, value: 2}\n");
    assert_eq!(w.save(&one, None).await.unwrap(), 1);
    assert_eq!(w.save(&two, None).await.unwrap(), 2);
    env.store
        .set_workflow_label("lab", "prod", Some(1))
        .await
        .unwrap();
    let desc = |s: WorkflowSpec| s.description.unwrap();
    assert_eq!(desc(w.resolve("lab").await.unwrap().0), "second");
    assert_eq!(w.resolve("lab@v1").await.unwrap().1, Some(1));
    let (s, v) = w.resolve("lab@prod").await.unwrap();
    assert_eq!((desc(s), v), ("first".to_owned(), Some(1)));
    assert!(w.resolve("lab@staging").await.is_err());
}
