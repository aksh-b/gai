//! Recordings processed end to end with mock transcription and a scripted
//! writer: a meeting's speakers, insights and notes; speakers worked out
//! when the transcriber gives none; long audio in pieces and a video's
//! frames and chapters, where ffmpeg is installed.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use gai_llm::dialect::mock::mock_wav;
use gai_llm::{
    LlmConfig, LlmError, MockModel, ModelInfo, ModelRegistry, Provider, ProviderConfig,
    SharedChatModel, text_response,
};
use gai_media::transcribe::{MediaInput, TranscribeOptions, transcribe_media};
use gai_media::{MediaModels, Meter, RecordingOptions, RecordingResult, Recordings, ffmpeg};
use gai_store::{NewBlob, SharedStore, SqliteStore};
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

async fn setup(answers: Vec<serde_json::Value>) -> (Arc<Recordings>, SharedStore, Arc<MockModel>) {
    let script = Arc::new(MockModel::scripted(
        answers
            .into_iter()
            .map(|a| text_response(&a.to_string()))
            .collect(),
    ));
    let mut cfg = LlmConfig::default();
    cfg.providers.insert(
        "mock".into(),
        ProviderConfig {
            dialect: "mock".into(),
            ..Default::default()
        },
    );
    let models = ModelRegistry::builder()
        .config(cfg)
        .provider("script", Arc::new(Scripted(script.clone())))
        .build()
        .unwrap();
    let store: SharedStore = Arc::new(SqliteStore::temporary().await.unwrap());
    let r = Arc::new(Recordings::new(
        store.clone(),
        Arc::new(models),
        MediaModels {
            transcription: Some("mock/ears".into()),
            chat: Some("script/writer".into()),
            vision: None,
        },
    ));
    (r, store, script)
}

const MEETING: &str = "Ana: Thanks for joining. Let's decide on the release.\nBen: I think we can ship on Friday.\nAna: Agreed, Friday it is.\nBen: I'll fix the login bug by Thursday.";

