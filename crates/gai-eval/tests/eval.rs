//! Evaluations end to end on the mock models: `mock/echo` as the agent
//! (it repeats the input, or calls a tool on `call:<tool> <json>`) and
//! `mock/judge` as the judge.

use std::sync::Arc;

use gai_agent::{AgentSpec, Runtime, RuntimeConfig};
use gai_eval::{EvalOptions, EvalSet, Evaluator, Target, compare, gate};
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
            default_model: Some("mock/echo".into()),
            data_dir: dir.to_path_buf(),
            ..Default::default()
        },
    )
}

const SET: &str = r#"
name: echo-basics
description: What the echo agent should manage.
agent: echoer
threshold: 0.6
judge_model: mock/judge
cases:
  - id: greets
    input: hello there friend
    expected: hello
  - input: please mention refunds within thirty days
    scorers:
      - judge: Mentions refunds within thirty days
      - not_contains: [sorry]
      - max_steps: 2
  - id: maths
    input: 'call:calculator {"expression": "6*7"}'
    scorers:
      - contains: ["42"]
      - tool_called: calculator
  - id: wrong
    input: nothing useful
    expected: banana
    tags: [hard]
"#;

#[tokio::test]
async fn runs_scores_stores_and_compares() {
    let dir = tempfile_dir();
    let rt = runtime(&dir).await;
    rt.save_agent(&AgentSpec::new("echoer").tools(["calculator"]), None)
        .await
        .unwrap();
    let ev = Evaluator::new(rt.clone());
    let set = EvalSet::from_yaml(SET).unwrap();
    assert_eq!(set.cases[1].id, "2");
    assert_eq!(ev.save(&set, None).await.unwrap(), 1);

    let seen = Arc::new(std::sync::Mutex::new(0));
    let counter = seen.clone();
    let tick: gai_eval::Progress = Arc::new(move |_p| *counter.lock().unwrap() += 1);
    let report = ev
        .run(
            "echo-basics",
            &Target::default(),
            &EvalOptions::default(),
            Some(tick),
        )
        .await
        .unwrap();
    assert_eq!(*seen.lock().unwrap(), 5, "a start and four cases");
    let passed: Vec<(&str, bool)> = report
        .results
        .iter()
        .map(|r| (r.case_id.as_str(), r.passed))
        .collect();
    assert_eq!(
        passed,
        [
            ("greets", true),
            ("2", true),
            ("maths", true),
            ("wrong", false)
        ],
        "{:#?}",
        report.results
    );
    let judged = &report.results[1].scores[0];
    assert_eq!(judged["scorer"], "judge");
    assert!(judged["score"].as_f64().unwrap() >= 0.7, "{judged}");
    assert_eq!(report.summary.passed, 3);
    assert!(report.summary.ok, "3/4 is over the 0.6 threshold");
    assert_eq!(report.run.target["agent"], "echoer");
    assert!(report.results.iter().all(|r| r.run_id.is_some()));

    // Stored, and readable back.
    let again = ev.report(&report.run.id).await.unwrap();
    assert_eq!(again.summary, report.summary);
    assert_eq!(again.results.len(), 4);

    // Filters, a model override, and comparing two evaluations.
    let only_hard = ev
        .run(
            "echo-basics@v1",
            &Target {
                agent: Some("echoer".into()),
                model: Some("mock/echo".into()),
            },
            &EvalOptions {
                tags: vec!["hard".into()],
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(only_hard.results.len(), 1);
    let c = compare(&report, &only_hard);
    assert_eq!(c.cases.len(), 4);
    assert!(!gate(&only_hard, Some(&report), 0.05).ok);
    assert!(gate(&report, None, 0.05).ok);

    // Unknown sets and agents.
    assert!(
        ev.run("nope", &Target::default(), &EvalOptions::default(), None)
            .await
            .is_err()
    );
    assert!(
        ev.run(
            "echo-basics",
            &Target {
                agent: Some("ghost".into()),
                model: None
            },
            &EvalOptions::default(),
            None
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn grounding_and_citations_use_what_tools_returned() {
    let dir = tempfile_dir();
    let rt = runtime(&dir).await;
    rt.knowledge()
        .create_collection(gai_memory::NewCollection {
            name: "hr".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    rt.knowledge()
        .ingest(
            "hr",
            gai_memory::DocumentInput {
                source: "leave.md".into(),
                title: Some("Leave policy".into()),
                text: "Everyone gets twenty five days of annual leave.".into(),
                metadata: serde_json::json!({}),
            },
        )
        .await
        .unwrap();
    let mut spec = AgentSpec::new("hr-bot");
    spec.knowledge = vec!["hr".into()];
    rt.save_agent(&spec, None).await.unwrap();
    let set = EvalSet::from_yaml(
        r#"
name: grounding
agent: hr-bot
judge_model: mock/judge
cases:
  - id: searched
    input: 'call:rag_search {"query": "annual leave days"}'
    scorers: [{cites: true}, {tool_called: rag_search}]
  - id: no-search
    input: Everyone gets ninety days of leave
    scorers: [{grounded: true}]
"#,
    )
    .unwrap();
    let ev = Evaluator::new(rt);
    ev.save(&set, None).await.unwrap();
    let r = ev
        .run(
            "grounding",
            &Target::default(),
            &EvalOptions::default(),
            None,
        )
        .await
        .unwrap();
    let searched = &r.results[0];
    assert!(searched.passed, "{:#?}", searched);
    let none = &r.results[1];
    assert!(!none.passed);
    assert!(
        none.scores[0]["reason"]
            .as_str()
            .unwrap()
            .contains("no sources"),
        "{}",
        none.scores
    );
}

fn tempfile_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(gai_core::new_id("eval-test"));
    std::fs::create_dir_all(&d).unwrap();
    d
}
