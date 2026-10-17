//! ElevenLabs: text to speech (`/v1/text-to-speech/{voice}`) and speech to
//! text (`/v1/speech-to-text`). It has no chat models.
//!
//! Options (`[providers.<name>.options]`):
//! - `default_voice`: voice id used when a request names none.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::{Dialect, Provider, RemoteModel};
use crate::http::{HttpTarget, send};
use crate::{
    LlmError, MediaUsage, ModelInfo, ProviderConfig, SharedChatModel, SharedSpeechToText,
    SharedTextToSpeech, Speech, SpeechRequest, SpeechToText, TextToSpeech, TranscribeRequest,
    Transcript, audio_media_type, extension_for,
};

/// "Rachel", one of the default voices every account has.
const FALLBACK_VOICE: &str = "21m00Tcm4TlvDq8ikWAM";

pub struct ElevenLabsDialect;

impl Dialect for ElevenLabsDialect {
    fn name(&self) -> &str {
        "elevenlabs"
    }

    fn build(&self, provider: &str, cfg: &ProviderConfig) -> Result<Arc<dyn Provider>, LlmError> {
        let http = HttpTarget::new(
            provider,
            cfg,
            "https://api.elevenlabs.io",
            ("xi-api-key", ""),
        )?;
        Ok(Arc::new(ElevenLabsProvider {
            name: provider.to_owned(),
            http,
            voice: Arc::from(cfg.option_str("default_voice").unwrap_or(FALLBACK_VOICE)),
        }))
    }
}

struct ElevenLabsProvider {
    name: String,
    http: HttpTarget,
    voice: Arc<str>,
}

impl ElevenLabsProvider {
    fn media(&self, info: ModelInfo) -> Arc<ElevenLabsModel> {
        Arc::new(ElevenLabsModel {
            info,
            http: self.http.clone(),
            voice: self.voice.clone(),
        })
    }
}

#[async_trait]
impl Provider for ElevenLabsProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn chat(&self, info: ModelInfo) -> Result<SharedChatModel, LlmError> {
        Err(LlmError::Unsupported(format!(
            "{} is a speech provider; it has no chat models",
            info.id
        )))
    }

    fn transcription(&self, info: ModelInfo) -> Result<SharedSpeechToText, LlmError> {
        Ok(self.media(info))
    }

    fn speech(&self, info: ModelInfo) -> Result<SharedTextToSpeech, LlmError> {
        Ok(self.media(info))
    }

    async fn list_models(&self) -> Result<Vec<RemoteModel>, LlmError> {
        #[derive(Deserialize)]
        struct Item {
            model_id: String,
            #[serde(default)]
            name: Option<String>,
        }
        let items: Vec<Item> = send(self.http.get("/v1/models")).await?.json().await?;
        Ok(items
            .into_iter()
            .map(|m| RemoteModel {
                id: m.model_id,
                display_name: m.name,
                ..Default::default()
            })
            .collect())
    }
}

pub struct ElevenLabsModel {
    info: ModelInfo,
    http: HttpTarget,
    voice: Arc<str>,
}

/// ElevenLabs output format names for gai's format names.
fn output_format(format: &str) -> &'static str {
    match format {
        "wav" => "wav_44100",
        "pcm" => "pcm_44100",
        "opus" => "opus_48000_64",
        _ => "mp3_44100_128",
    }
}

#[async_trait]
impl TextToSpeech for ElevenLabsModel {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    async fn speak(&self, req: SpeechRequest) -> Result<Speech, LlmError> {
        let format = req.format.as_deref().unwrap_or("mp3");
        let voice = req.voice.as_deref().unwrap_or(&self.voice);
        let mut body = json!({"text": req.text, "model_id": self.info.model});
        if let Some(s) = req.speed {
            body["voice_settings"] = json!({"speed": s});
        }
        let path = format!(
            "/v1/text-to-speech/{voice}?output_format={}",
            output_format(format)
        );
        let resp = send(self.http.post_json(&path, body, &Map::new())).await?;
        let audio = resp.bytes().await?.to_vec();
        Ok(Speech {
            audio,
            media_type: audio_media_type(format).to_owned(),
            usage: MediaUsage {
                chars: req.text.chars().count() as u64,
                ..Default::default()
            },
        })
    }
}

#[async_trait]
impl SpeechToText for ElevenLabsModel {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    async fn transcribe(&self, req: TranscribeRequest) -> Result<Transcript, LlmError> {
        use reqwest::multipart::{Form, Part};
        #[derive(Deserialize)]
        struct Resp {
            text: String,
            #[serde(default)]
            language_code: Option<String>,
            #[serde(default)]
            words: Vec<Value>,
        }
        let filename = req
            .filename
            .clone()
            .unwrap_or_else(|| format!("audio.{}", extension_for(&req.media_type)));
        let part = Part::bytes(req.audio)
            .file_name(filename)
            .mime_str(&req.media_type)
            .map_err(|e| LlmError::InvalidRequest(format!("media type: {e}")))?;
        let mut form = Form::new()
            .part("file", part)
            .text("model_id", self.info.model.clone());
        if let Some(l) = req.language {
            form = form.text("language_code", l);
        }
        let r: Resp = send(self.http.post_multipart("/v1/speech-to-text", form))
            .await?
            .json()
            .await?;
        // The last word's end time is the length of the speech.
        let seconds = r.words.iter().rev().find_map(|w| w["end"].as_f64());
        Ok(Transcript {
            text: r.text.trim().to_owned(),
            language: r.language_code,
            duration_secs: seconds,
            usage: MediaUsage {
                seconds: seconds.unwrap_or(0.0),
                ..Default::default()
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::{LlmConfig, ModelRegistry};

    async fn registry(server: &MockServer) -> ModelRegistry {
        let mut cfg = LlmConfig::default();
        let mut p = ProviderConfig::new("elevenlabs", &server.uri());
        p.api_key = Some("el-key".to_owned().into());
        cfg.providers.insert("el".into(), p);
        ModelRegistry::from_config(cfg).unwrap()
    }

    #[tokio::test]
    async fn speaks_and_transcribes() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/v1/text-to-speech/{FALLBACK_VOICE}")))
            .and(query_param("output_format", "mp3_44100_128"))
            .and(header("xi-api-key", "el-key"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"ID3fake".to_vec()))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/speech-to-text"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "text": " hello there ",
                "language_code": "en",
                "words": [{"text": "hello", "end": 0.4}, {"text": "there", "end": 1.25}],
            })))
            .mount(&server)
            .await;
        let r = registry(&server).await;
        let s = r
            .speech("el/eleven_flash_v2_5")
            .unwrap()
            .speak(SpeechRequest {
                text: "hi".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(s.audio, b"ID3fake");
        assert_eq!((s.media_type.as_str(), s.usage.chars), ("audio/mpeg", 2));
        let t = r
            .transcription("el/scribe_v1")
            .unwrap()
            .transcribe(TranscribeRequest {
                audio: vec![1, 2, 3],
                media_type: "audio/wav".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(t.text, "hello there");
        assert_eq!(t.duration_secs, Some(1.25));
        assert!(r.chat("el/x").is_err());
    }
}
