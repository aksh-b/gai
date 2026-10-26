//! The `meeting_notes`, `analyze_audio` and `summarize_video` tools: a
//! recording in, notes out, kept as a recording too.

use std::sync::Arc;

use gai_llm::ModelRegistry;
use gai_media::{MediaModels, Meter, RecordingOptions, Recordings};
use gai_store::SharedStore;
use gai_tools::{SharedTool, ToolCtx, ToolError, ToolOutput, typed_tool};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::media::{MediaSettings, read_media};

pub(crate) const RECORDING_TOOLS: [&str; 3] = ["meeting_notes", "analyze_audio", "summarize_video"];

#[derive(Deserialize, JsonSchema)]
struct RecordingArgs {
    /// A stored blob id (`blob_…`) of the recording.
    #[serde(default)]
    blob_id: Option<String>,
    /// Or an audio or video file in the workspace.
    #[serde(default)]
    path: Option<String>,
    /// A title for the recording.
    #[serde(default)]
    title: Option<String>,
    /// Notes template: `general` (default), `standup`, `one_on_one`,
    /// `sales_call`, `interview`, `retro` or `lecture`.
    #[serde(default)]
    template: Option<String>,
    /// What else is known: agenda, attendees, names to spell right; for a
    /// video, what to pay attention to.
    #[serde(default)]
    context: Option<String>,
    /// ISO-639-1 language of the speech, if known.
    #[serde(default)]
    language: Option<String>,
}

/// The notes without the full transcript, which can be long; it is in the
/// saved file.
fn without_transcript(md: &str) -> &str {
    md.split("\n## Transcript\n").next().unwrap_or(md)
}

fn tool(
    name: &str,
    kind: &'static str,
    description: String,
    recordings: Arc<Recordings>,
    store: SharedStore,
) -> SharedTool {
    Arc::new(typed_tool(
        name,
        &description,
        move |ctx: ToolCtx, a: RecordingArgs| {
            let (recordings, store) = (recordings.clone(), store.clone());
            async move {
                let input =
                    read_media(&store, &ctx, a.blob_id.as_deref(), a.path.as_deref()).await?;
                let options = RecordingOptions {
                    language: a.language,
                    template: a.template,
                    context: a.context,
                    ..Default::default()
                };
                let meter = Meter::new(
                    Some(store.clone()),
                    (!ctx.run_id.is_empty()).then(|| ctx.run_id.clone()),
                );
                let result = recordings
                    .analyze(kind, &input, &options, &meter)
                    .await
                    .map_err(|e| ToolError::Failed(e.to_string()))?;
                let title = a
                    .title
                    .or_else(|| result.notes.as_ref().map(|n| n.title.clone()))
                    .or_else(|| result.video.as_ref().map(|v| v.title.clone()))
                    .or(input.name.clone())
                    .unwrap_or_else(|| "Recording".into());
                let saved = recordings
                    .save(kind, &title, a.blob_id.clone(), &options, &result)
                    .await
                    .map_err(|e| ToolError::Failed(e.to_string()))?;
                let file = format!("notes/{}.md", saved.id);
                let written = ctx.resolve_path(&file).ok().and_then(|p| {
                    std::fs::create_dir_all(p.parent()?).ok()?;
                    std::fs::write(&p, &result.markdown).ok()
                });
                Ok(ToolOutput {
                    cost: Some(result.cost),
                    ..ToolOutput::text(format!(
                        "{}\n\n(Recording {}{}.)",
                        without_transcript(&result.markdown).trim(),
                        saved.id,
                        if written.is_some() {
                            format!("; notes and the full transcript saved as {file}")
                        } else {
                            String::new()
                        }
                    ))
                })
            }
        },
    ))
}

/// Recording tools named in `wanted`: transcription from `settings`, the
/// writing by `chat_model` (the agent's model).
pub(crate) fn recording_tools(
    models: Arc<ModelRegistry>,
    store: SharedStore,
    settings: &MediaSettings,
    chat_model: Option<String>,
    wanted: &[&str],
) -> Result<Vec<SharedTool>, String> {
    let names = wanted.join(", ");
    let transcription = settings.transcription.clone().ok_or_else(|| {
        format!("{names} need a transcription model: set media.transcription in the agent or transcription under [media] in gai.toml")
    })?;
    let chat = chat_model.ok_or_else(|| {
        format!("{names} need a model: give the agent one or set a default model")
    })?;
    models
        .transcription(&transcription)
        .map_err(|e| e.to_string())?;
    let recordings = Arc::new(Recordings::new(
        store.clone(),
        models,
        MediaModels {
            transcription: Some(transcription),
            chat: Some(chat),
            vision: settings.vision.clone(),
        },
    ));
    let mut out = Vec::new();
    for w in wanted {
        let (kind, description) = match *w {
            "meeting_notes" => (
                "meeting",
                "Take notes on a meeting or call recording: who spoke, a summary, decisions, action items with owners and due dates, open questions, and the template's sections. Give the recording as a blob_id or a workspace path; the notes and transcript are saved.",
            ),
            "analyze_audio" => (
                "audio",
                "Analyse an audio recording: transcript with speakers and times, the language, each speaker's talk time and sentiment, the overall sentiment and the key moments.",
            ),
            "summarize_video" => (
                "video",
                "Summarize a video from what is shown (sampled frames) and said (its audio): a summary, chapters with times and key points.",
            ),
            _ => continue,
        };
        out.push(tool(
            w,
            kind,
            description.to_owned(),
            recordings.clone(),
            store.clone(),
        ));
    }
    Ok(out)
}
