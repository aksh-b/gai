//! Speech-to-text, text-to-speech and image generation.
//!
//! These sit next to [`ChatModel`](crate::ChatModel) and
//! [`EmbeddingModel`](crate::EmbeddingModel): same registry, same
//! `provider/model` addresses, same config-declared providers.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{LlmError, ModelInfo};

/// Prices for models billed by something other than tokens, in USD.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MediaPricing {
    /// Per minute of audio transcribed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_minute: Option<f64>,
    /// Per million characters spoken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_million_chars: Option<f64>,
    /// Per image generated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_image: Option<f64>,
}

/// What a media call used, for pricing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MediaUsage {
    #[serde(default)]
    pub seconds: f64,
    #[serde(default)]
    pub chars: u64,
    #[serde(default)]
    pub images: u32,
}

impl MediaPricing {
    pub fn cost(&self, u: &MediaUsage) -> f64 {
        self.per_minute.unwrap_or(0.0) * u.seconds / 60.0
            + self.per_million_chars.unwrap_or(0.0) * u.chars as f64 / 1e6
            + self.per_image.unwrap_or(0.0) * u.images as f64
    }
}

impl ModelInfo {
    /// Cost of a media call, when the model has media prices.
    pub fn media_cost(&self, u: &MediaUsage) -> Option<f64> {
        self.media_pricing.map(|p| p.cost(u))
    }
}

/// Audio to transcribe.
#[derive(Clone, Debug, Default)]
pub struct TranscribeRequest {
    pub audio: Vec<u8>,
    /// e.g. `audio/mpeg`, `audio/wav`, `audio/webm`.
    pub media_type: String,
    /// A file name hint; some servers pick the decoder by extension.
    pub filename: Option<String>,
    /// ISO-639-1 language, when known.
    pub language: Option<String>,
    /// Words or context that help recognition.
    pub prompt: Option<String>,
    /// Ask for who said what, from models that can tell speakers apart.
    pub diarize: bool,
}

/// A stretch of a transcript: when it was said and, where known, by whom.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Segment {
    /// Seconds from the start.
    pub start: f64,
    pub end: f64,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Transcript {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Length of the audio, when the provider reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<f64>,
    #[serde(default)]
    pub usage: MediaUsage,
    /// Timed stretches, from models that report them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<Segment>,
}

impl Transcript {
    /// The text with a time (and speaker, where known) on each segment,
    /// or the plain text when there are no segments.
    pub fn timed_text(&self) -> String {
        if self.segments.is_empty() {
            return self.text.clone();
        }
        self.segments
            .iter()
            .map(|s| {
                format!(
                    "[{}] {}{}",
                    clock(s.start),
                    s.speaker
                        .as_deref()
                        .map(|p| format!("{p}: "))
                        .unwrap_or_default(),
                    s.text.trim()
                )
            })
            .collect::<Vec<_>>()
            .join(
                "
",
            )
    }

    /// Whether any segment has a speaker.
    pub fn has_speakers(&self) -> bool {
        self.segments.iter().any(|s| s.speaker.is_some())
    }

    /// Join transcripts of consecutive pieces of one recording, each
    /// starting `offset` seconds in.
    pub fn join(parts: Vec<(f64, Transcript)>) -> Transcript {
        let mut out = Transcript::default();
        for (offset, t) in parts {
            if !t.text.trim().is_empty() {
                if !out.text.is_empty() {
                    out.text.push(' ');
                }
                out.text.push_str(t.text.trim());
            }
            out.language = out.language.or(t.language);
            let end = offset + t.duration_secs.unwrap_or(0.0);
            out.duration_secs = Some(out.duration_secs.unwrap_or(0.0).max(end));
            out.usage.seconds += t.usage.seconds;
            out.segments.extend(t.segments.into_iter().map(|mut s| {
                s.start += offset;
                s.end += offset;
                s
            }));
        }
        out
    }
}

