//! Personas: which one a run picks, where it lands in the system prompt,
//! and that a resumed run keeps the one it started with.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use gai_agent::{
    AgentSpec, Decision, PersonaDefaults, PersonaRef, PersonaSpec, Placement, RunEvent, RunRequest,
    Runtime, RuntimeConfig,
};
use gai_llm::{
    LlmConfig, LlmError, MockModel, ModelInfo, ModelRegistry, Provider, SharedChatModel,
    text_response, tool_response,
};
use gai_store::{RunStatus, SharedStore, SqliteStore};
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
struct DangerArgs {
    target: String,
}

fn runtime(store: SharedStore, dir: &std::path::Path, mock: &Arc<MockModel>) -> Runtime {
    let models = ModelRegistry::builder()
        .config(LlmConfig::default())
        .provider("script", Arc::new(Scripted(mock.clone())))
        .build()
        .unwrap();
    let mut tools = ToolRegistry::with_builtins();
    tools.register(Arc::new(
        typed_tool("danger", "Dangerous", |_ctx, a: DangerArgs| async move {
            Ok(ToolOutput::text(format!("destroyed {}", a.target)))
        })
        .policy(ToolPolicy::Approve),
    ));
    let config = RuntimeConfig {
        data_dir: dir.to_path_buf(),
        ..Default::default()
    };
    Runtime::new(Arc::new(models), tools, store, config)
}

fn scripted(n: usize) -> Arc<MockModel> {
    Arc::new(MockModel::scripted(
        (0..n)
            .map(|i| text_response(&format!("reply {i}")))
            .collect(),
    ))
}

fn persona(name: &str, tone: &str) -> PersonaSpec {
    let mut p = PersonaSpec::new(name);
    p.tone = Some(tone.into());
    p
}

fn system_prompts(m: &MockModel) -> Vec<String> {
    m.requests()
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.system.clone().unwrap_or_default())
        .collect()
}

struct Setup {
    rt: Runtime,
    mock: Arc<MockModel>,
    _dir: tempfile::TempDir,
}

async fn setup(replies: usize) -> Setup {
    let store: SharedStore = Arc::new(SqliteStore::temporary().await.unwrap());
    let dir = tempfile::tempdir().unwrap();
    let mock = scripted(replies);
    let rt = runtime(store, dir.path(), &mock);
    Setup {
        rt,
        mock,
        _dir: dir,
    }
}

fn agent() -> AgentSpec {
    AgentSpec::new("helper")
        .model("script/x")
        .system("You help.")
}

#[tokio::test]
async fn run_beats_agent_beats_global_default() {
    let s = setup(5).await;
    let rt = &s.rt;
    rt.save_persona(&persona("dry", "dry wit"), None)
        .await
        .unwrap();
    rt.save_persona(&persona("warm", "warm"), None)
        .await
        .unwrap();
    rt.save_persona(&persona("global", "neutral"), None)
        .await
        .unwrap();

    // No persona anywhere: the prompt is the agent's own.
    rt.run(RunRequest::new(agent(), "1")).await.unwrap();
    // The global default applies when the agent names none.
    rt.set_default_persona(Some(PersonaDefaults {
        persona: Some("global".into()),
        placement: Placement::Prepend,
    }));
    rt.run(RunRequest::new(agent(), "2")).await.unwrap();
    // The agent's persona beats the global default.
    let mut a = agent();
    a.persona = Some("warm".into());
    rt.run(RunRequest::new(a.clone(), "3")).await.unwrap();
    // The run's choice beats both, and "none" turns it off.
    let mut req = RunRequest::new(a.clone(), "4");
    req.persona = Some(PersonaRef::Name("dry".into()));
    let out = rt.run(req).await.unwrap();
    let mut req = RunRequest::new(a, "5");
    req.persona = Some(PersonaRef::Name("none".into()));
    rt.run(req).await.unwrap();

    let p = system_prompts(&s.mock);
    assert_eq!(p[0], "You help.");
    assert!(p[1].starts_with("<persona name=\"global\">"), "{}", p[1]);
    assert!(p[1].ends_with("</persona>\n\nYou help."), "{}", p[1]);
    assert!(p[2].contains("Tone: warm."), "{}", p[2]);
    assert!(p[3].contains("Tone: dry wit."), "{}", p[3]);
    assert_eq!(p[4], "You help.");

    let run = rt.store().get_run(&out.run_id).await.unwrap().unwrap();
    assert_eq!(run.metadata["persona"]["name"], "dry");
    assert_eq!(run.metadata["persona"]["version"], 1);
    assert_eq!(run.metadata["persona"]["placement"], "prepend");
}

