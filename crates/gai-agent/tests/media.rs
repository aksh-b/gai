//! Media tools, blobs in requests, tool costs, and routed models in runs.

use std::sync::Arc;

use async_trait::async_trait;
use gai_agent::{AgentSpec, MediaSettings, RunEvent, RunRequest, Runtime, RuntimeConfig};
use gai_core::{ContentBlock, MediaSource};
use gai_llm::{
    Capabilities, ChatResponse, LlmConfig, LlmError, MediaPricing, MockModel, ModelConfig,
    ModelInfo, ModelKind, ModelRegistry, Provider, ProviderConfig, SharedChatModel, text_response,
    tool_response,
};
use gai_store::{RunStatus, SqliteStore, StepKind};
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

/// A runtime whose `script/x` plays `script` (with vision), and whose
/// `mock/*` media models are priced.
async fn runtime(
    script: Vec<ChatResponse>,
    vision: bool,
) -> (Runtime, Arc<MockModel>, tempfile::TempDir) {
    let mut info = ModelInfo::new("script", "x");
    info.capabilities = Capabilities {
        vision,
        ..Default::default()
    };
    let mock = Arc::new(MockModel::scripted(script).with_info(info));
    let mut cfg = LlmConfig::default();
    let mut m = ProviderConfig {
        dialect: "mock".into(),
        ..Default::default()
    };
    let price = MediaPricing {
        per_image: Some(0.04),
        per_million_chars: Some(15.0),
        per_minute: Some(0.006),
    };
    m.models = vec![
        ModelConfig::media("painter", ModelKind::Image, price),
        ModelConfig::media("voice", ModelKind::Speech, price),
        ModelConfig::media("ears", ModelKind::Transcription, price),
    ];
    cfg.providers.insert("mock".into(), m);
    let models = ModelRegistry::builder()
        .config(cfg)
        .provider("script", Arc::new(Scripted(mock.clone())))
        .build()
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let config = RuntimeConfig {
        default_model: Some("mock/echo".into()),
        data_dir: dir.path().to_path_buf(),
        media: MediaSettings {
            image: Some("mock/painter".into()),
            speech: Some("mock/voice".into()),
            transcription: Some("mock/ears".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    let store = Arc::new(SqliteStore::temporary().await.unwrap());
    (
        Runtime::new(
            Arc::new(models),
            ToolRegistry::with_builtins(),
            store,
            config,
        ),
        mock,
        dir,
    )
}

fn agent(tools: &[&str]) -> AgentSpec {
    AgentSpec::new("artist")
        .model("script/x")
        .tools(tools.iter().copied())
}

#[tokio::test]
async fn speak_then_transcribe_round_trips_and_costs_count() {
    let (rt, _mock, dir) = runtime(
        vec![
            tool_response("speak", json!({"text": "good morning"})),
            text_response("said it"),
        ],
        false,
    )
    .await;
    let out = rt
        .run(RunRequest::new(
            agent(&["speak", "transcribe"]),
            "say good morning",
        ))
        .await
        .unwrap();
    assert_eq!(out.status, RunStatus::Completed, "{:?}", out.error);
    // 12 characters at $15 per million.
    assert!(
        (out.cost - 12.0 * 15.0 / 1e6).abs() < 1e-12,
        "cost {}",
        out.cost
    );

    let store = rt.store();
    let blobs = store.list_blobs(Some(&out.run_id), 10).await.unwrap();
    assert_eq!(blobs.len(), 1);
    assert_eq!(
        (blobs[0].media_type.as_str(), blobs[0].source.as_deref()),
        ("audio/wav", Some("speak"))
    );
    // A copy in the workspace, and the model was told where.
    let run = store.get_run(&out.run_id).await.unwrap().unwrap();
    let ws = std::path::PathBuf::from(run.metadata["workspace"].as_str().unwrap());
    assert!(ws.join(format!("audio/{}.wav", blobs[0].id)).is_file());
    let steps = store.list_steps(&out.run_id).await.unwrap();
    let tool = steps.iter().find(|s| s.kind == StepKind::Tool).unwrap();
    assert!(
        tool.output.as_ref().unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains(&blobs[0].id)
    );
    // Usage log has the speech model's cost.
    let totals = store.usage_totals(None).await.unwrap();
    assert!(
        totals
            .iter()
            .any(|t| t.model == "mock/voice" && t.cost > 0.0)
    );

    drop(dir);
}

#[tokio::test]
async fn transcribe_reads_a_workspace_file() {
    let (rt, mock, dir) = runtime(
        vec![
            tool_response("transcribe", json!({"path": "hello.wav"})),
            text_response("heard it"),
        ],
        false,
    )
    .await;
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(
        ws.join("hello.wav"),
        gai_llm::dialect::mock::mock_wav("hello there", 1.5),
    )
    .unwrap();
    let mut spec = agent(&["transcribe"]);
    spec.workspace = Some(ws);
    let out = rt
        .run(RunRequest::new(spec, "what does it say?"))
        .await
        .unwrap();
    assert_eq!(out.status, RunStatus::Completed, "{:?}", out.error);
    let sent = mock.requests().lock().unwrap()[1].clone();
    let ContentBlock::ToolResult { content, .. } = &sent.messages.last().unwrap().content[0] else {
        panic!("no tool result")
    };
    assert_eq!(content[0].as_text(), Some("hello there"));
    // 1.5 s at $0.006 a minute.
    assert!(
        (out.cost - 1.5 / 60.0 * 0.006).abs() < 1e-6,
        "cost {}",
        out.cost
    );
}

#[tokio::test]
async fn generated_images_reach_vision_models_as_bytes_and_others_as_text() {
    for vision in [true, false] {
        let (rt, mock, _dir) = runtime(
            vec![
                tool_response("image_gen", json!({"prompt": "a lighthouse", "n": 2})),
                text_response("drew it"),
            ],
            vision,
        )
        .await;
        let out = rt
            .run(RunRequest::new(agent(&["image_gen"]), "draw a lighthouse"))
            .await
            .unwrap();
        assert_eq!(out.status, RunStatus::Completed, "{:?}", out.error);
        assert!((out.cost - 0.08).abs() < 1e-12, "cost {}", out.cost);
        let blobs = rt.store().list_blobs(Some(&out.run_id), 10).await.unwrap();
        assert_eq!(blobs.len(), 2);
        assert_eq!(blobs[0].metadata["prompt"], "a lighthouse");
        // Mock images are SVG, which no model can see: they reach the model
        // as notes...
        let sent = mock.requests().lock().unwrap()[1].clone();
        let ContentBlock::ToolResult { content, .. } = &sent.messages.last().unwrap().content[0]
        else {
            panic!("no tool result")
        };
        assert!(
            content[0]
                .as_text()
                .unwrap()
                .contains("Generated 2 image(s)")
        );
        assert!(
            content[1..]
                .iter()
                .all(|b| b.as_text().is_some_and(|t| t.contains("cannot see")))
        );
        // ...while the stored result keeps them, for people to see.
        let msgs = rt.store().list_messages(&out.session_id).await.unwrap();
        let ContentBlock::ToolResult { content, .. } = &msgs[2].message.content[0] else {
            panic!("no stored tool result")
        };
        let images = content
            .iter()
            .filter(|b| matches!(b, ContentBlock::Image { .. }))
            .count();
        assert_eq!(images, 2);
    }
}

#[tokio::test]
async fn blob_images_in_input_are_inlined_or_described() {
    for vision in [true, false] {
        let (rt, mock, _dir) = runtime(vec![text_response("a red dot")], vision).await;
        let b = rt
            .store()
            .put_blob(gai_store::NewBlob {
                media_type: "image/png".into(),
                data: vec![0x89, b'P', b'N', b'G'],
                ..Default::default()
            })
            .await
            .unwrap();
        let input = vec![
            ContentBlock::text("what is this?"),
            ContentBlock::Image {
                source: MediaSource::Blob {
                    id: b.id.clone(),
                    media_type: "image/png".into(),
                },
            },
        ];
        let out = rt
            .run(RunRequest::with_input(agent(&[]), input))
            .await
            .unwrap();
        assert_eq!(out.status, RunStatus::Completed, "{:?}", out.error);
        let sent = mock.requests().lock().unwrap()[0].clone();
        let block = &sent.messages[0].content[1];
        if vision {
            assert!(
                matches!(block, ContentBlock::Image { source: MediaSource::Base64 { data, .. } } if data == "iVBORw=="),
                "{block:?}"
            );
        } else {
            assert!(block.as_text().unwrap().contains("cannot see"));
        }
        // The stored conversation keeps the reference, not the bytes.
        let msgs = rt.store().list_messages(&out.session_id).await.unwrap();
        assert!(matches!(
            &msgs[0].message.content[1],
            ContentBlock::Image {
                source: MediaSource::Blob { .. }
            }
        ));
    }
}

#[tokio::test]
async fn missing_media_model_is_a_clear_error() {
    let (rt, _, _dir) = runtime(vec![], false).await;
    let mut spec = agent(&["image_gen"]);
    spec.media.image = None;
    // The runtime default is set, so this works...
    assert!(rt.start(RunRequest::new(spec.clone(), "x")).await.is_ok());
    // ...but an agent pointing at a chat-only provider does not.
    spec.media.image = Some("anthropic/claude-opus-5-5".into());
    let err = match rt.start(RunRequest::new(spec, "x")).await {
        Err(e) => e.to_string(),
        Ok(_) => panic!("expected an error"),
    };
    assert!(err.contains("image") || err.contains("key"), "{err}");
}

#[tokio::test]
async fn routed_runs_record_the_real_model_and_its_cost() {
    let mut cfg = LlmConfig::default();
    cfg.routers.insert(
        "auto".into(),
        serde_json::from_value(json!({"models": [
            {"model": "mock/echo", "level": "easy"},
        ]}))
        .unwrap(),
    );
    let models = ModelRegistry::from_config(cfg).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let rt = Runtime::new(
        Arc::new(models),
        ToolRegistry::with_builtins(),
        Arc::new(SqliteStore::temporary().await.unwrap()),
        RuntimeConfig {
            data_dir: dir.path().into(),
            ..Default::default()
        },
    );
    let mut h = rt
        .start(RunRequest::new(
            AgentSpec::new("r").model("router/auto"),
            "hello",
        ))
        .await
        .unwrap();
    let mut routed = None;
    while let Some(e) = h.next_event().await {
        if let RunEvent::ModelRouted { model, reason, .. } = e.event {
            routed = Some((model, reason));
        }
    }
    let out = h.wait().await;
    assert_eq!(out.status, RunStatus::Completed, "{:?}", out.error);
    assert_eq!(out.text, "hello");
    let (model, reason) = routed.expect("a routing event");
    assert_eq!(model, "mock/echo");
    assert!(reason.contains("easy"), "{reason}");
    let steps = rt.store().list_steps(&out.run_id).await.unwrap();
    assert_eq!(
        steps[0].output.as_ref().unwrap()["routed"]["model"],
        "mock/echo"
    );
    let totals = rt.store().usage_totals(None).await.unwrap();
    assert!(totals.iter().any(|t| t.model == "mock/echo"));
}

#[tokio::test]
async fn meeting_notes_tool_writes_notes_and_keeps_the_recording() {
    let (rt, mock, dir) = runtime(
        vec![
            tool_response(
                "meeting_notes",
                json!({"path": "standup.wav", "template": "standup"}),
            ),
            // The tool's own calls go to the agent's model: insights, then notes.
            text_response(
                &json!({"language": "en", "sentiment": {"label": "positive", "score": 0.5}})
                    .to_string(),
            ),
            text_response(
                &json!({
                    "title": "Standup",
                    "summary": "Quick standup.",
                    "decisions": [],
                    "action_items": [{"task": "Review the PR", "owner": "Ana"}],
                    "sections": {"updates": "- Ana: reviewing\n- Ben: testing"}
                })
                .to_string(),
            ),
            text_response("Notes are ready."),
        ],
        false,
    )
    .await;
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(
        ws.join("standup.wav"),
        gai_llm::dialect::mock::mock_wav("Ana: I'm reviewing the PR.\nBen: I'm testing.", 3.0),
    )
    .unwrap();
    let mut spec = agent(&["meeting_notes"]);
    spec.workspace = Some(ws.clone());
    let out = rt
        .run(RunRequest::new(spec, "notes on standup.wav please"))
        .await
        .unwrap();
    assert_eq!(out.status, RunStatus::Completed, "{:?}", out.error);
    let sent = mock.requests().lock().unwrap()[3].clone();
    let ContentBlock::ToolResult { content, .. } = &sent.messages.last().unwrap().content[0] else {
        panic!("no tool result")
    };
    let text = content[0].as_text().unwrap();
    assert!(text.contains("- [ ] Review the PR (**Ana**)"), "{text}");
    assert!(text.contains("## Updates"), "{text}");
    assert!(
        !text.contains("## Transcript"),
        "the transcript stays in the file"
    );
    let recs = rt.store().list_recordings(5).await.unwrap();
    assert_eq!((recs.len(), recs[0].status.as_str()), (1, "done"));
    let saved = std::fs::read_to_string(ws.join(format!("notes/{}.md", recs[0].id))).unwrap();
    assert!(saved.contains("Ben: I'm testing."), "{saved}");
    // Transcription is charged to the run.
    assert!(out.cost > 0.0);
}