#[tokio::test]
async fn a_meeting_gets_speakers_insights_and_notes() {
    let (r, store, script) = setup(vec![
        json!({
            "language": "en",
            "sentiment": {"label": "positive", "score": 0.6, "reason": "Quick agreement."},
            "speakers": {
                "Ana": {"label": "positive", "score": 0.7, "summary": "Ran the meeting."},
                "Ben": {"label": "neutral", "score": 0.2, "summary": "Took the login fix."}
            },
            "moments": [{"at": "0:03", "what": "Release set for Friday"}]
        }),
        json!({
            "title": "Release planning",
            "summary": "The team agreed to ship on Friday.",
            "decisions": ["Ship on Friday"],
            "action_items": [{"task": "Fix the login bug", "owner": "Ben", "due": "Thursday", "at": "0:04"}],
            "open_questions": [],
            "topics": [{"start": "0:00", "title": "Release date"}]
        }),
    ])
    .await;
    let blob = store
        .put_blob(NewBlob {
            media_type: "audio/wav".into(),
            name: Some("standup.wav".into()),
            data: mock_wav(MEETING, 6.0),
            ..Default::default()
        })
        .await
        .unwrap();
    let rec = r
        .start(
            "meeting",
            "Release planning",
            &blob.id,
            RecordingOptions {
                context: Some("Attendees: Ana, Ben".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(rec.status, "processing");
    let done = loop {
        let got = r.get(&rec.id).await.unwrap().unwrap();
        if got.status != "processing" {
            break got;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(done.status, "done", "{:?}", done.error);
    let result: RecordingResult = serde_json::from_value(done.result.unwrap()).unwrap();

    let t = &result.transcript;
    assert_eq!(t.segments.len(), 4);
    assert_eq!(t.segments[1].speaker.as_deref(), Some("Ben"));
    assert_eq!(t.segments[1].text, "I think we can ship on Friday.");

    let i = result.insights.as_ref().unwrap();
    assert_eq!(i.language.as_deref(), Some("en"));
    assert_eq!(i.sentiment.as_ref().unwrap().label, "positive");
    let ana = i.speakers.iter().find(|s| s.name == "Ana").unwrap();
    assert_eq!(ana.turns, 2);
    assert_eq!(ana.summary.as_deref(), Some("Ran the meeting."));
    assert!((i.speakers.iter().map(|s| s.share).sum::<f64>() - 1.0).abs() < 1e-9);
    assert_eq!(i.moments[0].at, 3.0);

    let n = result.notes.as_ref().unwrap();
    assert_eq!(n.action_items[0].owner.as_deref(), Some("Ben"));
    assert_eq!(n.template, "general");
    assert!(
        result.markdown.contains("## Action items"),
        "{}",
        result.markdown
    );
    assert!(
        result.markdown.contains("**Speakers:** Ana"),
        "{}",
        result.markdown
    );
    assert!(result.markdown.contains("## Transcript"));

    let reqs = script.requests().lock().unwrap().clone();
    assert_eq!(
        reqs.len(),
        2,
        "speakers came from the transcriber: no labelling call"
    );
    let notes_prompt = reqs[1].messages[0].text();
    assert!(notes_prompt.contains("Attendees: Ana, Ben"));
    assert!(
        notes_prompt.contains("] Ben: I think we can ship on Friday."),
        "{notes_prompt}"
    );
    assert_eq!(r.list(10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn speakers_are_worked_out_when_the_transcriber_has_none() {
    let (r, _store, script) = setup(vec![
        json!({"speakers": ["Host", "Guest"], "lines": {"1": "Host", "2": "Guest", "3": "Host"}}),
        json!({"language": "en", "sentiment": {"label": "neutral", "score": 0.0}}),
    ])
    .await;
    let input = MediaInput {
        data: mock_wav(
            "Welcome to the show.\nThanks for having me.\nLet's begin.",
            3.0,
        ),
        media_type: "audio/wav".into(),
        name: None,
    };
    let result = r
        .analyze(
            "audio",
            &input,
            &RecordingOptions::default(),
            &Meter::default(),
        )
        .await
        .unwrap();
    let who: Vec<_> = result
        .transcript
        .segments
        .iter()
        .map(|s| s.speaker.clone().unwrap_or_default())
        .collect();
    assert_eq!(who, ["Host", "Guest", "Host"]);
    let i = result.insights.unwrap();
    assert_eq!(i.speakers[0].name, "Host");
    assert_eq!(i.speakers[0].turns, 2);
    assert!(result.notes.is_none());
    assert!(result.markdown.contains("**Host**"), "{}", result.markdown);
    let first = script.requests().lock().unwrap()[0].messages[0].text();
    assert!(first.contains("2. [0:01] Thanks for having me."), "{first}");

    assert!(
        r.analyze(
            "podcast",
            &input,
            &RecordingOptions::default(),
            &Meter::default()
        )
        .await
        .is_err()
    );
    let bad = RecordingOptions {
        template: Some("nope".into()),
        ..Default::default()
    };
    assert!(
        r.analyze("meeting", &input, &bad, &Meter::default())
            .await
            .is_err()
    );
}

fn ffmpeg_make(args: &[&str], out: &std::path::Path) {
    let status = std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .arg(out)
        .status()
        .unwrap();
    assert!(status.success());
}

#[tokio::test]
async fn long_audio_is_transcribed_in_pieces() {
    if !ffmpeg::available() {
        eprintln!("ffmpeg is not installed; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("long.mp3");
    ffmpeg_make(
        &[
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=300:duration=660",
            "-ac",
            "1",
            "-b:a",
            "32k",
        ],
        &file,
    );
    let mut cfg = LlmConfig::default();
    cfg.providers.insert(
        "mock".into(),
        ProviderConfig {
            dialect: "mock".into(),
            ..Default::default()
        },
    );
    let models = ModelRegistry::from_config(cfg).unwrap();
    let input = MediaInput {
        data: std::fs::read(&file).unwrap(),
        media_type: "audio/mpeg".into(),
        name: Some("long.mp3".into()),
    };
    let t = transcribe_media(
        &models,
        "mock/ears",
        &input,
        &TranscribeOptions::default(),
        &Meter::default(),
    )
    .await
    .unwrap();
    assert_eq!(t.text.matches("(mock transcript").count(), 2, "{}", t.text);
    assert_eq!(t.segments.len(), 2);
    assert!(
        (t.segments[1].start - 600.0).abs() < 2.0,
        "the second piece starts ten minutes in: {}",
        t.segments[1].start
    );
}

#[tokio::test]
async fn a_video_gets_frames_and_chapters() {
    if !ffmpeg::available() {
        eprintln!("ffmpeg is not installed; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("demo.mp4");
    ffmpeg_make(
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=20:size=320x240:rate=10",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=20",
            "-shortest",
            "-pix_fmt",
            "yuv420p",
        ],
        &file,
    );
    let (r, store, script) = setup(vec![
        json!({"frames": [
            {"at": "0:05", "description": "A colour test pattern with a counter."},
            {"at": "0:15", "description": "The same pattern, counter at 15."}
        ]}),
        json!({
            "title": "Test pattern",
            "summary": "A test pattern with a tone.",
            "chapters": [{"start": "0:10", "title": "Second half"}, {"start": "0:00", "title": "Start"}],
            "key_points": ["It is a test"]
        }),
    ])
    .await;
    let input = MediaInput {
        data: std::fs::read(&file).unwrap(),
        media_type: "video/mp4".into(),
        name: Some("demo.mp4".into()),
    };
    let result = r
        .analyze(
            "video",
            &input,
            &RecordingOptions::default(),
            &Meter::default(),
        )
        .await
        .unwrap();
    let v = result.video.unwrap();
    assert_eq!(v.title, "Test pattern");
    assert_eq!(v.frames.len(), 2);
    assert_eq!(v.frames[0].at, 5.0);
    assert!(v.frames[1].description.contains("15"));
    let frame = store
        .get_blob(v.frames[0].blob_id.as_deref().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(frame.media_type, "image/jpeg");
    assert_eq!(v.chapters[0].title, "Start");
    assert!(
        result.markdown.contains("**0:10** Second half"),
        "{}",
        result.markdown
    );
    assert!(
        result.transcript.text.contains("(mock transcript"),
        "the audio was transcribed"
    );
    let reqs = script.requests();
    let reqs = reqs.lock().unwrap();
    let images = reqs[0].messages[0]
        .content
        .iter()
        .filter(|b| matches!(b, gai_core::ContentBlock::Image { .. }))
        .count();
    assert_eq!(images, 2);
}

#[tokio::test]
async fn audio_ffprobe_cannot_read_still_goes_to_the_model() {
    let mut cfg = LlmConfig::default();
    cfg.providers.insert(
        "mock".into(),
        ProviderConfig {
            dialect: "mock".into(),
            ..Default::default()
        },
    );
    let models = ModelRegistry::from_config(cfg).unwrap();
    let input = MediaInput {
        data: b"Ana: plain words, not a real WAV file".to_vec(),
        media_type: "audio/wav".into(),
        name: Some("odd.wav".into()),
    };
    let t = transcribe_media(
        &models,
        "mock/ears",
        &input,
        &TranscribeOptions::default(),
        &Meter::default(),
    )
    .await
    .unwrap();
    assert_eq!(t.text, "Ana: plain words, not a real WAV file");
}
