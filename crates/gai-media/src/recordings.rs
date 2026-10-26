//! Recordings kept in the store and processed in the background.

use std::sync::Arc;

use gai_core::Usage;
use gai_llm::{ModelRegistry, Transcript};
use gai_store::{NewRecording, Recording, SharedStore};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::analysis::{AudioInsights, basic_insights, ensure_segments, insights, label_speakers};
use crate::meeting::{MeetingNotes, NotesTemplate, meeting_notes};
use crate::transcribe::{MediaInput, TranscribeOptions, transcribe_media};
use crate::video::{VideoOptions, VideoSummary, summarize_video};
use crate::{MediaError, MediaModels, Meter, Result, invalid};

/// What to make of a recording.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct RecordingOptions {
    /// ISO-639-1 language of the speech, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Tell speakers apart (default: yes for meetings and audio).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speakers: Option<bool>,
    /// Meeting notes template: `general` (default), `standup`,
    /// `one_on_one`, `sales_call`, `interview`, `retro`, `lecture`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// Or a template of your own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_template: Option<NotesTemplate>,
    /// What else is known: the agenda, who attended, names to spell right.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Video: seconds between frames.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub every_secs: Option<f64>,
    /// Video: at most this many frames (default 24).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_frames: Option<usize>,
    /// Models to use instead of the defaults.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<MediaModels>,
}

/// What processing made.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RecordingResult {
    pub transcript: Transcript,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub insights: Option<AudioInsights>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<MeetingNotes>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<VideoSummary>,
    /// Everything above as one Markdown document.
    pub markdown: String,
    pub models: MediaModels,
    pub usage: Usage,
    pub cost: f64,
}

/// What kind of recording, which decides what is made of it.
pub const KINDS: [&str; 3] = ["meeting", "audio", "video"];

pub struct Recordings {
    store: SharedStore,
    models: Arc<ModelRegistry>,
    defaults: MediaModels,
}

impl Recordings {
    pub fn new(store: SharedStore, models: Arc<ModelRegistry>, defaults: MediaModels) -> Self {
        Self {
            store,
            models,
            defaults,
        }
    }

    pub fn defaults(&self) -> &MediaModels {
        &self.defaults
    }

    /// Models from `options` over the defaults.
    fn models_for(&self, options: &RecordingOptions) -> MediaModels {
        let o = options.models.clone().unwrap_or_default();
        MediaModels {
            transcription: o
                .transcription
                .or_else(|| self.defaults.transcription.clone()),
            chat: o.chat.or_else(|| self.defaults.chat.clone()),
            vision: o.vision.or_else(|| self.defaults.vision.clone()),
        }
    }

