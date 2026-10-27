//! Behaviour every store backend must share. Each check takes a fresh,
//! empty store and panics on a difference; backends run them in their own
//! tests (SQLite always, Postgres when `GAI_TEST_POSTGRES_URL` is set).

use serde_json::{Value, json};

use chrono::{Duration, Utc};
use gai_core::{Message, Usage};

use crate::*;

fn coll(name: &str) -> Collection {
    Collection {
        name: name.into(),
        description: None,
        embedding_model: "mock/hash".into(),
        dimensions: None,
        chunk_tokens: 200,
        chunk_overlap: 20,
        metadata: Value::Null,
        documents: 0,
        chunks: 0,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

fn doc(collection: &str, source: &str) -> NewDocument {
    NewDocument {
        collection: collection.into(),
        source: source.into(),
        title: Some(source.into()),
        content_hash: "h".into(),
        chars: 10,
        metadata: Value::Null,
    }
}

fn ch(text: &str, v: Vec<f32>) -> NewChunk {
    NewChunk {
        text: text.into(),
        embedding: Some(v),
        metadata: json!({}),
    }
}

/// Versions, dedup of identical saves, labels and deletes.
pub async fn agents(s: SharedStore) {
    let a = json!({"name": "a", "model": "mock/echo"});
    assert_eq!(s.put_agent("a", &a, Some("first")).await.unwrap(), 1);
    // Saving the same spec again is not a new version.
    assert_eq!(s.put_agent("a", &a, None).await.unwrap(), 1);
    let b = json!({"name": "a", "model": "mock/other"});
    assert_eq!(s.put_agent("a", &b, None).await.unwrap(), 2);
    assert_eq!(s.get_agent("a", None).await.unwrap().unwrap().spec, b);
    let v1 = s.get_agent("a", Some(1)).await.unwrap().unwrap();
    assert_eq!((v1.spec, v1.note.as_deref()), (a, Some("first")));
    assert!(s.get_agent("a", Some(9)).await.unwrap().is_none());
    assert!(s.get_agent("nope", None).await.unwrap().is_none());
    let versions: Vec<u32> = s
        .list_agent_versions("a")
        .await
        .unwrap()
        .iter()
        .map(|v| v.version)
        .collect();
    assert_eq!(versions, [2, 1]);

    s.set_agent_label("a", "prod", Some(1)).await.unwrap();
    assert!(matches!(
        s.set_agent_label("a", "prod", Some(7)).await,
        Err(StoreError::NotFound(_))
    ));
    assert!(matches!(
        s.set_agent_label("nope", "prod", Some(1)).await,
        Err(StoreError::NotFound(_))
    ));
    s.put_agent("b", &json!({"name": "b"}), None).await.unwrap();
    let list = s.list_agents().await.unwrap();
    assert_eq!(
        list.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(list[0].latest_version, 2);
    assert_eq!(list[0].labels.get("prod"), Some(&1));
    s.set_agent_label("a", "prod", None).await.unwrap();
    assert!(s.list_agents().await.unwrap()[0].labels.is_empty());

    assert!(s.delete_agent("a").await.unwrap());
    assert!(!s.delete_agent("a").await.unwrap());
    assert!(s.get_agent("a", Some(1)).await.unwrap().is_none());
}

/// Get, overwrite, list by prefix, delete.
pub async fn settings(s: SharedStore) {
    assert_eq!(s.get_setting("default_model").await.unwrap(), None);
    s.put_setting("default_model", "mock/echo").await.unwrap();
    s.put_setting("default_model", "ollama/x").await.unwrap();
    s.put_setting("provider_key:openai", "sk-1").await.unwrap();
    s.put_setting("provider_key:anthropic", "sk-2")
        .await
        .unwrap();
    // A prefix is literal, not a pattern.
    s.put_setting("provider_keyX", "no").await.unwrap();
    assert_eq!(
        s.get_setting("default_model").await.unwrap().as_deref(),
        Some("ollama/x")
    );
    assert_eq!(
        s.list_settings("provider_key:").await.unwrap(),
        [
            ("provider_key:anthropic".to_owned(), "sk-2".to_owned()),
            ("provider_key:openai".to_owned(), "sk-1".to_owned()),
        ]
    );
    assert!(s.delete_setting("provider_key:openai").await.unwrap());
    assert!(!s.delete_setting("provider_key:openai").await.unwrap());
}

/// Server configs and OAuth tokens.
pub async fn mcp(s: SharedStore) {
    s.put_mcp_server("b", &json!({"url": "http://b"}))
        .await
        .unwrap();
    s.put_mcp_server("a", &json!({"command": "x"}))
        .await
        .unwrap();
    s.put_mcp_server("a", &json!({"command": "y"}))
        .await
        .unwrap();
    let all = s.list_mcp_servers().await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0], ("a".to_owned(), json!({"command": "y"})));
    assert!(s.delete_mcp_server("b").await.unwrap());
    assert!(!s.delete_mcp_server("b").await.unwrap());

    assert_eq!(s.get_mcp_oauth("a").await.unwrap(), None);
    s.put_mcp_oauth("a", &json!({"token": 1})).await.unwrap();
    s.put_mcp_oauth("a", &json!({"token": 2})).await.unwrap();
    assert_eq!(
        s.get_mcp_oauth("a").await.unwrap(),
        Some(json!({"token": 2}))
    );
    s.delete_mcp_oauth("a").await.unwrap();
    assert_eq!(s.get_mcp_oauth("a").await.unwrap(), None);
}

pub async fn settings_round_trip(s: SharedStore) {
    assert_eq!(s.get_setting("default_model").await.unwrap(), None);
    s.put_setting("default_model", "mock/echo").await.unwrap();
    s.put_setting("default_model", "ollama/gemma3:1b")
        .await
        .unwrap();
    s.put_setting("provider_key:openai", "sk-1").await.unwrap();
    assert_eq!(
        s.get_setting("default_model").await.unwrap().as_deref(),
        Some("ollama/gemma3:1b")
    );
    let keys = s.list_settings("provider_key:").await.unwrap();
    assert_eq!(keys, [("provider_key:openai".into(), "sk-1".into())]);
    assert!(s.delete_setting("provider_key:openai").await.unwrap());
    assert!(!s.delete_setting("provider_key:openai").await.unwrap());
}

pub async fn concurrent_writers_wait_instead_of_failing(s: SharedStore) {
    let ses = s.create_session(NewSession::default()).await.unwrap();
    let k = s
        .create_key(&NewKey::new("k", "p", "h", &[Scope::Run], None))
        .await
        .unwrap();
    let mut jobs = Vec::new();
    for i in 0..24 {
        let s = s.clone();
        let ses = ses.id.clone();
        let key = k.id.clone();
        jobs.push(tokio::spawn(async move {
            s.append_messages(&ses, None, &[Message::user(format!("m{i}"))])
                .await?;
            s.put_agent(&format!("a{}", i % 3), &serde_json::json!({"v": i}), None)
                .await?;
            s.touch_key(&key).await?;
            Ok::<_, StoreError>(())
        }));
    }
    for j in jobs {
        j.await.unwrap().unwrap();
    }
    let seqs: Vec<i64> = s
        .list_messages(&ses.id)
        .await
        .unwrap()
        .iter()
        .map(|m| m.seq)
        .collect();
    assert_eq!(
        seqs,
        (1..=24).collect::<Vec<_>>(),
        "message numbers are unique and gapless"
    );
}

pub async fn mcp_servers_round_trip(s: SharedStore) {
    s.put_mcp_server("fs", &serde_json::json!({"command": "x"}))
        .await
        .unwrap();
    s.put_mcp_server("fs", &serde_json::json!({"command": "y"}))
        .await
        .unwrap();
    assert_eq!(
        s.list_mcp_servers().await.unwrap(),
        vec![("fs".to_owned(), serde_json::json!({"command": "y"}))]
    );
    assert!(s.delete_mcp_server("fs").await.unwrap());
    assert!(s.list_mcp_servers().await.unwrap().is_empty());
}

pub async fn api_keys_lifecycle(s: SharedStore) {
    assert!(!s.has_keys().await.unwrap());
    let k = s
        .create_key(&NewKey::new(
            "ci",
            "gai_ab12",
            "hash1",
            &[Scope::Run],
            Some(5.0),
        ))
        .await
        .unwrap();
    assert!(s.has_keys().await.unwrap());
    assert_eq!(s.find_key("hash1").await.unwrap().unwrap().id, k.id);
    s.add_key_spend(&k.id, 1.5).await.unwrap();
    s.touch_key(&k.id).await.unwrap();
    let listed = &s.list_keys(None).await.unwrap()[0];
    assert_eq!(listed.org_id, DEFAULT_ORG);
    assert_eq!(listed.spent_usd, 1.5);
    assert!(listed.last_used_at.is_some());
    assert!(s.revoke_key(&k.id).await.unwrap());
    assert!(s.find_key("hash1").await.unwrap().is_none());
    assert!(!s.has_keys().await.unwrap());
    assert!(Scope::allows(&[Scope::Run], Scope::Read));
    assert!(!Scope::allows(&[Scope::Read], Scope::Run));
    assert!(Scope::allows(&[Scope::Admin], Scope::Run));
    assert!(Scope::allows(&[Scope::Build], Scope::Run));
    assert!(!Scope::allows(&[Scope::Build], Scope::Admin));
    assert_eq!(Role::Builder.scope(), Scope::Build);
}

pub async fn agents_are_versioned_and_labelled(s: SharedStore) {
    assert_eq!(
        s.put_agent("bot", &json!({"model": "a"}), None)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        s.put_agent("bot", &json!({"model": "a"}), None)
            .await
            .unwrap(),
        1,
        "unchanged spec"
    );
    assert_eq!(
        s.put_agent("bot", &json!({"model": "b"}), Some("swap"))
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        s.get_agent("bot", None).await.unwrap().unwrap().spec["model"],
        "b"
    );
    assert_eq!(
        s.get_agent("bot", Some(1)).await.unwrap().unwrap().spec["model"],
        "a"
    );
    s.set_agent_label("bot", "prod", Some(1)).await.unwrap();
    assert!(s.set_agent_label("bot", "prod", Some(9)).await.is_err());
    let list = s.list_agents().await.unwrap();
    assert_eq!(list[0].labels["prod"], 1);
    assert_eq!(s.list_agent_versions("bot").await.unwrap().len(), 2);
    assert!(s.delete_agent("bot").await.unwrap());
    assert!(s.get_agent("bot", None).await.unwrap().is_none());
}

pub async fn history_respects_compaction(s: SharedStore) {
    let ses = s
        .create_session(NewSession {
            agent: Some("bot".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let seqs = s
        .append_messages(
            &ses.id,
            None,
            &[
                Message::user("a"),
                Message::assistant("b"),
                Message::user("c"),
            ],
        )
        .await
        .unwrap();
    assert_eq!(seqs, [1, 2, 3]);
    s.add_compaction(&ses.id, None, 2, "a then b")
        .await
        .unwrap();
    s.append_messages(&ses.id, None, &[Message::assistant("d")])
        .await
        .unwrap();
    let h = s.load_history(&ses.id).await.unwrap();
    assert_eq!(h.summary.as_deref(), Some("a then b"));
    let texts: Vec<String> = h.messages.iter().map(|m| m.message.text()).collect();
    assert_eq!(texts, ["c", "d"]);
    assert_eq!(h.messages[1].seq, 5);
    assert_eq!(s.list_messages(&ses.id).await.unwrap().len(), 4);

    // A fork keeps rows before the cut, compactions included.
    let all = s
        .fork_session(&ses.id, None, NewSession::default())
        .await
        .unwrap();
    let h = s.load_history(&all.id).await.unwrap();
    assert_eq!(h.summary.as_deref(), Some("a then b"));
    assert_eq!(h.messages.len(), 2);
    let early = s
        .fork_session(&ses.id, Some(3), NewSession::default())
        .await
        .unwrap();
    let h = s.load_history(&early.id).await.unwrap();
    assert!(h.summary.is_none());
    assert_eq!(h.messages.len(), 2);
    let seqs = s
        .append_messages(&early.id, None, &[Message::user("e")])
        .await
        .unwrap();
    assert_eq!(seqs, [3]);
    assert!(
        s.fork_session("nope", None, NewSession::default())
            .await
            .is_err()
    );
}

pub async fn runs_steps_events_and_usage(s: SharedStore) {
    let ses = s.create_session(NewSession::default()).await.unwrap();
    let r = s
        .create_run(NewRun {
            session_id: ses.id.clone(),
            agent: "bot".into(),
            input: json!("hi"),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(r.status, RunStatus::Queued);
    s.update_run(
        &r.id,
        RunUpdate {
            status: Some(RunStatus::Running),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    s.start_step(
        &r.id,
        1,
        StepKind::Model,
        Some("mock/echo"),
        Some(&json!({"n": 1})),
    )
    .await
    .unwrap();
    let u = Usage {
        input_tokens: 3,
        output_tokens: 4,
        ..Default::default()
    };
    s.finish_step(
        &r.id,
        1,
        StepStatus::Completed,
        Some(&json!("ok")),
        Some(&u),
        Some(0.5),
    )
    .await
    .unwrap();
    let steps = s.list_steps(&r.id).await.unwrap();
    assert_eq!(
        (steps[0].kind, steps[0].status),
        (StepKind::Model, StepStatus::Completed)
    );
    for i in 0..3 {
        s.append_event(&r.id, i, &json!({"i": i})).await.unwrap();
    }
    assert_eq!(s.list_events(&r.id, Some(0), 10).await.unwrap().len(), 2);
    s.update_run(
        &r.id,
        RunUpdate {
            status: Some(RunStatus::Completed),
            output: Some(json!("done")),
            usage: Some(u),
            cost: Some(0.5),
            steps: Some(1),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let done = s.get_run(&r.id).await.unwrap().unwrap();
    assert!(done.finished_at.is_some());
    assert_eq!(done.usage, u);
    assert_eq!(
        s.list_runs(RunFilter {
            status: Some(RunStatus::Completed),
            ..Default::default()
        })
        .await
        .unwrap()
        .len(),
        1
    );
    s.record_usage(Some(&r.id), "m", &u, Some(0.25))
        .await
        .unwrap();
    s.record_usage(None, "m", &u, None).await.unwrap();
    let totals = s.usage_totals(None).await.unwrap();
    assert_eq!(
        (
            totals[0].calls,
            totals[0].usage.output_tokens,
            totals[0].cost
        ),
        (2, 8, 0.25)
    );
}

pub async fn blobs_round_trip(s: SharedStore) {
    let b = s
        .put_blob(NewBlob {
            media_type: "audio/wav".into(),
            name: Some("hi.wav".into()),
            run_id: Some("run_1".into()),
            source: Some("speak".into()),
            data: b"RIFF....".to_vec(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(b.size, 8);
    assert_eq!(b.sha256.len(), 64);
    assert_eq!(
        s.get_blob(&b.id).await.unwrap().unwrap().name.as_deref(),
        Some("hi.wav")
    );
    assert_eq!(s.blob_data(&b.id).await.unwrap().unwrap(), b"RIFF....");
    s.put_blob(NewBlob {
        media_type: "image/png".into(),
        data: vec![1],
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(s.list_blobs(None, 10).await.unwrap().len(), 2);
    assert_eq!(s.list_blobs(Some("run_1"), 10).await.unwrap().len(), 1);
    assert!(s.delete_blob(&b.id).await.unwrap());
    assert!(s.blob_data(&b.id).await.unwrap().is_none());
}

pub async fn audit_entries_filter_by_actor_target_and_time(s: SharedStore) {
    for (actor, target) in [
        ("ada", "agents/a"),
        ("ada", "keys/k1"),
        ("alan", "agents/b"),
    ] {
        s.add_audit(NewAudit {
            actor: actor.into(),
            actor_id: format!("key_{actor}"),
            action: "POST /v1/x".into(),
            target: target.into(),
            status: 200,
            detail: json!({"n": 1}),
        })
        .await
        .unwrap();
    }
    let all = s.list_audit(AuditFilter::default()).await.unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].actor, "alan", "newest first");
    let ada = s
        .list_audit(AuditFilter {
            actor: Some("ada".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(ada.len(), 2);
    let agents = s
        .list_audit(AuditFilter {
            target: Some("agents/".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(agents.len(), 2);
    let later = s
        .list_audit(AuditFilter {
            since: Some(Utc::now() + Duration::hours(1)),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(later.is_empty());
}

pub async fn pruning_and_forgetting(s: SharedStore) {
    let mk = |agent: &str, user: &str| NewSession {
        agent: Some(agent.into()),
        user_id: Some(user.into()),
        ..Default::default()
    };
    let old = s.create_session(mk("support", "ada")).await.unwrap();
    let keep = s.create_session(mk("other", "ada")).await.unwrap();
    let alan = s.create_session(mk("support", "alan")).await.unwrap();
    for ses in [&old, &keep, &alan] {
        let run = s
            .create_run(NewRun {
                session_id: ses.id.clone(),
                agent: ses.agent.clone().unwrap(),
                input: json!("hi"),
                ..Default::default()
            })
            .await
            .unwrap();
        s.put_blob(NewBlob {
            media_type: "text/plain".into(),
            name: None,
            run_id: Some(run.id.clone()),
            source: None,
            metadata: json!({}),
            data: b"x".to_vec(),
        })
        .await
        .unwrap();
        s.put_feedback(&run, "ada", 1, None, &[]).await.unwrap();
    }
    s.create_block(
        NewBlock {
            scope: "user:ada".into(),
            label: "human".into(),
            value: "likes tea".into(),
            limit: 100,
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(s.session_agents().await.unwrap(), ["other", "support"]);

    // Nothing is old enough yet.
    let none = s
        .prune_sessions("support", Utc::now() - Duration::days(1))
        .await
        .unwrap();
    assert_eq!(none, Deleted::default());
    let pruned = s
        .prune_sessions("support", Utc::now() + Duration::seconds(1))
        .await
        .unwrap();
    assert_eq!((pruned.sessions, pruned.runs, pruned.blobs), (2, 2, 2));
    assert!(s.get_session(&keep.id).await.unwrap().is_some());

    let gone = s.forget_user("ada").await.unwrap();
    assert_eq!(
        (gone.sessions, gone.runs, gone.blobs, gone.memory_blocks),
        (1, 1, 1, 1)
    );
    assert!(gone.feedback >= 1);
    assert!(s.get_session(&keep.id).await.unwrap().is_none());
    assert!(s.find_block("user:ada", "human").await.unwrap().is_none());
}

pub async fn blocks_are_versioned_and_guarded(s: SharedStore) {
    let new = NewBlock {
        scope: "agent:a".into(),
        label: "persona".into(),
        value: "I help.".into(),
        limit: 100,
        ..Default::default()
    };
    let b = s.ensure_block(new.clone()).await.unwrap();
    assert_eq!(s.ensure_block(new.clone()).await.unwrap().id, b.id);
    assert!(matches!(
        s.create_block(new, None).await,
        Err(StoreError::Conflict(_))
    ));
    let b2 = s
        .set_block_value(&b.id, "I help a lot.", Some(1), "append", Some("run_1"))
        .await
        .unwrap();
    assert_eq!((b2.version, b2.value.as_str()), (2, "I help a lot."));
    assert!(matches!(
        s.set_block_value(&b.id, "x", Some(1), "rethink", None)
            .await,
        Err(StoreError::Conflict(_))
    ));
    let h = s.block_history(&b.id).await.unwrap();
    assert_eq!(h.len(), 2);
    assert_eq!(
        (h[0].op.as_str(), h[0].actor.as_deref()),
        ("append", Some("run_1"))
    );
    let b3 = s
        .update_block_settings(
            &b.id,
            BlockSettings {
                read_only: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(b3.read_only);
    assert_eq!(s.list_blocks(Some("agent:a")).await.unwrap().len(), 1);
    assert!(s.delete_block(&b.id).await.unwrap());
    assert!(s.block_history(&b.id).await.unwrap().is_empty());
}

pub async fn documents_replace_by_source_and_search_both_ways(s: SharedStore) {
    s.create_collection(&coll("kb")).await.unwrap();
    s.create_collection(&coll("other")).await.unwrap();
    assert!(s.create_collection(&coll("kb")).await.is_err());
    s.put_document(
        doc("kb", "a.md"),
        vec![
            ch("The quick brown fox jumps", vec![1.0, 0.0, 0.0]),
            ch("Rust ownership and borrowing", vec![0.0, 1.0, 0.0]),
        ],
    )
    .await
    .unwrap();
    s.put_document(
        doc("other", "b.md"),
        vec![ch("Borrowing books from the library", vec![0.0, 0.9, 0.1])],
    )
    .await
    .unwrap();
    let kb = vec!["kb".to_owned()];

    let v = s.vector_search(&kb, &[0.0, 1.0, 0.0], 5).await.unwrap();
    assert_eq!(v.len(), 2);
    assert!(v[0].chunk.text.starts_with("Rust"));
    assert!(v[0].score > 0.99 && v[1].score < 0.01);
    assert_eq!(v[0].source, "a.md");

    let t = s.text_search(&kb, "borrowing OR \"fox", 5).await.unwrap();
    assert_eq!(t.len(), 2, "both chunks match one word each");
    let both = vec!["kb".to_owned(), "other".to_owned()];
    assert_eq!(s.text_search(&both, "borrowing", 5).await.unwrap().len(), 2);

    // Same source again: the old chunks (and their index rows) go.
    let d = s
        .put_document(
            doc("kb", "a.md"),
            vec![ch("Only foxes now", vec![1.0, 0.0, 0.0])],
        )
        .await
        .unwrap();
    assert_eq!(s.list_documents("kb", 10, 0).await.unwrap().len(), 1);
    assert!(s.text_search(&kb, "borrowing", 5).await.unwrap().is_empty());
    assert_eq!(s.document_chunks(&d.id).await.unwrap().len(), 1);
    let c = s.get_collection("kb").await.unwrap().unwrap();
    assert_eq!((c.documents, c.chunks), (1, 1));

    assert!(s.delete_collection("kb").await.unwrap());
    assert!(s.text_search(&kb, "foxes", 5).await.unwrap().is_empty());
    assert!(s.get_document(&d.id).await.unwrap().is_none());
}

pub async fn skills_version_on_change(s: SharedStore) {
    let files = BTreeMap::from([("a.py".to_owned(), "print(1)".to_owned())]);
    assert_eq!(s.put_skill("pdf", "d", "body", &files).await.unwrap(), 1);
    assert_eq!(s.put_skill("pdf", "d", "body", &files).await.unwrap(), 1);
    assert_eq!(s.put_skill("pdf", "d", "body 2", &files).await.unwrap(), 2);
    let sk = s.get_skill("pdf").await.unwrap().unwrap();
    assert_eq!((sk.version, sk.files["a.py"].as_str()), (2, "print(1)"));
    assert_eq!(s.list_skills().await.unwrap().len(), 1);
    assert!(s.delete_skill("pdf").await.unwrap());
}

pub async fn personas_are_versioned(s: SharedStore) {
    let a = json!({"name": "w", "description": "first", "avatar": "🦉"});
    assert_eq!(s.put_persona("w", &a, None).await.unwrap(), 1);
    assert_eq!(s.put_persona("w", &a, None).await.unwrap(), 1);
    let b = json!({"name": "w", "description": "second", "avatar": "🦉"});
    assert_eq!(s.put_persona("w", &b, Some("edit")).await.unwrap(), 2);
    let list = s.list_personas().await.unwrap();
    assert_eq!(
        (
            list[0].latest_version,
            list[0].description.as_deref(),
            list[0].avatar.as_deref()
        ),
        (2, Some("second"), Some("🦉"))
    );
    assert_eq!(s.get_persona("w", Some(1)).await.unwrap().unwrap().spec, a);
    assert_eq!(s.get_persona("w", None).await.unwrap().unwrap().version, 2);
    assert_eq!(s.list_persona_versions("w").await.unwrap().len(), 2);
    s.set_persona_label("w", "prod", Some(1)).await.unwrap();
    assert!(s.set_persona_label("w", "prod", Some(9)).await.is_err());
    assert_eq!(s.list_personas().await.unwrap()[0].labels["prod"], 1);
    s.set_persona_label("w", "prod", None).await.unwrap();
    assert!(s.list_personas().await.unwrap()[0].labels.is_empty());
    assert!(s.delete_persona("w").await.unwrap());
    assert!(s.get_persona("w", None).await.unwrap().is_none());
}

pub async fn feedback_is_one_per_user_and_counted(s: SharedStore) {
    let ses = s.create_session(NewSession::default()).await.unwrap();
    let mk = |agent: &str| NewRun {
        session_id: ses.id.clone(),
        agent: agent.into(),
        agent_version: Some(2),
        input: json!("hi"),
        ..Default::default()
    };
    let a = s.create_run(mk("helper")).await.unwrap();
    let b = s.create_run(mk("helper")).await.unwrap();
    s.put_feedback(&a, "ada", 1, None, &[]).await.unwrap();
    // Rating again replaces.
    let f = s
        .put_feedback(&a, "ada", -1, Some("wrong"), &["wrong".into()])
        .await
        .unwrap();
    assert_eq!((f.rating, f.comment.as_deref()), (-1, Some("wrong")));
    s.put_feedback(&a, "alan", 1, None, &[]).await.unwrap();
    s.put_feedback(&b, "ada", 5, None, &[]).await.unwrap();
    assert!(s.put_feedback(&b, "x", 0, None, &[]).await.is_err());
    let sum = s.feedback_summary(Some("helper")).await.unwrap();
    assert_eq!(
        sum,
        [FeedbackSummary {
            agent: "helper".into(),
            agent_version: Some(2),
            good: 2,
            bad: 1
        }]
    );
    let bad = s
        .list_feedback(FeedbackFilter {
            rating: Some(-1),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(bad.len(), 1);
    assert_eq!(bad[0].tags, ["wrong"]);
    assert!(s.delete_feedback(&a.id, "ada").await.unwrap());
    assert_eq!(
        s.list_feedback(FeedbackFilter {
            run_id: Some(a.id.clone()),
            ..Default::default()
        })
        .await
        .unwrap()
        .len(),
        1
    );
}

pub async fn eval_sets_runs_and_results(s: SharedStore) {
    let spec = json!({"name": "maths", "description": "Sums", "cases": [{"id": "a"}, {"id": "b"}]});
    assert_eq!(s.put_eval_set("maths", &spec, None).await.unwrap(), 1);
    assert_eq!(s.put_eval_set("maths", &spec, None).await.unwrap(), 1);
    let mut spec2 = spec.clone();
    spec2["cases"] = json!([{"id": "a"}]);
    assert_eq!(
        s.put_eval_set("maths", &spec2, Some("fewer"))
            .await
            .unwrap(),
        2
    );
    let list = s.list_eval_sets().await.unwrap();
    assert_eq!((list[0].cases, list[0].latest_version), (1, 2));
    assert_eq!(
        s.get_eval_set("maths", Some(1))
            .await
            .unwrap()
            .unwrap()
            .spec,
        spec
    );

    let run = s
        .create_eval_run("maths", 2, &json!({"agent": "helper"}))
        .await
        .unwrap();
    assert_eq!(run.status, "running");
    for (case, score) in [("a", 1.0), ("b", 0.0)] {
        s.put_eval_result(&EvalResult {
            eval_run_id: run.id.clone(),
            case_id: case.into(),
            run_id: None,
            output: Some("4".into()),
            scores: json!([]),
            score,
            passed: score > 0.5,
            cost: 0.01,
            latency_ms: 10,
            error: None,
        })
        .await
        .unwrap();
    }
    s.finish_eval_run(&run.id, "completed", &json!({"score": 0.5}), None)
        .await
        .unwrap();
    let got = s.get_eval_run(&run.id).await.unwrap().unwrap();
    assert_eq!(
        (got.status.as_str(), &got.summary["score"]),
        ("completed", &json!(0.5))
    );
    let res = s.list_eval_results(&run.id).await.unwrap();
    assert_eq!(
        res.iter().map(|r| r.case_id.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert!(res[0].passed && !res[1].passed);
    assert_eq!(s.list_eval_runs(Some("maths"), 0).await.unwrap().len(), 1);
    assert!(s.delete_eval_set("maths").await.unwrap());
}

pub async fn triggers_fire_once_per_due_time(s: SharedStore) {
    let t = s
        .create_trigger("nightly", "cron", &json!({"schedule": "0 2 * * *"}), None)
        .await
        .unwrap();
    assert!(t.enabled && t.next_fire_at.is_none());
    assert!(
        s.create_trigger("nightly", "cron", &json!({}), None)
            .await
            .is_err()
    );
    let due = Utc::now();
    s.set_trigger_next(&t.id, Some(due)).await.unwrap();
    let next = due + chrono::Duration::days(1);
    // Two schedulers race for the same firing: only one wins.
    assert!(
        s.advance_trigger(&t.id, due, Some(next), due)
            .await
            .unwrap()
    );
    assert!(
        !s.advance_trigger(&t.id, due, Some(next), due)
            .await
            .unwrap()
    );
    let got = s.get_trigger("nightly").await.unwrap().unwrap();
    // Times are kept to the microsecond (Postgres' precision).
    assert_eq!(
        got.next_fire_at.map(|t| t.timestamp_micros()),
        Some(next.timestamp_micros())
    );
    assert!(got.last_fired_at.is_some());

    s.add_trigger_event(NewTriggerEvent {
        trigger_id: t.id.clone(),
        reason: "schedule".into(),
        payload: Some(json!({"n": 1})),
        run_id: Some("run_x".into()),
        status: "started".into(),
        error: None,
    })
    .await
    .unwrap();
    let events = s.list_trigger_events(&t.id, 10).await.unwrap();
    assert_eq!(events[0].payload, Some(json!({"n": 1})));

    let renamed = s
        .update_trigger(
            &t.id,
            "nightly-2",
            "cron",
            &json!({"schedule": "0 3 * * *"}),
        )
        .await
        .unwrap();
    assert_eq!(renamed.name, "nightly-2");
    s.set_trigger_enabled(&t.id, false).await.unwrap();
    assert!(!s.get_trigger(&t.id).await.unwrap().unwrap().enabled);
    assert!(s.delete_trigger(&t.id).await.unwrap());
    assert!(s.list_trigger_events(&t.id, 10).await.unwrap().is_empty());
}

pub async fn workflows_are_versioned(s: SharedStore) {
    let a = json!({"name": "w", "description": "first", "nodes": []});
    assert_eq!(s.put_workflow("w", &a, None).await.unwrap(), 1);
    assert_eq!(s.put_workflow("w", &a, None).await.unwrap(), 1);
    let b = json!({"name": "w", "description": "second", "nodes": []});
    assert_eq!(s.put_workflow("w", &b, Some("edit")).await.unwrap(), 2);
    let list = s.list_workflows().await.unwrap();
    assert_eq!(
        (list[0].latest_version, list[0].description.as_deref()),
        (2, Some("second"))
    );
    assert_eq!(s.get_workflow("w", Some(1)).await.unwrap().unwrap().spec, a);
    assert_eq!(s.get_workflow("w", None).await.unwrap().unwrap().version, 2);
    assert_eq!(s.list_workflow_versions("w").await.unwrap().len(), 2);
    s.set_workflow_label("w", "prod", Some(1)).await.unwrap();
    assert!(s.set_workflow_label("w", "prod", Some(9)).await.is_err());
    assert_eq!(s.list_workflows().await.unwrap()[0].labels["prod"], 1);
    s.set_workflow_label("w", "prod", None).await.unwrap();
    assert!(s.list_workflows().await.unwrap()[0].labels.is_empty());
    assert!(s.delete_workflow("w").await.unwrap());
    assert!(s.get_workflow("w", None).await.unwrap().is_none());
}

pub async fn dashboards(s: SharedStore) {
    s.put_dashboard("growth", &json!({"title": "Growth"}))
        .await
        .unwrap();
    let d = s.get_dashboard("growth").await.unwrap().unwrap();
    assert_eq!(
        (d.spec["title"].clone(), d.results),
        (json!("Growth"), None)
    );
    s.save_dashboard_results("growth", &json!({"panels": [1]}))
        .await
        .unwrap();
    s.put_dashboard("growth", &json!({"title": "Growth 2"}))
        .await
        .unwrap();
    let d = s.get_dashboard("growth").await.unwrap().unwrap();
    assert_eq!(d.spec["title"], "Growth 2");
    assert_eq!(
        d.results.unwrap()["panels"][0],
        1,
        "results survive a new spec"
    );
    assert!(d.refreshed_at.is_some());
    s.put_dashboard("ads", &json!({})).await.unwrap();
    assert_eq!(s.list_dashboards().await.unwrap().len(), 2);
    assert!(s.save_dashboard_results("nope", &json!({})).await.is_err());
    assert!(s.delete_dashboard("ads").await.unwrap());
    assert!(!s.delete_dashboard("ads").await.unwrap());
}

pub async fn apps(s: SharedStore) {
    s.put_app("intake", &json!({"title": "Intake"}))
        .await
        .unwrap();
    let a = s.get_app("intake").await.unwrap().unwrap();
    assert_eq!(
        (a.spec["title"].clone(), a.key_hash),
        (json!("Intake"), None)
    );
    s.set_app_key("intake", Some(("hash1", "gai_app_ab")))
        .await
        .unwrap();
    s.put_app("intake", &json!({"title": "Intake form"}))
        .await
        .unwrap();
    let a = s.get_app("intake").await.unwrap().unwrap();
    assert_eq!(a.spec["title"], "Intake form");
    assert_eq!(
        (a.key_hash.as_deref(), a.key_prefix.as_deref()),
        (Some("hash1"), Some("gai_app_ab")),
        "a new spec keeps the key"
    );
    s.put_app("brief", &json!({})).await.unwrap();
    let names: Vec<String> = s
        .list_apps()
        .await
        .unwrap()
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert_eq!(names, ["brief", "intake"]);
    s.set_app_key("intake", None).await.unwrap();
    assert_eq!(s.get_app("intake").await.unwrap().unwrap().key_hash, None);
    assert!(s.set_app_key("nope", None).await.is_err());
    assert!(s.delete_app("brief").await.unwrap());
    assert!(!s.delete_app("brief").await.unwrap());
}

pub async fn recordings(s: SharedStore) {
    let r = s
        .create_recording(NewRecording {
            kind: "meeting".into(),
            title: "Standup".into(),
            blob_id: Some("blob_1".into()),
            options: json!({"template": "standup"}),
        })
        .await
        .unwrap();
    assert_eq!((r.status.as_str(), r.result.clone()), ("processing", None));
    let got = s.get_recording(&r.id).await.unwrap().unwrap();
    assert_eq!(got.options["template"], "standup");
    s.finish_recording(&r.id, "done", Some(&json!({"summary": "ok"})), None)
        .await
        .unwrap();
    let got = s.get_recording(&r.id).await.unwrap().unwrap();
    assert_eq!(
        (got.status.as_str(), got.result.unwrap()["summary"].clone()),
        ("done", json!("ok"))
    );
    let second = s
        .create_recording(NewRecording {
            kind: "video".into(),
            title: "Demo".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    s.finish_recording(&second.id, "failed", None, Some("no audio"))
        .await
        .unwrap();
    let list = s.list_recordings(10).await.unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id, second.id, "newest first");
    assert_eq!(list[0].error.as_deref(), Some("no audio"));
    assert!(
        s.finish_recording("rec_nope", "done", None, None)
            .await
            .is_err()
    );
    assert!(s.delete_recording(&r.id).await.unwrap());
    assert!(!s.delete_recording(&r.id).await.unwrap());
}

pub async fn data_connections(s: SharedStore) {
    let spec = json!({"kind": "sqlite", "path": "a.db"});
    s.put_connection("sales", "sqlite", &spec).await.unwrap();
    let c = s.get_connection("sales").await.unwrap().unwrap();
    assert_eq!(
        (c.kind.as_str(), c.spec.clone(), c.schema.clone()),
        ("sqlite", spec.clone(), None)
    );
    s.save_connection_schema("sales", &json!({"tables": [{"name": "orders"}]}))
        .await
        .unwrap();
    let c = s.get_connection("sales").await.unwrap().unwrap();
    assert_eq!(c.schema.unwrap()["tables"][0]["name"], "orders");
    assert!(c.schema_at.is_some());
    // A new spec forgets the old schema.
    s.put_connection("sales", "postgres", &json!({"kind": "postgres"}))
        .await
        .unwrap();
    let c = s.get_connection("sales").await.unwrap().unwrap();
    assert_eq!((c.kind.as_str(), c.schema), ("postgres", None));
    s.put_connection("ads", "bigquery", &json!({}))
        .await
        .unwrap();
    let names: Vec<String> = s
        .list_connections()
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(names, ["ads", "sales"]);
    assert!(s.save_connection_schema("nope", &json!({})).await.is_err());
    assert!(s.delete_connection("ads").await.unwrap());
    assert!(!s.delete_connection("ads").await.unwrap());
}

pub async fn knowledge_graph(s: SharedStore) {
    s.create_collection(&coll("kb")).await.unwrap();
    let doc = |source: &str, texts: &[&str]| {
        let s = s.clone();
        let source = source.to_owned();
        let texts: Vec<String> = texts.iter().map(|t| t.to_string()).collect();
        async move {
            s.put_document(
                NewDocument {
                    collection: "kb".into(),
                    source,
                    title: None,
                    content_hash: "h".into(),
                    chars: 10,
                    metadata: json!({}),
                },
                texts
                    .into_iter()
                    .map(|t| NewChunk {
                        text: t,
                        embedding: None,
                        metadata: json!({}),
                    })
                    .collect(),
            )
            .await
            .unwrap()
        }
    };
    doc(
        "a.md",
        &["Acme signed with Globex.", "Ada Lovelace runs Acme."],
    )
    .await;
    let pending = s.graph_pending("kb", 10).await.unwrap();
    assert_eq!(pending.len(), 2);
    let (c1, c2) = (pending[0].id, pending[1].id);
    let ent = |name: &str, kind: &str, chunks: &[i64]| NewEntity {
        name: name.into(),
        kind: kind.into(),
        description: None,
        chunk_ids: chunks.to_vec(),
    };
    let rel = |a: &str, b: &str, r: &str, c: i64| NewRelation {
        source: a.into(),
        target: b.into(),
        relation: r.into(),
        chunk_id: c,
    };
    s.graph_add(
        "kb",
        &[c1],
        &[
            ent("Acme", "organization", &[c1]),
            ent("Globex", "organization", &[c1]),
        ],
        &[
            rel("Acme", "Globex", "Signed with", c1),
            rel("Acme", "acme", "is", c1),
        ],
    )
    .await
    .unwrap();
    s.graph_add(
        "kb",
        &[c2],
        &[
            ent("ada  lovelace", "person", &[c2]),
            ent("ACME", "other", &[c2]),
        ],
        &[
            rel("Ada Lovelace", "Acme", "runs", c2),
            rel("Ada Lovelace", "Analytical Engine", "built", c2),
        ],
    )
    .await
    .unwrap();
    assert!(s.graph_pending("kb", 10).await.unwrap().is_empty());

    let nodes = s.graph_nodes("kb", "", 50).await.unwrap();
    let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(nodes.len(), 4, "{names:?}");
    // Merged by name; the first kind that isn't `other` wins.
    let acme = nodes.iter().find(|n| n.name == "Acme").unwrap();
    assert_eq!((acme.kind.as_str(), acme.mentions), ("organization", 2));
    assert_eq!(nodes[0].id, acme.id, "most mentioned first");
    assert_eq!(
        s.graph_nodes("kb", "LOVE", 10).await.unwrap()[0].kind,
        "person"
    );
    let engine = nodes
        .iter()
        .find(|n| n.name == "Analytical Engine")
        .unwrap();
    assert_eq!(engine.kind, "other");

    let edges = s.graph_edges("kb", &[], 50).await.unwrap();
    assert_eq!(edges.len(), 3, "self-relations are dropped: {edges:?}");
    assert!(
        edges
            .iter()
            .any(|e| e.relation == "signed with" && e.source == acme.id)
    );
    let around = s.graph_edges("kb", &[engine.id], 50).await.unwrap();
    assert_eq!(around.len(), 1);
    let chunks = s.graph_chunks(&[acme.id], 10).await.unwrap();
    assert_eq!(chunks.len(), 2);
    assert_eq!(s.graph_chunk_nodes(&[c1]).await.unwrap().len(), 2);
    assert_eq!(
        s.graph_nodes_by_id(&[engine.id]).await.unwrap()[0].name,
        "Analytical Engine"
    );
    let stats = s.graph_stats("kb").await.unwrap();
    assert_eq!(
        (
            stats.nodes,
            stats.edges,
            stats.chunks_done,
            stats.chunks_pending
        ),
        (4, 3, 2, 0)
    );

    // Replacing the document drops what was learned from it.
    doc("a.md", &["Nothing to see."]).await;
    assert!(s.graph_nodes("kb", "", 50).await.unwrap().is_empty());
    assert!(s.graph_edges("kb", &[], 50).await.unwrap().is_empty());
    assert_eq!(s.graph_stats("kb").await.unwrap().chunks_pending, 1);
    let c3 = s.graph_pending("kb", 10).await.unwrap()[0].id;
    s.graph_add("kb", &[c3], &[ent("Nothing", "concept", &[c3])], &[])
        .await
        .unwrap();
    s.graph_clear("kb").await.unwrap();
    assert_eq!(
        s.graph_stats("kb").await.unwrap(),
        GraphStats {
            chunks_pending: 1,
            ..Default::default()
        }
    );
}

pub async fn agent_stats_per_version(s: SharedStore) {
    let ses = s.create_session(NewSession::default()).await.unwrap();
    let run = |agent: &str, version: u32, status: RunStatus, kind: Option<&str>| {
        let s = s.clone();
        let ses = ses.id.clone();
        let agent = agent.to_owned();
        let kind = kind.map(str::to_owned);
        async move {
            let r = s
                .create_run(NewRun {
                    session_id: ses,
                    agent,
                    agent_version: Some(version),
                    input: json!("hi"),
                    metadata: kind.map(|k| json!({"kind": k})).unwrap_or(json!({})),
                    ..Default::default()
                })
                .await
                .unwrap();
            s.update_run(
                &r.id,
                RunUpdate {
                    status: Some(status),
                    steps: Some(2),
                    cost: Some(0.5),
                    usage: Some(Usage {
                        input_tokens: 10,
                        output_tokens: 5,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            r.id
        }
    };
    let good = run("helper", 1, RunStatus::Completed, None).await;
    run("helper", 1, RunStatus::Failed, None).await;
    run("helper", 1, RunStatus::Running, None).await;
    run("helper", 2, RunStatus::Completed, None).await;
    run("helper", 2, RunStatus::Completed, Some("eval")).await;
    run("other", 1, RunStatus::Cancelled, None).await;
    let good = s.get_run(&good).await.unwrap().unwrap();
    s.put_feedback(&good, "ada", 1, None, &[]).await.unwrap();

    let stats = s.agent_stats(StatsFilter::default()).await.unwrap();
    assert_eq!(stats.len(), 3, "{stats:?}");
    let v1 = &stats[0];
    assert_eq!((v1.agent.as_str(), v1.agent_version), ("helper", Some(1)));
    assert_eq!((v1.runs, v1.completed, v1.failed, v1.open), (3, 1, 1, 1));
    assert_eq!(v1.success_rate, 0.5);
    assert_eq!(v1.avg_steps, 2.0);
    assert!((v1.cost_usd - 1.5).abs() < 1e-9);
    assert_eq!((v1.input_tokens, v1.output_tokens), (30, 15));
    assert_eq!((v1.good, v1.bad), (1, 0));
    assert!(v1.last_run_at.is_some());
    // Evaluation runs don't count.
    let v2 = stats.iter().find(|x| x.agent_version == Some(2)).unwrap();
    assert_eq!(v2.runs, 1);
    let other = stats.iter().find(|x| x.agent == "other").unwrap();
    assert_eq!((other.stopped, other.success_rate), (1, 0.0));
    let only = s
        .agent_stats(StatsFilter {
            agent: Some("other".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(only.len(), 1);
    let later = s
        .agent_stats(StatsFilter {
            since: Some(Utc::now() + Duration::hours(1)),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(later.is_empty());
}

pub async fn conversations_are_searchable(s: SharedStore) {
    let a = s
        .create_session(NewSession {
            agent: Some("helper".into()),
            user_id: Some("ada".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let b = s
        .create_session(NewSession {
            agent: Some("other".into()),
            user_id: Some("bob".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    s.append_messages(
        &a.id,
        None,
        &[
            Message::user("How do I reset my router password?"),
            Message::assistant("Hold the reset button for ten seconds."),
        ],
    )
    .await
    .unwrap();
    s.append_messages(
        &b.id,
        None,
        &[Message::user("Passwords for the wiki are in the vault")],
    )
    .await
    .unwrap();
    s.add_compaction(&a.id, None, 1, "router talk")
        .await
        .unwrap();

    let find = |text: &str| MessageSearch {
        text: text.into(),
        ..Default::default()
    };
    let hits = s.search_messages(&find("router password")).await.unwrap();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].session_id, a.id);
    assert_eq!(hits[0].seq, 1);
    assert_eq!(hits[0].role, "user");
    assert_eq!(hits[0].agent.as_deref(), Some("helper"));
    assert!(hits[0].snippet.contains("[router]"), "{}", hits[0].snippet);
    // Stemmed, and the last word may be partial.
    assert_eq!(
        s.search_messages(&find("passwords")).await.unwrap().len(),
        2
    );
    assert_eq!(s.search_messages(&find("rese")).await.unwrap().len(), 2);
    let mut only_bob = find("password");
    only_bob.user_id = Some("bob".into());
    assert_eq!(
        s.search_messages(&only_bob).await.unwrap()[0].session_id,
        b.id
    );
    let mut only_helper = find("password");
    only_helper.agent = Some("helper".into());
    assert_eq!(s.search_messages(&only_helper).await.unwrap().len(), 1);
    let mut later = find("password");
    later.since = Some(Utc::now() + Duration::hours(1));
    assert!(s.search_messages(&later).await.unwrap().is_empty());
    assert!(s.search_messages(&find("  ?! ")).await.unwrap().is_empty());
    // Forks are searchable; deleted conversations aren't.
    let fork = s
        .fork_session(&a.id, None, NewSession::default())
        .await
        .unwrap();
    assert_eq!(
        s.search_messages(&find("ten seconds")).await.unwrap().len(),
        2
    );
    assert!(s.delete_session(&fork.id).await.unwrap());
    assert!(s.delete_session(&b.id).await.unwrap());
    assert_eq!(s.search_messages(&find("password")).await.unwrap().len(), 1);
}

pub async fn directory_orgs_users_and_teams(s: SharedStore) {
    // The default organization is always there and can't be deleted.
    assert_eq!(
        s.get_org(DEFAULT_ORG).await.unwrap().unwrap().slug,
        "default"
    );
    assert!(s.delete_org(DEFAULT_ORG).await.is_err());
    let acme = s.create_org("acme", "Acme").await.unwrap();
    assert!(matches!(
        s.create_org("acme", "Again").await,
        Err(StoreError::Conflict(_))
    ));
    assert_eq!(s.get_org("acme").await.unwrap().unwrap().id, acme.id);
    let acme = s
        .update_org(
            &acme.id,
            None,
            Some(&json!({"branding": {"name": "Acme AI"}})),
        )
        .await
        .unwrap();
    assert_eq!(acme.name, "Acme");
    assert_eq!(acme.settings["branding"]["name"], "Acme AI");
    assert_eq!(s.list_orgs().await.unwrap().len(), 2);

    // An invitation, then the first sign-in claims it by email.
    let ada = s.create_user("Ada@Example.com", Some("Ada")).await.unwrap();
    assert_eq!(ada.email, "ada@example.com");
    assert_eq!(
        s.create_user("ada@example.com", None).await.unwrap().id,
        ada.id
    );
    let signed = s
        .sign_in_user("https://idp|1", "ada@example.com", None)
        .await
        .unwrap();
    assert_eq!(signed.id, ada.id);
    assert_eq!(signed.name.as_deref(), Some("Ada"));
    assert!(signed.last_seen_at.is_some());
    let again = s
        .sign_in_user("https://idp|1", "ada@example.com", Some("Ada L"))
        .await
        .unwrap();
    assert_eq!(again.id, ada.id);
    // Another identity can't take an email that is already claimed.
    assert!(matches!(
        s.sign_in_user("https://idp|2", "ADA@example.com", None)
            .await,
        Err(StoreError::Conflict(_))
    ));
    let bob = s
        .sign_in_user("https://idp|3", "bob@example.com", Some("Bob"))
        .await
        .unwrap();
    assert_ne!(bob.id, ada.id);
    assert_eq!(
        s.find_user_by_email("BOB@example.com")
            .await
            .unwrap()
            .unwrap()
            .id,
        bob.id
    );
    assert_eq!(s.list_users().await.unwrap().len(), 2);

    s.set_membership(&acme.id, &ada.id, Role::Owner)
        .await
        .unwrap();
    s.set_membership(&acme.id, &bob.id, Role::Member)
        .await
        .unwrap();
    s.set_membership(&acme.id, &bob.id, Role::Builder)
        .await
        .unwrap();
    s.set_membership(DEFAULT_ORG, &bob.id, Role::Viewer)
        .await
        .unwrap();
    let members = s.list_members(&acme.id).await.unwrap();
    assert_eq!(
        members
            .iter()
            .map(|m| (m.email.as_str(), m.role))
            .collect::<Vec<_>>(),
        [
            ("ada@example.com", Role::Owner),
            ("bob@example.com", Role::Builder)
        ]
    );
    let bobs = s.user_memberships(&bob.id).await.unwrap();
    assert_eq!(bobs.len(), 2);
    assert_eq!(bobs[0].org_slug, "default");
    assert_eq!(bobs[1].org_name, "Acme");

    let eng = s.create_team(&acme.id, "eng").await.unwrap();
    assert!(matches!(
        s.create_team(&acme.id, "eng").await,
        Err(StoreError::Conflict(_))
    ));
    s.create_team(DEFAULT_ORG, "eng").await.unwrap();
    s.add_team_member(&eng.id, &bob.id).await.unwrap();
    s.add_team_member(&eng.id, &bob.id).await.unwrap();
    assert_eq!(
        s.get_team(&acme.id, "eng").await.unwrap().unwrap().members,
        std::slice::from_ref(&bob.id)
    );
    assert!(s.get_team(DEFAULT_ORG, &eng.id).await.unwrap().is_none());
    assert_eq!(s.list_teams(&acme.id).await.unwrap().len(), 1);

    s.set_owner(&acme.id, "agent", "helper", Some(&eng.id))
        .await
        .unwrap();
    assert_eq!(
        s.get_owner(&acme.id, "agent", "helper").await.unwrap(),
        Some(eng.id.clone())
    );
    assert_eq!(
        s.get_owner(DEFAULT_ORG, "agent", "helper").await.unwrap(),
        None
    );
    assert_eq!(
        s.list_owners(&acme.id).await.unwrap(),
        [("agent".to_owned(), "helper".to_owned(), eng.id.clone())]
    );
    // Only the organization's own teams.
    let other = s.list_teams(DEFAULT_ORG).await.unwrap()[0].id.clone();
    assert!(
        s.set_owner(&acme.id, "agent", "x", Some(&other))
            .await
            .is_err()
    );
    s.set_owner(&acme.id, "agent", "helper", None)
        .await
        .unwrap();
    assert_eq!(
        s.get_owner(&acme.id, "agent", "helper").await.unwrap(),
        None
    );
    s.set_owner(&acme.id, "collection", "docs", Some(&eng.id))
        .await
        .unwrap();

    // Leaving the organization leaves its teams.
    assert!(s.remove_membership(&acme.id, &bob.id).await.unwrap());
    assert!(
        s.get_team(&acme.id, "eng")
            .await
            .unwrap()
            .unwrap()
            .members
            .is_empty()
    );
    assert!(!s.remove_membership(&acme.id, &bob.id).await.unwrap());

    // Keys belong to an organization; a deleted team's keys stop working.
    let mut k = NewKey::new("ci", "gai_x", "hash-acme", &[Scope::Run], None);
    k.org_id = acme.id.clone();
    k.team_id = Some(eng.id.clone());
    let key = s.create_key(&k).await.unwrap();
    assert_eq!(key.team_id.as_deref(), Some(eng.id.as_str()));
    assert_eq!(s.list_keys(Some(&acme.id)).await.unwrap().len(), 1);
    assert!(s.list_keys(Some(DEFAULT_ORG)).await.unwrap().is_empty());
    assert!(s.delete_team(&acme.id, &eng.id).await.unwrap());
    assert!(s.find_key("hash-acme").await.unwrap().is_none());
    assert!(s.list_owners(&acme.id).await.unwrap().is_empty());

    // Sign-ins expire, and end when the user is disabled.
    let later = Utc::now() + Duration::hours(1);
    s.create_login("login1", &ada.id, later).await.unwrap();
    s.create_login("old", &ada.id, Utc::now() - Duration::seconds(1))
        .await
        .unwrap();
    assert_eq!(s.find_login("login1").await.unwrap().unwrap().1.id, ada.id);
    assert!(s.find_login("old").await.unwrap().is_none());
    assert!(s.find_login("nope").await.unwrap().is_none());
    assert!(s.set_user_disabled(&ada.id, true).await.unwrap());
    assert!(s.find_login("login1").await.unwrap().is_none());
    s.set_user_disabled(&ada.id, false).await.unwrap();
    s.create_login("login2", &ada.id, later).await.unwrap();
    s.delete_login("login2").await.unwrap();
    assert!(s.find_login("login2").await.unwrap().is_none());

    let mut k = NewKey::new("ci2", "gai_y", "hash-acme2", &[Scope::Run], None);
    k.org_id = acme.id.clone();
    s.create_key(&k).await.unwrap();
    assert!(s.delete_org(&acme.id).await.unwrap());
    assert!(s.get_org("acme").await.unwrap().is_none());
    assert!(s.find_key("hash-acme2").await.unwrap().is_none());
    assert_eq!(s.user_memberships(&ada.id).await.unwrap().len(), 0);
}

pub async fn sources_keep_their_sync_state(s: SharedStore) {
    let spec = json!({"kind": "folder", "path": "/docs"});
    s.put_source("docs", "handbook", "folder", &spec)
        .await
        .unwrap();
    let got = s.get_source("docs").await.unwrap().unwrap();
    assert_eq!((got.status.as_str(), got.last_sync_at), ("new", None));
    s.save_source_sync("docs", "syncing", None, &json!({}), None)
        .await
        .unwrap();
    assert!(
        s.get_source("docs")
            .await
            .unwrap()
            .unwrap()
            .last_sync_at
            .is_none()
    );
    let state = json!({"a.md": "v1"});
    s.save_source_sync("docs", "ok", None, &json!({"added": 1}), Some(&state))
        .await
        .unwrap();
    // Changing the spec keeps the state.
    s.put_source(
        "docs",
        "handbook",
        "folder",
        &json!({"kind": "folder", "path": "/docs2"}),
    )
    .await
    .unwrap();
    let got = s.get_source("docs").await.unwrap().unwrap();
    assert_eq!(got.state, state);
    assert_eq!(got.stats["added"], 1);
    assert!(got.last_sync_at.is_some());
    assert_eq!(got.spec["path"], "/docs2");
    s.save_source_sync("docs", "failed", Some("boom"), &json!({}), None)
        .await
        .unwrap();
    let got = s.get_source("docs").await.unwrap().unwrap();
    assert_eq!(
        (got.status.as_str(), got.error.as_deref()),
        ("failed", Some("boom"))
    );
    assert_eq!(got.state, state, "a failed sync keeps the last state");
    assert!(
        s.save_source_sync("nope", "ok", None, &json!({}), None)
            .await
            .is_err()
    );
    assert_eq!(s.list_sources().await.unwrap().len(), 1);
    assert!(s.delete_source("docs").await.unwrap());
    assert!(!s.delete_source("docs").await.unwrap());
}

/// One `#[tokio::test]` per conformance check, each on a fresh store from
/// `$make` (an async block giving `Option<SharedStore>`; `None` skips).
#[macro_export]
macro_rules! conformance_tests {
    ($make:expr) => {
        #[tokio::test]
        async fn dashboards() {
            if let Some(s) = $make.await {
                $crate::conformance::dashboards(s).await;
            }
        }
        #[tokio::test]
        async fn apps() {
            if let Some(s) = $make.await {
                $crate::conformance::apps(s).await;
            }
        }
        #[tokio::test]
        async fn recordings() {
            if let Some(s) = $make.await {
                $crate::conformance::recordings(s).await;
            }
        }
        #[tokio::test]
        async fn data_connections() {
            if let Some(s) = $make.await {
                $crate::conformance::data_connections(s).await;
            }
        }
        #[tokio::test]
        async fn knowledge_graph() {
            if let Some(s) = $make.await {
                $crate::conformance::knowledge_graph(s).await;
            }
        }
        #[tokio::test]
        async fn agent_stats_per_version() {
            if let Some(s) = $make.await {
                $crate::conformance::agent_stats_per_version(s).await;
            }
        }
        #[tokio::test]
        async fn conversations_are_searchable() {
            if let Some(s) = $make.await {
                $crate::conformance::conversations_are_searchable(s).await;
            }
        }
        #[tokio::test]
        async fn directory_orgs_users_and_teams() {
            if let Some(s) = $make.await {
                $crate::conformance::directory_orgs_users_and_teams(s).await;
            }
        }
        #[tokio::test]
        async fn sources_keep_their_sync_state() {
            if let Some(s) = $make.await {
                $crate::conformance::sources_keep_their_sync_state(s).await;
            }
        }
        #[tokio::test]
        async fn agents() {
            if let Some(s) = $make.await {
                $crate::conformance::agents(s).await;
            }
        }
        #[tokio::test]
        async fn settings() {
            if let Some(s) = $make.await {
                $crate::conformance::settings(s).await;
            }
        }
        #[tokio::test]
        async fn mcp() {
            if let Some(s) = $make.await {
                $crate::conformance::mcp(s).await;
            }
        }
        #[tokio::test]
        async fn settings_round_trip() {
            if let Some(s) = $make.await {
                $crate::conformance::settings_round_trip(s).await;
            }
        }
        #[tokio::test]
        async fn concurrent_writers_wait_instead_of_failing() {
            if let Some(s) = $make.await {
                $crate::conformance::concurrent_writers_wait_instead_of_failing(s).await;
            }
        }
        #[tokio::test]
        async fn mcp_servers_round_trip() {
            if let Some(s) = $make.await {
                $crate::conformance::mcp_servers_round_trip(s).await;
            }
        }
        #[tokio::test]
        async fn api_keys_lifecycle() {
            if let Some(s) = $make.await {
                $crate::conformance::api_keys_lifecycle(s).await;
            }
        }
        #[tokio::test]
        async fn agents_are_versioned_and_labelled() {
            if let Some(s) = $make.await {
                $crate::conformance::agents_are_versioned_and_labelled(s).await;
            }
        }
        #[tokio::test]
        async fn history_respects_compaction() {
            if let Some(s) = $make.await {
                $crate::conformance::history_respects_compaction(s).await;
            }
        }
        #[tokio::test]
        async fn runs_steps_events_and_usage() {
            if let Some(s) = $make.await {
                $crate::conformance::runs_steps_events_and_usage(s).await;
            }
        }
        #[tokio::test]
        async fn blobs_round_trip() {
            if let Some(s) = $make.await {
                $crate::conformance::blobs_round_trip(s).await;
            }
        }
        #[tokio::test]
        async fn audit_entries_filter_by_actor_target_and_time() {
            if let Some(s) = $make.await {
                $crate::conformance::audit_entries_filter_by_actor_target_and_time(s).await;
            }
        }
        #[tokio::test]
        async fn pruning_and_forgetting() {
            if let Some(s) = $make.await {
                $crate::conformance::pruning_and_forgetting(s).await;
            }
        }
        #[tokio::test]
        async fn blocks_are_versioned_and_guarded() {
            if let Some(s) = $make.await {
                $crate::conformance::blocks_are_versioned_and_guarded(s).await;
            }
        }
        #[tokio::test]
        async fn documents_replace_by_source_and_search_both_ways() {
            if let Some(s) = $make.await {
                $crate::conformance::documents_replace_by_source_and_search_both_ways(s).await;
            }
        }
        #[tokio::test]
        async fn skills_version_on_change() {
            if let Some(s) = $make.await {
                $crate::conformance::skills_version_on_change(s).await;
            }
        }
        #[tokio::test]
        async fn personas_are_versioned() {
            if let Some(s) = $make.await {
                $crate::conformance::personas_are_versioned(s).await;
            }
        }
        #[tokio::test]
        async fn feedback_is_one_per_user_and_counted() {
            if let Some(s) = $make.await {
                $crate::conformance::feedback_is_one_per_user_and_counted(s).await;
            }
        }
        #[tokio::test]
        async fn eval_sets_runs_and_results() {
            if let Some(s) = $make.await {
                $crate::conformance::eval_sets_runs_and_results(s).await;
            }
        }
        #[tokio::test]
        async fn triggers_fire_once_per_due_time() {
            if let Some(s) = $make.await {
                $crate::conformance::triggers_fire_once_per_due_time(s).await;
            }
        }
        #[tokio::test]
        async fn workflows_are_versioned() {
            if let Some(s) = $make.await {
                $crate::conformance::workflows_are_versioned(s).await;
            }
        }
    };
}
