//! The prompt optimizer on the mock models: `mock/obey` answers by its
//! instructions, and `mock/rewriter` adds the expected answers of failing
//! cases to them.

use std::sync::{Arc, Mutex};

use gai_agent::{AgentSpec, PromptSpec, Runtime, RuntimeConfig};
use gai_eval::{EvalSet, Evaluator, OptimizeOptions, OptimizeReport, Target};
use gai_llm::{LlmConfig, ModelRegistry};
use gai_store::{SharedStore, SqliteStore};
use gai_tools::ToolRegistry;

async fn runtime(dir: &std::path::Path) -> Runtime {
    let store: SharedStore = Arc::new(SqliteStore::temporary().await.unwrap());
    let models = Arc::new(
        ModelRegistry::builder()
            .config(LlmConfig::default())
            .build()
            .unwrap(),
    );
    Runtime::new(
        models,
        ToolRegistry::with_builtins(),
        store,
        RuntimeConfig {
            default_model: Some("mock/rewriter".into()),
            data_dir: dir.to_path_buf(),
            ..Default::default()
        },
    )
}

const SET: &str = r#"
name: refunds
agent: support
cases:
  - id: window
    input: How long is the refund window?
    expected: 30 days
  - id: shipping
    input: Do you ship to Canada?
    expected: Yes, we ship to Canada
  - id: hello
    input: hello
    expected: hello
"#;

#[tokio::test]
async fn rewrites_instructions_until_the_cases_pass() {
    let dir = tempfile::tempdir().unwrap();
    let rt = runtime(dir.path()).await;
    let mut agent = AgentSpec::new("support").model("mock/obey");
    agent.system = Some("You answer customers.".into());
    rt.save_agent(&agent, None).await.unwrap();
    let ev = Evaluator::new(rt.clone());
    ev.save(&EvalSet::from_yaml(SET).unwrap(), None)
        .await
        .unwrap();

    let seen: Arc<Mutex<Vec<OptimizeReport>>> = Arc::default();
    let s = seen.clone();
    let report = ev
        .optimize(
            "refunds",
            &Target::default(),
            &OptimizeOptions::default(),
            Some(Arc::new(move |r| s.lock().unwrap().push(r.clone()))),
        )
        .await
        .unwrap();
    assert_eq!((report.baseline.passed, report.baseline.total), (1, 3));
    assert!(report.improved);
    assert_eq!(report.best.passed, 3, "{report:#?}");
    assert!(report.best.kept && report.best.round == 1);
    assert!(
        report
            .best
            .instructions
            .starts_with("You answer customers.")
    );
    assert!(report.best.instructions.contains("answer: 30 days"));
    assert_eq!(report.attempts.len(), 1, "it stops once every case passes");
    assert!(report.done);
    {
        let progress = seen.lock().unwrap();
        assert!(!progress[0].done && progress.last().unwrap().done);
    }

    // Nothing was saved to the agent.
    let (stored, _) = rt.resolve_agent("support").await.unwrap();
    assert_eq!(stored.system.as_deref(), Some("You answer customers."));
    // Each evaluation was recorded, marked as the optimizer's.
    let runs = rt
        .store()
        .list_eval_runs(Some("refunds"), 10)
        .await
        .unwrap();
    assert_eq!(runs.len(), 2);
    assert!(
        runs.iter()
            .all(|r| r.target["optimizer"]["round"].is_number())
    );
}

#[tokio::test]
async fn rewrites_the_prompt_template_an_agent_uses() {
    let dir = tempfile::tempdir().unwrap();
    let rt = runtime(dir.path()).await;
    rt.save_prompt(
        &PromptSpec::parse(
            "name: care\ntext: 'You answer customers of {{ company }}.'\nvariables:\n  company: { default: Acme }\n",
            "yaml",
        )
        .unwrap(),
        None,
    )
    .await
    .unwrap();
    let mut agent = AgentSpec::new("support").model("mock/obey");
    agent.prompt = Some("care".into());
    rt.save_agent(&agent, None).await.unwrap();
    let ev = Evaluator::new(rt.clone());
    ev.save(&EvalSet::from_yaml(SET).unwrap(), None)
        .await
        .unwrap();
    let report = ev
        .optimize(
            "refunds",
            &Target::default(),
            &OptimizeOptions {
                writer_model: Some("mock/rewriter".into()),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(report.prompt.as_deref(), Some("care@v1"));
    assert_eq!(report.best.passed, 3, "{report:#?}");
    assert!(
        report
            .best
            .instructions
            .starts_with("You answer customers of {{ company }}."),
        "the template's variables stay: {}",
        report.best.instructions
    );
}