    /// Process `input` as `kind` without storing a recording: a meeting
    /// gets a transcript with speakers, insights and notes; audio a
    /// transcript and insights; a video a transcript and a summary.
    pub async fn analyze(
        &self,
        kind: &str,
        input: &MediaInput,
        options: &RecordingOptions,
        meter: &Meter,
    ) -> Result<RecordingResult> {
        if !KINDS.contains(&kind) {
            return Err(invalid(format!(
                "kind must be one of {}, not {kind:?}",
                KINDS.join(", ")
            )));
        }
        let mm = self.models_for(options);
        let template = match &options.custom_template {
            Some(t) => {
                let mut t = t.clone();
                if t.name.is_empty() {
                    t.name = "custom".into();
                }
                t
            }
            None => NotesTemplate::named(options.template.as_deref())?,
        };
        let speakers = options.speakers.unwrap_or(kind != "video");
        let mut transcript = transcribe_media(
            &self.models,
            mm.transcription()?,
            input,
            &TranscribeOptions {
                language: options.language.clone(),
                diarize: speakers,
                prompt: options
                    .context
                    .clone()
                    .map(|c| c.chars().take(800).collect()),
            },
            meter,
        )
        .await?;
        ensure_segments(&mut transcript);
        if speakers && !transcript.has_speakers() {
            // Without speakers the rest still works.
            if let Err(e) = label_speakers(&self.models, mm.chat()?, &mut transcript, meter).await {
                tracing::warn!(error = %e, "could not work out the speakers");
            }
        }
        let mut result = RecordingResult {
            models: mm.clone(),
            ..Default::default()
        };
        match kind {
            "video" => {
                let v = summarize_video(
                    &self.models,
                    &mm,
                    input,
                    Some(&transcript),
                    &VideoOptions {
                        every_secs: options.every_secs,
                        max_frames: options.max_frames,
                        keep_frames: true,
                        focus: options.context.clone(),
                    },
                    Some(&self.store),
                    meter,
                )
                .await?;
                result.markdown = v.markdown();
                result.video = Some(v);
            }
            _ => {
                let i = match insights(&self.models, mm.chat()?, &transcript, meter).await {
                    Ok(i) => i,
                    // The model's reading is a bonus: keep the talk times.
                    Err(MediaError::Invalid(e)) => {
                        tracing::warn!(error = %e, "could not analyse the recording");
                        basic_insights(&transcript)
                    }
                    Err(e) => return Err(e),
                };
                if kind == "meeting" {
                    let n = meeting_notes(
                        &self.models,
                        mm.chat()?,
                        &transcript,
                        &template,
                        options.context.as_deref(),
                        meter,
                    )
                    .await?;
                    result.markdown = n.markdown(Some(&i));
                    result.notes = Some(n);
                } else {
                    result.markdown = audio_markdown(&i);
                }
                result.insights = Some(i);
            }
        }
        if !transcript.text.trim().is_empty() {
            result.markdown.push_str("\n## Transcript\n\n");
            result.markdown.push_str(&transcript.timed_text());
            result.markdown.push('\n');
        }
        result.transcript = transcript;
        (result.usage, result.cost) = meter.spent();
        Ok(result)
    }

    /// Store a recording of the blob `blob_id` and process it in the
    /// background; poll [`get`](Self::get) for the result.
    pub async fn start(
        self: &Arc<Self>,
        kind: &str,
        title: &str,
        blob_id: &str,
        options: RecordingOptions,
    ) -> Result<Recording> {
        let rec = self.create(kind, title, blob_id, options).await?;
        let this = self.clone();
        let id = rec.id.clone();
        tokio::spawn(async move { this.process(&id).await });
        Ok(rec)
    }

    /// Store a recording of the blob `blob_id`, to be processed with
    /// [`process`](Self::process).
    pub async fn create(
        &self,
        kind: &str,
        title: &str,
        blob_id: &str,
        options: RecordingOptions,
    ) -> Result<Recording> {
        if !KINDS.contains(&kind) {
            return Err(invalid(format!(
                "kind must be one of {}, not {kind:?}",
                KINDS.join(", ")
            )));
        }
        if self.store.get_blob(blob_id).await?.is_none() {
            return Err(invalid(format!("no blob {blob_id}")));
        }
        if let Some(t) = &options.template {
            NotesTemplate::named(Some(t))?;
        }
        let rec = self
            .store
            .create_recording(NewRecording {
                kind: kind.to_owned(),
                title: title.to_owned(),
                blob_id: Some(blob_id.to_owned()),
                options: serde_json::to_value(&options).map_err(invalid)?,
            })
            .await?;
        Ok(rec)
    }

    /// Store a recording that was already processed (by an agent's tool).
    pub async fn save(
        &self,
        kind: &str,
        title: &str,
        blob_id: Option<String>,
        options: &RecordingOptions,
        result: &RecordingResult,
    ) -> Result<Recording> {
        let rec = self
            .store
            .create_recording(NewRecording {
                kind: kind.to_owned(),
                title: title.to_owned(),
                blob_id,
                options: serde_json::to_value(options).map_err(invalid)?,
            })
            .await?;
        let value = serde_json::to_value(result).map_err(invalid)?;
        self.store
            .finish_recording(&rec.id, "done", Some(&value), None)
            .await?;
        Ok(self.store.get_recording(&rec.id).await?.unwrap_or(rec))
    }

