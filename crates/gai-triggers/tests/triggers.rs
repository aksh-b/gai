use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use gai_agent::{AgentSpec, Runtime, RuntimeConfig};
use gai_llm::{LlmConfig, ModelRegistry};
use gai_orchestra::{WorkflowSpec, Workflows};
use gai_store::{RunStatus, SharedStore, SqliteStore};
use gai_tools::ToolRegistry;
use gai_triggers::{TriggerSpec, Triggers};
use serde_json::json;

struct Env {
    triggers: Triggers,
    store: SharedStore,
    dir: tempfile::TempDir,
}

async fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let store: SharedStore = Arc::new(SqliteStore::temporary().await.unwrap());
    let rt = Runtime::new(
        Arc::new(ModelRegistry::from_config(LlmConfig::default()).unwrap()),
        ToolRegistry::with_builtins(),
        store.clone(),
        RuntimeConfig {
            default_model: Some("mock/echo".into()),
            data_dir: dir.path().to_path_buf(),
            ..Default::default()
        },
    );
    rt.save_agent(&AgentSpec::new("echo").model("mock/echo"), None)
        .await
        .unwrap();
    let wf = Workflows::new(rt.clone(), vec![]);
    wf.save(
        &WorkflowSpec::parse(
            "name: sum\ninputs:\n  a: {}\n  b: {default: 1}\nnodes:\n  - id: total\n    value: \"{{ inputs.a + inputs.b }}\"\noutput: \"{{ nodes.total.output }}\"\n",
            "yaml",
        )
        .unwrap(),
        None,
    )
    .await
    .unwrap();
    wf.save(
        &WorkflowSpec::parse(
            "name: wait\nnodes:\n  - id: ask\n    human: \"go on?\"\n",
            "yaml",
        )
        .unwrap(),
        None,
    )
    .await
    .unwrap();
    rt.start_queue(4);
    Env {
        triggers: Triggers::new(rt, wf),
        store,
        dir,
    }
}

fn spec(yaml: &str) -> TriggerSpec {
    TriggerSpec::parse(yaml).unwrap()
}

