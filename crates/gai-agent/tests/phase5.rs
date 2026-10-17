//! Memory blocks, knowledge, skills and the background-run queue, end to
//! end through the runtime.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use gai_agent::{AgentSpec, RunRequest, Runtime, RuntimeConfig};
use gai_core::ContentBlock;
use gai_llm::{
    ChatResponse, LlmConfig, LlmError, MockModel, ModelInfo, ModelRegistry, Provider,
    SharedChatModel, text_response, tool_response,
};
use gai_memory::{DocumentInput, NewCollection};
use gai_store::{RunStatus, SharedStore, SqliteStore};
use gai_tools::ToolRegistry;
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

struct Setup {
    store: SharedStore,
    dir: tempfile::TempDir,
}

impl Setup {
    async fn new() -> Self {
        Self {
            store: Arc::new(SqliteStore::temporary().await.unwrap()),
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn runtime(&self, script: Option<&Arc<MockModel>>, private_ok: bool) -> Runtime {
        let mut b = ModelRegistry::builder().config(LlmConfig::default());
        if let Some(m) = script {
            b = b.provider("script", Arc::new(Scripted(m.clone())));
        }
        let mut config = RuntimeConfig {
            default_model: Some("mock/echo".into()),
            data_dir: self.dir.path().to_path_buf(),
            skills_dirs: vec![self.dir.path().join("skills")],
            ..Default::default()
        };
        config.tools.allow_private_network = private_ok;
        Runtime::new(
            Arc::new(b.build().unwrap()),
            ToolRegistry::with_builtins(),
            self.store.clone(),
            config,
        )
    }
}

fn scripted(responses: Vec<ChatResponse>) -> Arc<MockModel> {
    Arc::new(MockModel::scripted(responses))
}

fn system_prompts(m: &MockModel) -> Vec<String> {
    m.requests()
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.system.clone().unwrap_or_default())
        .collect()
}

fn memory_agent() -> AgentSpec {
    serde_json::from_value(json!({
        "name": "tutor",
        "model": "script/x",
        "system": "You tutor.",
        "memory": {
            "blocks": [
                {"label": "persona", "value": "Patient and brief."},
                {"label": "human", "scope": "user", "description": "About the student"}
            ]
        }
    }))
    .unwrap()
}

#[tokio::test]
async fn memory_blocks_reach_the_prompt_and_edits_persist_per_user() {
    let s = Setup::new().await;
    let m = scripted(vec![
        tool_response(
            "memory_append",
            json!({"label": "human", "content": "Name: Ann. Studies algebra."}),
        ),
        text_response("Noted."),
        text_response("Welcome back, Ann."),
    ]);
    let rt = s.runtime(Some(&m), false);
    let mut req = RunRequest::new(memory_agent(), "I'm Ann and I study algebra.");
    req.user_id = Some("ann".into());
    let out = rt.run(req).await.unwrap();
    assert_eq!(out.status, RunStatus::Completed, "{:?}", out.error);

    let prompts = system_prompts(&m);
    assert!(
        prompts[0].starts_with("You tutor.\n\n<memory_blocks>"),
        "{}",
        prompts[0]
    );
    assert!(prompts[0].contains("<persona chars=\"18/2000\">\nPatient and brief.\n</persona>"));
    assert!(prompts[0].contains("<human chars=\"0/2000\" description=\"About the student\">"));
    assert!(!prompts[0].contains("Name: Ann"));
    assert!(
        prompts[1].contains("Name: Ann. Studies algebra."),
        "edit shows up on the next step"
    );
    let tools: Vec<String> = m.requests().lock().unwrap()[0]
        .tools
        .iter()
        .map(|t| t.name.clone())
        .collect();
    assert_eq!(tools, ["memory_append", "memory_replace", "memory_rethink"]);

    // A new conversation, same user: the profile is still there.
    let mut req = RunRequest::new(memory_agent(), "Hi again");
    req.user_id = Some("ann".into());
    rt.run(req).await.unwrap();
    assert!(system_prompts(&m)[2].contains("Name: Ann."));

    let block = rt
        .store()
        .find_block("user:ann", "human")
        .await
        .unwrap()
        .unwrap();
    let history = rt.store().block_history(&block.id).await.unwrap();
    assert_eq!(history[0].op, "append");
    assert_eq!(history[0].actor.as_deref(), Some(out.run_id.as_str()));
}

#[tokio::test]
async fn knowledge_and_skills_are_offered_by_progressive_disclosure() {
    let s = Setup::new().await;
    let skill = s.dir.path().join("skills/haiku");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: haiku\ndescription: Write haiku. Use when asked for a poem.\n---\nCount syllables: 5, 7, 5.\n",
    )
    .unwrap();
    let m = scripted(vec![
        tool_response("rag_search", json!({"query": "refund days"})),
        tool_response("load_skill", json!({"name": "haiku"})),
        text_response("Refunds take five days."),
    ]);
    let rt = s.runtime(Some(&m), false);
    rt.knowledge()
        .create_collection(NewCollection {
            name: "faq".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    rt.knowledge()
        .ingest(
            "faq",
            DocumentInput {
                source: "refunds.md".into(),
                text: "Refunds take five working days to arrive.".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let spec: AgentSpec = serde_json::from_value(json!({
        "name": "helper", "model": "script/x", "knowledge": ["faq"], "skills": ["*"]
    }))
    .unwrap();
    let out = rt
        .run(RunRequest::new(
            spec.clone(),
            "How long do refunds take? Then a poem.",
        ))
        .await
        .unwrap();
    assert_eq!(out.status, RunStatus::Completed, "{:?}", out.error);
    let prompts = system_prompts(&m);
    assert!(prompts[0].contains("<skills>") && prompts[0].contains("- haiku: Write haiku."));
    assert!(!prompts[0].contains("Count syllables"));

    let messages = rt.store().list_messages(&out.session_id).await.unwrap();
    let results: Vec<String> = messages
        .iter()
        .flat_map(|m| m.message.content.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolResult { content, .. } => {
                Some(content.iter().filter_map(ContentBlock::as_text).collect())
            }
            _ => None,
        })
        .collect();
    assert!(results[0].contains("[1] refunds.md") && results[0].contains("five working days"));
    assert!(results[1].contains("Count syllables: 5, 7, 5."));

    let bad: AgentSpec =
        serde_json::from_value(json!({"name": "x", "knowledge": ["nope"]})).unwrap();
    assert!(rt.start(RunRequest::new(bad, "hi")).await.is_err());
    let bad: AgentSpec = serde_json::from_value(json!({"name": "x", "skills": ["nope"]})).unwrap();
    let e = rt.start(RunRequest::new(bad, "hi")).await.err().unwrap();
    assert!(e.to_string().contains("unknown skills"), "{e}");
}

#[tokio::test]
async fn background_runs_wait_in_the_store_survive_a_restart_and_call_back() {
    use wiremock::matchers::{header, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let hook = MockServer::start().await;
    Mock::given(method("POST"))
        .and(header("x-gai-event", "run.finished"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&hook)
        .await;
    let s = Setup::new().await;

    // Without permission, a callback to a local address is refused.
    let strict = s.runtime(None, false);
    let mut req = RunRequest::new(AgentSpec::new("bg"), "nope");
    req.callback_url = Some(format!("{}/done", hook.uri()));
    assert!(strict.enqueue(req).await.is_err());

    let first = s.runtime(None, true);
    let mut req = RunRequest::new(AgentSpec::new("bg"), "hello from the queue");
    req.callback_url = Some(format!("{}/done", hook.uri()));
    let (run_id, _) = first.enqueue(req).await.unwrap();
    let run = first.store().get_run(&run_id).await.unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Queued);
    drop(first); // the process "stops" before any worker picked it up

    let second = s.runtime(None, true);
    let finished = Arc::new(AtomicUsize::new(0));
    let f = finished.clone();
    second.set_finish_hook(
        "count",
        Arc::new(move |_o| {
            let f = f.clone();
            Box::pin(async move {
                f.fetch_add(1, Ordering::SeqCst);
            })
        }),
    );
    assert!(
        second.recover().await.is_empty(),
        "queued runs are the queue's job"
    );
    second.start_queue(2);
    let mut run = None;
    for _ in 0..100 {
        let r = second.store().get_run(&run_id).await.unwrap().unwrap();
        if r.status.is_terminal() {
            run = Some(r);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let run = run.expect("queued run finished");
    assert_eq!(run.status, RunStatus::Completed, "{:?}", run.error);
    assert_eq!(run.output, Some(json!("hello from the queue")));
    assert_eq!(finished.load(Ordering::SeqCst), 1);

    let mut calls = Vec::new();
    for _ in 0..50 {
        calls = hook.received_requests().await.unwrap_or_default();
        if !calls.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(calls.len(), 1);
    let body: serde_json::Value = calls[0].body_json().unwrap();
    assert_eq!(
        (body["run_id"].as_str(), body["status"].as_str()),
        (Some(run_id.as_str()), Some("completed"))
    );
}