/// `h:mm:ss`, or `m:ss` under an hour.
pub fn clock(secs: f64) -> String {
    let t = secs.max(0.0) as u64;
    let (h, m, s) = (t / 3600, t / 60 % 60, t % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Where to open a provider's speech-to-speech session (a WebSocket), and
/// the headers that authenticate it.
#[derive(Clone, Debug)]
pub struct RealtimeTarget {
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// The provider's name for the model.
    pub model: String,
}

/// Text to speak.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SpeechRequest {
    pub text: String,
    /// A provider voice name or id (`alloy`, an ElevenLabs voice id...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// `mp3` (default), `wav`, `opus`, `aac`, `flac` or `pcm`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
    /// How to say it (tone, accent), for models that take instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Speech {
    pub audio: Vec<u8>,
    pub media_type: String,
    pub usage: MediaUsage,
}

/// An image to make.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ImageRequest {
    pub prompt: String,
    /// `1024x1024`, `1536x1024`... Providers reject sizes they lack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    /// How many images (default 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,
    /// `low`, `medium`, `high`, `standard`, `hd`: whatever the model offers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct GeneratedImage {
    pub data: Vec<u8>,
    pub media_type: String,
    /// The prompt the provider actually used, when it rewrote it.
    pub revised_prompt: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Images {
    pub images: Vec<GeneratedImage>,
    pub usage: MediaUsage,
}

#[async_trait]
pub trait SpeechToText: Send + Sync {
    fn info(&self) -> &ModelInfo;
    async fn transcribe(&self, req: TranscribeRequest) -> Result<Transcript, LlmError>;
}

#[async_trait]
pub trait TextToSpeech: Send + Sync {
    fn info(&self) -> &ModelInfo;
    async fn speak(&self, req: SpeechRequest) -> Result<Speech, LlmError>;
}

#[async_trait]
pub trait ImageModel: Send + Sync {
    fn info(&self) -> &ModelInfo;
    async fn generate(&self, req: ImageRequest) -> Result<Images, LlmError>;
}

pub type SharedSpeechToText = Arc<dyn SpeechToText>;
pub type SharedTextToSpeech = Arc<dyn TextToSpeech>;
pub type SharedImageModel = Arc<dyn ImageModel>;

/// Media type for an audio format name (`mp3`, `wav`...).
pub fn audio_media_type(format: &str) -> &'static str {
    match format {
        "wav" => "audio/wav",
        "opus" => "audio/ogg",
        "aac" => "audio/aac",
        "flac" => "audio/flac",
        "pcm" => "audio/L16",
        "webm" => "audio/webm",
        _ => "audio/mpeg",
    }
}

/// A file extension for a media type, for servers that look at names.
pub fn extension_for(media_type: &str) -> &'static str {
    match media_type.split(';').next().unwrap_or_default().trim() {
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/ogg" | "audio/opus" => "ogg",
        "audio/webm" | "video/webm" => "webm",
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" => "m4a",
        "audio/flac" => "flac",
        "audio/aac" => "aac",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        _ => "bin",
    }
}

/// Guess an image's media type from its first bytes.
pub fn sniff_image(data: &[u8]) -> &'static str {
    match data {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [0xFF, 0xD8, 0xFF, ..] => "image/jpeg",
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => "image/webp",
        [b'G', b'I', b'F', ..] => "image/gif",
        _ => "image/png",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_costs_add_up() {
        let p = MediaPricing {
            per_minute: Some(0.006),
            per_million_chars: Some(15.0),
            per_image: Some(0.04),
        };
        let c = p.cost(&MediaUsage {
            seconds: 90.0,
            chars: 2_000,
            images: 2,
        });
        assert!((c - (0.009 + 0.03 + 0.08)).abs() < 1e-9);
        assert_eq!(sniff_image(&[0xFF, 0xD8, 0xFF, 0]), "image/jpeg");
        assert_eq!(extension_for("audio/webm;codecs=opus"), "webm");
    }

    #[test]
    fn transcripts_join_with_offsets() {
        let piece = |text: &str| Transcript {
            text: text.into(),
            duration_secs: Some(600.0),
            usage: MediaUsage {
                seconds: 600.0,
                ..Default::default()
            },
            segments: vec![Segment {
                start: 1.0,
                end: 4.0,
                text: text.into(),
                speaker: Some("A".into()),
            }],
            ..Default::default()
        };
        let t = Transcript::join(vec![(0.0, piece("hello")), (600.0, piece("again"))]);
        assert_eq!(t.text, "hello again");
        assert_eq!(t.duration_secs, Some(1200.0));
        assert_eq!(t.segments[1].start, 601.0);
        assert_eq!(
            t.timed_text(),
            "[0:01] A: hello
[10:01] A: again"
        );
        assert_eq!(clock(3725.0), "1:02:05");
    }
}
