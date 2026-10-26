//! Recordings: meetings, calls and videos turned into something to read.
//!
//! - [`transcribe`] transcribes audio or video of any length: ffmpeg takes
//!   the audio out of video and cuts long audio into pieces, which are
//!   transcribed in turn and joined, keeping times and speakers.
//! - [`analysis`] labels speakers when the transcription model could not,
//!   and finds the language, each speaker's talk time and the sentiment.
//! - [`meeting`] writes notes: a summary, decisions, action items with
//!   owners and dates, open questions, and a template's own sections.
//! - [`video`] samples frames, has a vision model describe them, and
//!   summarizes what is shown and said, in chapters.
//! - [`recordings`] keeps recordings in the store and processes them in
//!   the background.

use std::sync::{Arc, Mutex};

use gai_core::{ContentBlock, Message, Role, Usage};
use gai_llm::{ChatRequest, LlmError, MediaUsage, ModelInfo, ModelRegistry, ResponseFormat};
use gai_store::{SharedStore, StoreError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod analysis;
pub mod ffmpeg;
pub mod meeting;
pub mod recordings;
pub mod transcribe;
pub mod video;

pub use analysis::{AudioInsights, Sentiment, SpeakerStats};
pub use meeting::{ActionItem, MeetingNotes, NotesTemplate};
pub use recordings::{RecordingOptions, RecordingResult, Recordings};
pub use video::{Chapter, FrameNote, VideoSummary};

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Llm(#[from] LlmError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub type Result<T> = std::result::Result<T, MediaError>;

pub(crate) fn invalid(e: impl std::fmt::Display) -> MediaError {
    MediaError::Invalid(e.to_string())
}

/// The models a recording is processed with.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct MediaModels {
    /// Speech to text.
    pub transcription: Option<String>,
    /// Writes notes and summaries, and labels speakers and sentiment.
    pub chat: Option<String>,
    /// Describes video frames; the chat model when not set.
    pub vision: Option<String>,
}

impl MediaModels {
    pub(crate) fn transcription(&self) -> Result<&str> {
        self.transcription.as_deref().ok_or_else(|| {
            invalid("no transcription model: set transcription under [media] in gai.toml")
        })
    }

    pub(crate) fn chat(&self) -> Result<&str> {
        self.chat
            .as_deref()
            .ok_or_else(|| invalid("no model to write with: set a default model"))
    }

    pub(crate) fn vision(&self) -> Result<&str> {
        match self.vision.as_deref() {
            Some(v) => Ok(v),
            None => self.chat(),
        }
    }
}

/// What processing spent, recorded in the usage log as it goes.
#[derive(Clone, Default)]
pub struct Meter {
    store: Option<SharedStore>,
    run_id: Option<String>,
    spent: Arc<Mutex<(Usage, f64)>>,
}

impl Meter {
    /// Records each call in `store`'s usage log (on `run_id`, when given).
    pub fn new(store: Option<SharedStore>, run_id: Option<String>) -> Self {
        Self {
            store,
            run_id,
            spent: Arc::default(),
        }
    }

    /// Tokens and dollars so far.
    pub fn spent(&self) -> (Usage, f64) {
        *self.spent.lock().expect("meter")
    }

    async fn add(&self, model: &str, usage: &Usage, cost: Option<f64>) {
        {
            let mut s = self.spent.lock().expect("meter");
            s.0 += *usage;
            s.1 += cost.unwrap_or(0.0);
        }
        if let Some(store) = &self.store
            && let Err(e) = store
                .record_usage(self.run_id.as_deref(), model, usage, cost)
                .await
        {
            tracing::warn!(error = %e, "could not record media usage");
        }
    }

    pub(crate) async fn chat(&self, info: &ModelInfo, usage: &Usage) {
        self.add(&info.id, usage, info.cost(usage)).await;
    }

    pub(crate) async fn media(&self, info: &ModelInfo, usage: &MediaUsage) {
        self.add(&info.id, &Usage::default(), info.media_cost(usage))
            .await;
    }
}

/// One model call with `content` as the user's turn; JSON mode when `json`.
pub(crate) async fn ask(
    models: &ModelRegistry,
    model: &str,
    system: &str,
    content: Vec<ContentBlock>,
    json: bool,
    meter: &Meter,
) -> Result<String> {
    let m = models.chat(model)?;
    let mut req = ChatRequest::new(vec![Message {
        role: Role::User,
        content,
    }])
    .with_system(system);
    req.temperature = Some(0.2);
    req.max_tokens = Some(4096);
    if json {
        req.response_format = Some(ResponseFormat::Json);
    }
    let resp = m.complete(req).await?;
    meter.chat(m.info(), &resp.usage).await;
    Ok(resp.text())
}

/// [`ask`] for a JSON object, read even when the model wraps it in a code
/// fence or a sentence.
pub(crate) async fn ask_json(
    models: &ModelRegistry,
    model: &str,
    system: &str,
    prompt: String,
    meter: &Meter,
) -> Result<Value> {
    let text = ask(
        models,
        model,
        system,
        vec![ContentBlock::text(prompt)],
        true,
        meter,
    )
    .await?;
    parse_json(&text)
}

pub(crate) fn parse_json(text: &str) -> Result<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(text.trim()) {
        return Ok(v);
    }
    let (Some(a), Some(b)) = (text.find('{'), text.rfind('}')) else {
        return Err(invalid(format!(
            "the model did not answer with JSON: {}",
            text.chars().take(200).collect::<String>()
        )));
    };
    serde_json::from_str(&text[a..=b]).map_err(|e| invalid(format!("the model's JSON: {e}")))
}

/// Text cut into pieces of about `size` characters at line breaks, for
/// transcripts too long for one call.
pub(crate) fn pieces(text: &str, size: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in text.lines() {
        if !cur.is_empty() && cur.len() + line.len() > size {
            out.push(std::mem::take(&mut cur));
        }
        cur.push_str(line);
        cur.push('\n');
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// Strings from a JSON array, skipping blanks.
pub(crate) fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| match x {
            Value::String(s) => Some(s.trim().to_owned()),
            Value::Null => None,
            other => Some(other.to_string()),
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// A string field, `None` when missing, null or blank.
pub(crate) fn text_field(v: &Value, key: &str) -> Option<String> {
    v[key]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("null"))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_wrapped_json() {
        assert_eq!(
            parse_json("Sure:\n```json\n{\"a\": 1}\n```").unwrap()["a"],
            1
        );
        assert!(parse_json("no").is_err());
        assert_eq!(pieces("a\nb\nc\n", 3), vec!["a\nb\n", "c\n"]);
        assert_eq!(
            strings(&serde_json::json!(["x", " ", null, 2])),
            vec!["x", "2"]
        );
    }
}
