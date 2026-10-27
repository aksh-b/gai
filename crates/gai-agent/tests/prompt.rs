//! Prompt templates: an agent's `prompt` becomes its instructions, by
//! version or label, with the template's defaults for variables.

use std::sync::Arc;

use async_trait::async_trait;
use gai_agent::{AgentSpec, PromptSpec, RunRequest, Runtime, RuntimeConfig};
use gai_llm::{
    LlmConfig, LlmError, MockModel, ModelInfo, ModelRegistry, Provider, SharedChatModel,
    text_response,
};
use gai_store::{SharedStore, SqliteStore};
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

fn prompt(text: &str) -> PromptSpec {
    PromptSpec::parse(
        &format!("name: tone\ntext: '{text}'\nvariables:\n  company: {{ default: Acme }}\n"),
        "yaml",
    )
    .unwrap()
}

#[tokio::test]
async fn agents_use_stored_prompts_by_version_and_label() {
    let store: SharedStore = Arc::new(SqliteStore::temporary().await.unwrap());
    let dir = tempfile::tempdir().unwrap();
    let mock = Arc::new(MockModel::scripted(
        (0..4).map(|i| text_response(&format!("r{i}"))).collect(),
    ));
    let models = ModelRegistry::builder()
        .config(LlmConfig::default())
        .provider("script", Arc::new(Scripted(mock.clone())))
        .build()
        .unwrap();
    let rt = Runtime::new(
        Arc::new(models),
        ToolRegistry::with_builtins(),
        store.clone(),
        RuntimeConfig {
            data_dir: dir.path().to_path_buf(),
            ..Default::default()
        },
    );
    assert_eq!(
        rt.save_prompt(&prompt("Speak for {{ company }}."), None)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        rt.save_prompt(&prompt("Answer for {{ company }}."), None)
            .await
            .unwrap(),
        2
    );
    store
        .set_item_label("prompt", "tone", "prod", Some(1))
        .await
        .unwrap();

    let agent = |r: &str, system: Option<&str>| {
        let mut a = AgentSpec::new("helper").model("script/x");
        a.prompt = Some(r.into());
        a.system = system.map(str::to_owned);
        a
    };
    rt.run(RunRequest::new(agent("tone", Some("Be brief.")), "1"))
        .await
        .unwrap();
    rt.run(RunRequest::new(agent("tone@prod", None), "2"))
        .await
        .unwrap();
    let mut req = RunRequest::new(agent("tone@v2", None), "3");
    req.vars.insert("company".into(), json!("Initech"));
    let out = rt.run(req).await.unwrap();

    let systems: Vec<String> = mock
        .requests()
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.system.clone().unwrap_or_default())
        .collect();
    assert_eq!(
        systems,
        [
            "Answer for Acme.\n\nBe brief.",
            "Speak for Acme.",
            "Answer for Initech.",
        ]
    );
    let run = store.get_run(&out.run_id).await.unwrap().unwrap();
    assert_eq!(
        run.metadata["prompt"],
        json!({"name": "tone", "version": 2})
    );

    let err = rt
        .run(RunRequest::new(agent("tone@staging", None), "4"))
        .await;
    assert!(err.unwrap_err().to_string().contains("no label"));
    let err = rt.run(RunRequest::new(agent("missing", None), "4")).await;
    assert!(err.is_err());
}