#[tokio::test]
async fn placement_and_the_template_variable() {
    let s = setup(4).await;
    let rt = &s.rt;
    rt.save_persona(&persona("dry", "dry wit"), None)
        .await
        .unwrap();
    rt.save_persona(&persona("dry", "drier"), Some("v2"))
        .await
        .unwrap();

    let mut a = agent();
    a.persona = Some("dry@v1".into());
    a.persona_placement = Some(Placement::Append);
    rt.run(RunRequest::new(a.clone(), "1")).await.unwrap();

    // The template places it, so it is not added again.
    a.system = Some("Start.\n{{ persona }}\nEnd.".into());
    rt.run(RunRequest::new(a.clone(), "2")).await.unwrap();

    // An agent with no prompt of its own gets just the persona.
    a.system = None;
    rt.run(RunRequest::new(a, "3")).await.unwrap();

    // An unsaved persona can be tried inline.
    let mut req = RunRequest::new(agent(), "4");
    req.persona = Some(PersonaRef::Inline(Box::new(persona("draft", "sparkly"))));
    rt.run(req).await.unwrap();

    let p = system_prompts(&s.mock);
    assert!(
        p[0].starts_with("You help.\n\n<persona name=\"dry\">"),
        "{}",
        p[0]
    );
    assert!(p[0].contains("Tone: dry wit."), "pinned to v1: {}", p[0]);
    assert!(p[1].starts_with("Start.\n<persona"), "{}", p[1]);
    assert!(p[1].ends_with("</persona>\nEnd."), "{}", p[1]);
    assert_eq!(p[1].matches("<persona").count(), 1);
    assert!(p[2].starts_with("<persona") && p[2].ends_with("</persona>"));
    assert!(p[3].contains("Tone: sparkly."), "{}", p[3]);
}

#[tokio::test]
async fn unknown_personas_fail_before_the_run_is_recorded() {
    let s = setup(1).await;
    let mut a = agent();
    a.persona = Some("ghost".into());
    let err = s.rt.start(RunRequest::new(a, "hi")).await.err().unwrap();
    assert!(err.to_string().contains("persona ghost"), "{err}");
    let mut req = RunRequest::new(agent(), "hi");
    req.persona = Some(PersonaRef::Name("ghost@prod".into()));
    assert!(s.rt.start(req).await.is_err());
}

#[tokio::test]
async fn a_resumed_run_keeps_its_persona() {
    let dir = tempfile::tempdir().unwrap();
    let store: SharedStore = Arc::new(SqliteStore::temporary().await.unwrap());
    // Named as the agent names it, so the restarted runtime finds it.
    let mock = Arc::new(
        MockModel::scripted(vec![
            tool_response("danger", json!({"target": "prod"})),
            text_response("done"),
        ])
        .with_info(ModelInfo::new("script", "x")),
    );
    let mut a = agent();
    a.tools = vec!["danger".into()];
    a.persona = Some("dry".into());

    // Process 1 starts with v1 of the persona, asks for approval, dies.
    let rt1 = runtime(store.clone(), dir.path(), &mock);
    rt1.save_persona(&persona("dry", "dry wit"), None)
        .await
        .unwrap();
    let mut h = rt1.start(RunRequest::new(a, "go")).await.unwrap();
    loop {
        if let RunEvent::ApprovalRequired { .. } = h.next_event().await.unwrap().event {
            break;
        }
    }
    let run_id = h.run_id.clone();
    h.abort();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // The persona changes meanwhile; process 2 resumes with the old one.
    let rt2 = runtime(store.clone(), dir.path(), &mock);
    rt2.save_persona(&persona("dry", "cheerful"), None)
        .await
        .unwrap();
    assert_eq!(rt2.recover().await, vec![run_id.clone()]);
    let mut events = rt2.subscribe(&run_id).unwrap();
    let pending = loop {
        let p = rt2.approvals().pending(Some(&run_id));
        if !p.is_empty() {
            break p;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    rt2.resolve_approval(&run_id, &pending[0].call_id, Decision::approve())
        .unwrap();
    loop {
        if let RunEvent::RunFinished { status, .. } = events.recv().await.unwrap().event {
            assert_eq!(status, RunStatus::Completed);
            break;
        }
    }
    let p = system_prompts(&mock);
    assert_eq!(p.len(), 2);
    assert!(p.iter().all(|s| s.contains("Tone: dry wit.")), "{p:?}");
}