    /// Run a stored recording's processing and record how it went; what it
    /// cost, when it worked.
    pub async fn process(&self, id: &str) -> Option<f64> {
        let outcome = async {
            let rec = self
                .store
                .get_recording(id)
                .await?
                .ok_or_else(|| invalid(format!("no recording {id}")))?;
            let blob_id = rec
                .blob_id
                .clone()
                .ok_or_else(|| invalid("the recording has no media"))?;
            let blob = self
                .store
                .get_blob(&blob_id)
                .await?
                .ok_or_else(|| invalid(format!("blob {blob_id} is gone")))?;
            let data = self.store.blob_data(&blob_id).await?.unwrap_or_default();
            let options: RecordingOptions =
                serde_json::from_value(rec.options.clone()).unwrap_or_default();
            let input = MediaInput {
                data,
                media_type: blob.media_type,
                name: blob.name,
            };
            let meter = Meter::new(Some(self.store.clone()), None);
            self.analyze(&rec.kind, &input, &options, &meter).await
        }
        .await;
        let cost = outcome.as_ref().ok().map(|r| r.cost);
        let saved = match outcome {
            Ok(r) => match serde_json::to_value(&r) {
                Ok(v) => {
                    self.store
                        .finish_recording(id, "done", Some(&v), None)
                        .await
                }
                Err(e) => {
                    self.store
                        .finish_recording(id, "failed", None, Some(&e.to_string()))
                        .await
                }
            },
            Err(e) => {
                tracing::warn!(recording = id, error = %e, "recording failed");
                self.store
                    .finish_recording(id, "failed", None, Some(&e.to_string()))
                    .await
            }
        };
        if let Err(e) = saved {
            tracing::warn!(recording = id, error = %e, "could not save the recording");
        }
        cost
    }

    /// Process again any recording a restart left half done.
    pub async fn recover(self: &Arc<Self>) {
        let Ok(list) = self.store.list_recordings(200).await else {
            return;
        };
        for r in list.into_iter().filter(|r| r.status == "processing") {
            let this = self.clone();
            tokio::spawn(async move {
                this.process(&r.id).await;
            });
        }
    }

    pub async fn get(&self, id: &str) -> Result<Option<Recording>> {
        Ok(self.store.get_recording(id).await?)
    }

    pub async fn list(&self, limit: u32) -> Result<Vec<Recording>> {
        Ok(self.store.list_recordings(limit).await?)
    }

    pub async fn delete(&self, id: &str) -> Result<bool> {
        Ok(self.store.delete_recording(id).await?)
    }
}

fn audio_markdown(i: &AudioInsights) -> String {
    let mut md = String::from("# Recording\n\n");
    if let Some(l) = &i.language {
        md.push_str(&format!("**Language:** {l}\n\n"));
    }
    if let Some(s) = &i.sentiment {
        md.push_str(&format!("**Sentiment:** {} ({:+.2})", s.label, s.score));
        if let Some(r) = &s.reason {
            md.push_str(&format!(". {r}"));
        }
        md.push_str("\n\n");
    }
    if !i.speakers.is_empty() {
        md.push_str("## Speakers\n\n");
        for s in &i.speakers {
            md.push_str(&format!(
                "- **{}**: {:.0}% of the talking, {} turns{}{}\n",
                s.name,
                s.share * 100.0,
                s.turns,
                s.sentiment
                    .as_ref()
                    .map(|x| format!(", {}", x.label))
                    .unwrap_or_default(),
                s.summary
                    .as_ref()
                    .map(|x| format!(". {x}"))
                    .unwrap_or_default()
            ));
        }
    }
    if !i.moments.is_empty() {
        md.push_str("\n## Moments\n\n");
        for m in &i.moments {
            md.push_str(&format!("- **{}** {}\n", gai_llm::clock(m.at), m.what));
        }
    }
    md
}

/// The JSON a stored recording's `result` holds, read back.
pub fn read_result(v: &Value) -> Option<RecordingResult> {
    serde_json::from_value(v.clone()).ok()
}