/// Wait for a run to end.
async fn finished(store: &SharedStore, id: &str) -> gai_store::Run {
    for _ in 0..200 {
        let r = store.get_run(id).await.unwrap().unwrap();
        if r.status.is_terminal() {
            return r;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("run {id} did not finish");
}

#[tokio::test]
async fn schedules_fire_once_per_time_and_handle_missed_times() {
    let e = env().await;
    let t = e
        .triggers
        .create(spec(
            "name: tick\nkind: cron\nschedule: \"* * * * * *\"\ntarget: {agent: echo}\ninput: \"tick at {{ now }}\"\n",
        ))
        .await
        .unwrap();
    let due = t.next_fire_at.expect("scheduled");
    assert!(due > Utc::now() - chrono::Duration::seconds(1));

    tokio::time::sleep(Duration::from_millis(1_200)).await;
    e.triggers.tick().await.unwrap();
    // A second scheduler running the same tick finds nothing left to do.
    e.triggers.tick().await.unwrap();
    let events = e.store.list_trigger_events(&t.id, 10).await.unwrap();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(
        (events[0].reason.as_str(), events[0].status.as_str()),
        ("schedule", "started")
    );
    let run = finished(&e.store, events[0].run_id.as_ref().unwrap()).await;
    assert_eq!(run.status, RunStatus::Completed);
    assert_eq!(run.metadata["trigger"], "tick");
    assert!(
        run.output
            .unwrap()
            .as_str()
            .unwrap()
            .starts_with("tick at ")
    );

    // Missed while down: once fires with reason "missed"...
    e.store
        .set_trigger_next(&t.id, Some(Utc::now() - chrono::Duration::minutes(10)))
        .await
        .unwrap();
    e.triggers.tick().await.unwrap();
    let events = e.store.list_trigger_events(&t.id, 10).await.unwrap();
    assert_eq!(events[0].reason, "missed");
    assert_eq!(events[0].status, "started");
    // ...and skip records it without running.
    let skip = e
        .triggers
        .update(
            &t.id,
            spec("name: tick\nkind: cron\nschedule: \"@hourly\"\ncatch_up: skip\ntarget: {agent: echo}\n"),
        )
        .await
        .unwrap();
    assert!(skip.next_fire_at.unwrap() > Utc::now());
    e.store
        .set_trigger_next(&t.id, Some(Utc::now() - chrono::Duration::minutes(10)))
        .await
        .unwrap();
    e.triggers.tick().await.unwrap();
    let events = e.store.list_trigger_events(&t.id, 10).await.unwrap();
    assert_eq!(
        (events[0].reason.as_str(), events[0].status.as_str()),
        ("missed", "skipped")
    );

    // Disabled triggers do not fire.
    e.triggers.set_enabled(&t.id, false).await.unwrap();
    e.store
        .set_trigger_next(&t.id, Some(Utc::now() - chrono::Duration::seconds(1)))
        .await
        .unwrap();
    e.triggers.tick().await.unwrap();
    assert_eq!(
        e.store.list_trigger_events(&t.id, 10).await.unwrap().len(),
        3
    );
}

#[tokio::test]
async fn webhooks_fill_workflow_inputs_and_bad_targets_are_refused() {
    let e = env().await;
    assert!(
        e.triggers
            .create(spec("name: x\nkind: webhook\ntarget: {agent: nobody}\n"))
            .await
            .is_err()
    );
    let t = e
        .triggers
        .create(spec(
            "name: adder\nkind: webhook\ntarget: {workflow: sum}\ninputs: {b: \"{{ payload.extra }}\"}\n",
        ))
        .await
        .unwrap();
    assert_eq!(t.secret.as_ref().unwrap().len(), 48);
    let fired = e
        .triggers
        .fire(&t, "webhook", Some(json!({"a": 40, "extra": 2})), false)
        .await
        .unwrap();
    assert_eq!(fired.status, "started");
    let run = finished(&e.store, fired.run_id.as_ref().unwrap()).await;
    assert_eq!(run.status, RunStatus::Completed, "{:?}", run.error);
    assert_eq!(run.output.unwrap(), json!(42));
}

#[tokio::test]
async fn a_trigger_waits_for_its_previous_run() {
    let e = env().await;
    let t = e
        .triggers
        .create(spec(
            "name: asker\nkind: webhook\ntarget: {workflow: wait}\n",
        ))
        .await
        .unwrap();
    let first = e.triggers.fire(&t, "manual", None, false).await.unwrap();
    assert_eq!(first.status, "started");
    // Let the queue pick it up and reach the human step.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let second = e.triggers.fire(&t, "manual", None, false).await.unwrap();
    assert_eq!(second.status, "skipped", "{second:?}");
    assert!(
        second
            .error
            .unwrap()
            .contains(first.run_id.as_ref().unwrap())
    );
}

#[tokio::test]
async fn file_triggers_fire_on_new_files() {
    let e = env().await;
    let inbox = e.dir.path().join("inbox");
    std::fs::create_dir_all(&inbox).unwrap();
    let t = e
        .triggers
        .create(spec(&format!(
            "name: inbox\nkind: file\npath: {}\npattern: \"*.txt\"\ndebounce_ms: 200\ntarget: {{agent: echo}}\n",
            inbox.display()
        )))
        .await
        .unwrap();
    e.triggers.start_loops();
    tokio::time::sleep(Duration::from_millis(300)).await;
    std::fs::write(inbox.join("ignored.bin"), b"x").unwrap();
    std::fs::write(inbox.join("note.txt"), "buy milk").unwrap();

    let mut events = Vec::new();
    for _ in 0..100 {
        events = e.store.list_trigger_events(&t.id, 10).await.unwrap();
        if !events.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(
        events[0].payload.as_ref().unwrap()["path"]
            .as_str()
            .unwrap()
            .ends_with("note.txt")
    );
    let run = finished(&e.store, events[0].run_id.as_ref().unwrap()).await;
    let out = run.output.unwrap();
    let out = out.as_str().unwrap();
    assert!(
        out.contains("note.txt was created") && out.contains("buy milk"),
        "{out}"
    );
}
