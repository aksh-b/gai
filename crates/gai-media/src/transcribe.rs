//! Transcribing recordings of any length.

use std::path::Path;

use futures::StreamExt;
use gai_llm::{ModelRegistry, TranscribeRequest, Transcript, extension_for};

use crate::{Meter, Result, ffmpeg, invalid};

/// Media to process: the bytes and what they are.
#[derive(Clone, Debug, Default)]
pub struct MediaInput {
    pub data: Vec<u8>,
    /// e.g. `audio/mpeg`, `video/mp4`.
    pub media_type: String,
    pub name: Option<String>,
}

impl MediaInput {
    pub fn is_video(&self) -> bool {
        self.media_type.starts_with("video/")
    }

    /// The file extension, from the name or else the media type.
    pub(crate) fn extension(&self) -> String {
        self.name
            .as_deref()
            .and_then(|n| Path::new(n).extension())
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            .unwrap_or_else(|| match self.media_type.as_str() {
                "video/mp4" => "mp4".into(),
                "video/quicktime" => "mov".into(),
                "video/x-matroska" => "mkv".into(),
                "video/webm" => "webm".into(),
                t => extension_for(t).into(),
            })
    }

    /// Written to `dir`, for ffmpeg.
    pub(crate) fn write_to(&self, dir: &Path) -> Result<std::path::PathBuf> {
        let path = dir.join(format!("input.{}", self.extension()));
        std::fs::write(&path, &self.data).map_err(invalid)?;
        Ok(path)
    }
}

/// The media type for a file name's extension, for files that come without
/// one.
pub fn media_type_for(name: &str) -> &'static str {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase();
    match ext.as_str() {
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "ogg" | "opus" => "audio/ogg",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        "aac" => "audio/aac",
        "weba" => "audio/webm",
        "webm" => "video/webm",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        _ => "application/octet-stream",
    }
}

#[derive(Clone, Debug, Default)]
pub struct TranscribeOptions {
    pub language: Option<String>,
    /// Ask the model for speakers.
    pub diarize: bool,
    /// Names and terms that help recognition.
    pub prompt: Option<String>,
}

/// Providers take up to 25 MB a request.
const DIRECT_BYTES: usize = 24 * 1024 * 1024;
/// Pieces of long recordings: ten minutes each, well inside every
/// provider's limits.
const PIECE_SECS: u32 = 600;

/// The transcript of `input` with `model`: in one call when it is short
/// audio, else (with ffmpeg) its audio in ten-minute pieces, joined.
pub async fn transcribe_media(
    models: &ModelRegistry,
    model: &str,
    input: &MediaInput,
    opts: &TranscribeOptions,
    meter: &Meter,
) -> Result<Transcript> {
    let m = models.transcription(model)?;
    let request =
        |audio: Vec<u8>, media_type: String, filename: Option<String>| TranscribeRequest {
            audio,
            media_type,
            filename,
            language: opts.language.clone(),
            prompt: opts.prompt.clone(),
            diarize: opts.diarize,
        };
    let direct = !input.is_video() && input.data.len() <= DIRECT_BYTES;
    if !ffmpeg::available() {
        if !direct {
            ffmpeg::require()?;
        }
        let t = m
            .transcribe(request(
                input.data.clone(),
                input.media_type.clone(),
                input.name.clone(),
            ))
            .await?;
        meter.media(m.info(), &t.usage).await;
        return Ok(t);
    }
    let dir = tempfile::tempdir().map_err(invalid)?;
    let path = input.write_to(dir.path())?;
    let probe = match ffmpeg::probe(&path).await {
        Ok(p) => Some(p),
        // Short audio ffprobe can't read may still be fine for the
        // provider; send it as it is.
        Err(e) if direct => {
            tracing::debug!(error = %e, "ffprobe could not read the audio; sending it as it is");
            None
        }
        Err(e) => return Err(e),
    };
    if input.is_video() && probe.is_some_and(|p| !p.audio) {
        return Ok(Transcript::default());
    }
    if direct && probe.is_none_or(|p| p.duration.is_none_or(|d| d <= PIECE_SECS as f64)) {
        let t = m
            .transcribe(request(
                input.data.clone(),
                input.media_type.clone(),
                input.name.clone(),
            ))
            .await?;
        meter.media(m.info(), &t.usage).await;
        return Ok(t);
    }
    let parts = ffmpeg::audio_pieces(&path, dir.path(), PIECE_SECS).await?;
    if parts.is_empty() {
        return Err(invalid("ffmpeg found no audio to transcribe"));
    }
    let done: Vec<Result<(usize, f64, Transcript)>> =
        futures::stream::iter(parts.into_iter().enumerate())
            .map(|(i, (at, file))| {
                let (m, req) = (m.clone(), request(Vec::new(), "audio/mpeg".into(), None));
                async move {
                    let audio = tokio::fs::read(&file).await.map_err(invalid)?;
                    let t = m
                        .transcribe(TranscribeRequest {
                            audio,
                            filename: Some(format!("piece{i:03}.mp3")),
                            ..req
                        })
                        .await?;
                    Ok((i, at, t))
                }
            })
            .buffer_unordered(3)
            .collect()
            .await;
    let mut parts = Vec::with_capacity(done.len());
    for r in done {
        let (i, at, t) = r?;
        meter.media(m.info(), &t.usage).await;
        parts.push((i, at, t));
    }
    parts.sort_by_key(|(i, _, _)| *i);
    Ok(Transcript::join(
        parts.into_iter().map(|(_, at, t)| (at, t)).collect(),
    ))
}
