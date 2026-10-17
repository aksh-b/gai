//! Media for agents: the `image_gen`, `speak` and `transcribe` tools, and
//! turning stored blobs into something a model can read.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use base64::Engine;
use gai_core::{ContentBlock, MediaSource, Message, Usage};
use gai_llm::{ImageRequest, ModelRegistry, SpeechRequest, TranscribeRequest, extension_for};
use gai_store::{NewBlob, SharedStore};
use gai_tools::{SharedTool, ToolCtx, ToolError, ToolOutput, typed_tool};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Which models media tools use. Set in config (`[media]`) and, per agent,
/// in the spec's `media`, which wins.
///
/// ```yaml
/// media:
///   image: openai/gpt-image-1
///   speech: elevenlabs/eleven_flash_v2_5
///   voice: 21m00Tcm4TlvDq8ikWAM
///   transcription: openai/gpt-4o-mini-transcribe
/// ```
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct MediaSettings {
    /// Speech-to-text model for `transcribe`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcription: Option<String>,
    /// Text-to-speech model for `speak`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speech: Option<String>,
    /// Voice `speak` uses when the call names none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// Image model for `image_gen`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

impl MediaSettings {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// `self` with gaps filled from `defaults`.
    pub fn over(&self, defaults: &MediaSettings) -> MediaSettings {
        MediaSettings {
            transcription: self
                .transcription
                .clone()
                .or_else(|| defaults.transcription.clone()),
            speech: self.speech.clone().or_else(|| defaults.speech.clone()),
            voice: self.voice.clone().or_else(|| defaults.voice.clone()),
            image: self.image.clone().or_else(|| defaults.image.clone()),
        }
    }
}

/// The tools media settings provide.
pub(crate) const MEDIA_TOOLS: [&str; 3] = ["image_gen", "speak", "transcribe"];

#[derive(Deserialize, JsonSchema)]
struct ImageArgs {
    /// What to draw, in detail: subject, style, composition, lighting.
    prompt: String,
    /// e.g. `1024x1024`, `1536x1024` (landscape), `1024x1536` (portrait).
    #[serde(default)]
    size: Option<String>,
    /// How many images, 1 to 4 (default 1).
    #[serde(default)]
    n: Option<u32>,
    /// `low`, `medium` or `high`, where the model offers it.
    #[serde(default)]
    quality: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct SpeakArgs {
    /// The words to say.
    text: String,
    /// A voice name or id; leave out for the default voice.
    #[serde(default)]
    voice: Option<String>,
    /// `mp3` (default), `wav` or `opus`.
    #[serde(default)]
    format: Option<String>,
    /// How to say it (tone, pace), for models that take instructions.
    #[serde(default)]
    instructions: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct TranscribeArgs {
    /// A stored blob id (`blob_…`) to transcribe.
    #[serde(default)]
    blob_id: Option<String>,
    /// Or an audio file in the workspace.
    #[serde(default)]
    path: Option<String>,
    /// ISO-639-1 language of the speech, if known (`en`, `de`).
    #[serde(default)]
    language: Option<String>,
}

/// What a media call cost, written to the usage log so it shows up with
/// model costs.
async fn record(store: &SharedStore, ctx: &ToolCtx, model: &str, cost: Option<f64>) {
    if let Err(e) = store
        .record_usage(
            (!ctx.run_id.is_empty()).then_some(ctx.run_id.as_str()),
            model,
            &Usage::default(),
            cost,
        )
        .await
    {
        tracing::warn!(error = %e, "could not record media usage");
    }
}

fn failed(e: impl std::fmt::Display) -> ToolError {
    ToolError::Failed(e.to_string())
}

/// Keep a copy in the workspace too, so file tools can use it.
fn save_copy(ctx: &ToolCtx, dir: &str, id: &str, media_type: &str, data: &[u8]) -> Option<String> {
    let rel = format!("{dir}/{id}.{}", extension_for(media_type));
    let path = ctx.resolve_path(&rel).ok()?;
    std::fs::create_dir_all(path.parent()?).ok()?;
    std::fs::write(&path, data).ok()?;
    Some(rel)
}

/// Media tools named in `wanted`, using `settings` for their models.
pub(crate) fn media_tools(
    models: Arc<ModelRegistry>,
    store: SharedStore,
    settings: MediaSettings,
    wanted: &[&str],
) -> Result<Vec<SharedTool>, String> {
    let mut out: Vec<SharedTool> = Vec::new();
    let need = |tool: &str, key: &str, model: &Option<String>| {
        model.clone().ok_or_else(|| {
            format!("{tool} needs a model: set media.{key} in the agent or {key} under [media] in gai.toml")
        })
    };

    if wanted.contains(&"image_gen") {
        let model = need("image_gen", "image", &settings.image)?;
        let m = models.image(&model).map_err(|e| e.to_string())?;
        let store = store.clone();
        out.push(Arc::new(typed_tool(
            "image_gen",
            &format!("Generate images from a text description with {model}. The images are stored and shown to the user; you get their ids and workspace paths."),
            move |ctx, a: ImageArgs| {
                let (m, store) = (m.clone(), store.clone());
                async move {
                    if a.prompt.trim().is_empty() {
                        return Err(ToolError::InvalidInput("prompt is empty".into()));
                    }
                    let resp = m
                        .generate(ImageRequest {
                            prompt: a.prompt.clone(),
                            size: a.size,
                            n: Some(a.n.unwrap_or(1).clamp(1, 4)),
                            quality: a.quality,
                        })
                        .await
                        .map_err(failed)?;
                    let cost = m.info().media_cost(&resp.usage);
                    record(&store, &ctx, &m.info().id, cost).await;
                    let mut lines = vec![format!(
                        "Generated {} image(s) with {}:",
                        resp.images.len(),
                        m.info().id
                    )];
                    let mut content = Vec::new();
                    for img in resp.images {
                        let b = store
                            .put_blob(NewBlob {
                                media_type: img.media_type.clone(),
                                run_id: (!ctx.run_id.is_empty()).then(|| ctx.run_id.clone()),
                                source: Some("image_gen".into()),
                                metadata: json!({
                                    "prompt": a.prompt,
                                    "model": m.info().id,
                                    "revised_prompt": img.revised_prompt,
                                }),
                                data: img.data.clone(),
                                ..Default::default()
                            })
                            .await
                            .map_err(failed)?;
                        let saved = save_copy(&ctx, "images", &b.id, &img.media_type, &img.data);
                        lines.push(format!(
                            "- {}{}{}",
                            b.id,
                            saved.map(|p| format!(" (saved as {p})")).unwrap_or_default(),
                            img.revised_prompt
                                .map(|p| format!(", drawn from the prompt {p:?}"))
                                .unwrap_or_default()
                        ));
                        content.push(ContentBlock::Image {
                            source: MediaSource::Blob {
                                id: b.id,
                                media_type: img.media_type,
                            },
                        });
                    }
                    content.insert(0, ContentBlock::text(lines.join("\n")));
                    Ok(ToolOutput {
                        content,
                        is_error: false,
                        cost,
                    })
                }
            },
        )));
    }

    if wanted.contains(&"speak") {
        let model = need("speak", "speech", &settings.speech)?;
        let m = models.speech(&model).map_err(|e| e.to_string())?;
        let store = store.clone();
        let default_voice = settings.voice.clone();
        out.push(Arc::new(typed_tool(
            "speak",
            &format!("Turn text into speech with {model}. The audio is stored and played to the user; you get its id and workspace path."),
            move |ctx, a: SpeakArgs| {
                let (m, store, default_voice) = (m.clone(), store.clone(), default_voice.clone());
                async move {
                    if a.text.trim().is_empty() {
                        return Err(ToolError::InvalidInput("text is empty".into()));
                    }
                    let s = m
                        .speak(SpeechRequest {
                            text: a.text.clone(),
                            voice: a.voice.or(default_voice),
                            format: a.format,
                            speed: None,
                            instructions: a.instructions,
                        })
                        .await
                        .map_err(failed)?;
                    let cost = m.info().media_cost(&s.usage);
                    record(&store, &ctx, &m.info().id, cost).await;
                    let b = store
                        .put_blob(NewBlob {
                            media_type: s.media_type.clone(),
                            run_id: (!ctx.run_id.is_empty()).then(|| ctx.run_id.clone()),
                            source: Some("speak".into()),
                            metadata: json!({"text": a.text, "model": m.info().id}),
                            data: s.audio.clone(),
                            ..Default::default()
                        })
                        .await
                        .map_err(failed)?;
                    let saved = save_copy(&ctx, "audio", &b.id, &s.media_type, &s.audio);
                    Ok(ToolOutput {
                        content: vec![
                            ContentBlock::text(format!(
                                "Spoke {} characters with {}: {}{}",
                                a.text.chars().count(),
                                m.info().id,
                                b.id,
                                saved.map(|p| format!(" (saved as {p})")).unwrap_or_default()
                            )),
                            ContentBlock::Audio {
                                source: MediaSource::Blob {
                                    id: b.id,
                                    media_type: s.media_type,
                                },
                            },
                        ],
                        is_error: false,
                        cost,
                    })
                }
            },
        )));
    }

    if wanted.contains(&"transcribe") {
        let model = need("transcribe", "transcription", &settings.transcription)?;
        let m = models.transcription(&model).map_err(|e| e.to_string())?;
        let store = store.clone();
        out.push(Arc::new(
            typed_tool(
                "transcribe",
                &format!("Transcribe speech to text with {model}, from a stored blob (`blob_id`) or an audio file in the workspace (`path`)."),
                move |ctx, a: TranscribeArgs| {
                    let (m, store) = (m.clone(), store.clone());
                    async move {
                        let (audio, media_type, name) = match (&a.blob_id, &a.path) {
                            (Some(id), _) => {
                                let b = store
                                    .get_blob(id)
                                    .await
                                    .map_err(failed)?
                                    .ok_or_else(|| ToolError::InvalidInput(format!("no blob {id}")))?;
                                let data = store
                                    .blob_data(id)
                                    .await
                                    .map_err(failed)?
                                    .unwrap_or_default();
                                (data, b.media_type, b.name)
                            }
                            (None, Some(p)) => {
                                let path = ctx.resolve_path(p)?;
                                let data = tokio::fs::read(&path)
                                    .await
                                    .map_err(|e| ToolError::InvalidInput(format!("{p}: {e}")))?;
                                let ext = path
                                    .extension()
                                    .and_then(|e| e.to_str())
                                    .unwrap_or_default()
                                    .to_lowercase();
                                let media_type = match ext.as_str() {
                                    "wav" => "audio/wav",
                                    "ogg" | "opus" => "audio/ogg",
                                    "webm" => "audio/webm",
                                    "m4a" | "mp4" => "audio/mp4",
                                    "flac" => "audio/flac",
                                    _ => "audio/mpeg",
                                };
                                let name = path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned());
                                (data, media_type.to_owned(), name)
                            }
                            (None, None) => {
                                return Err(ToolError::InvalidInput(
                                    "give a blob_id or a path".into(),
                                ));
                            }
                        };
                        let t = m
                            .transcribe(TranscribeRequest {
                                audio,
                                media_type,
                                filename: name,
                                language: a.language,
                                prompt: None,
                            })
                            .await
                            .map_err(failed)?;
                        let cost = m.info().media_cost(&t.usage);
                        record(&store, &ctx, &m.info().id, cost).await;
                        Ok(ToolOutput {
                            cost,
                            ..ToolOutput::text(if t.text.is_empty() {
                                "(no speech found)".to_owned()
                            } else {
                                t.text
                            })
                        })
                    }
                },
            )
            .idempotent(),
        ));
    }
    Ok(out)
}

/// Swaps blob references in a request for the bytes a provider needs, and
/// leaves out media the model cannot take. Blobs are read once per run.
#[derive(Default)]
pub(crate) struct MediaResolver {
    cache: Mutex<HashMap<String, Option<String>>>,
}

impl MediaResolver {
    pub(crate) async fn prepare(
        &self,
        store: &SharedStore,
        messages: &mut [Message],
        vision: bool,
        audio: bool,
    ) {
        for m in messages.iter_mut() {
            for b in m.content.iter_mut() {
                self.block(store, b, vision, audio).await;
            }
        }
    }

    fn block<'a>(
        &'a self,
        store: &'a SharedStore,
        b: &'a mut ContentBlock,
        vision: bool,
        audio: bool,
    ) -> futures::future::BoxFuture<'a, ()> {
        Box::pin(async move {
            match b {
                ContentBlock::ToolResult { content, .. } => {
                    for c in content.iter_mut() {
                        self.block(store, c, vision, audio).await;
                    }
                }
                ContentBlock::Image { .. } if !vision => {
                    *b = ContentBlock::text("[an image this model cannot see]");
                }
                // Providers take raster images only.
                ContentBlock::Image { source } if source.media_type() == Some("image/svg+xml") => {
                    *b = ContentBlock::text("[a vector (SVG) image, which models cannot see]");
                }
                ContentBlock::Audio { .. } if !audio => {
                    *b = ContentBlock::text("[audio this model cannot hear]");
                }
                ContentBlock::Image { source } | ContentBlock::Audio { source } => {
                    if let MediaSource::Blob { id, media_type } = source {
                        match self.data(store, id).await {
                            Some(data) => {
                                *source = MediaSource::Base64 {
                                    media_type: media_type.clone(),
                                    data,
                                }
                            }
                            None => *b = ContentBlock::text(format!("[{id} is no longer stored]")),
                        }
                    }
                }
                _ => {}
            }
        })
    }

    async fn data(&self, store: &SharedStore, id: &str) -> Option<String> {
        if let Some(hit) = self.cache.lock().expect("media cache").get(id) {
            return hit.clone();
        }
        let data = store
            .blob_data(id)
            .await
            .ok()
            .flatten()
            .map(|d| base64::engine::general_purpose::STANDARD.encode(d));
        self.cache
            .lock()
            .expect("media cache")
            .insert(id.to_owned(), data.clone());
        data
    }
}
