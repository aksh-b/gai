//! Video summaries from what is shown (sampled frames, described by a
//! vision model) and what is said (the transcript).

use base64::Engine;
use gai_core::{ContentBlock, MediaSource};
use gai_llm::{ModelRegistry, Transcript, clock};
use gai_store::{NewBlob, SharedStore};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::analysis::parse_clock;
use crate::transcribe::MediaInput;
use crate::{
    MediaModels, Meter, Result, ask, ask_json, ffmpeg, invalid, parse_json, pieces, strings,
    text_field,
};

/// A part of a recording.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Chapter {
    /// Seconds from the start.
    pub start: f64,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
}

/// What one sampled frame shows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FrameNote {
    pub at: f64,
    pub description: String,
    /// The frame, stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct VideoSummary {
    pub title: String,
    /// Markdown.
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chapters: Vec<Chapter>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub key_points: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frames: Vec<FrameNote>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<f64>,
}

impl VideoSummary {
    pub fn markdown(&self) -> String {
        let mut md = format!("# {}\n\n{}\n", self.title, self.summary.trim());
        if !self.key_points.is_empty() {
            md.push_str("\n## Key points\n\n");
            for k in &self.key_points {
                md.push_str(&format!("- {k}\n"));
            }
        }
        if !self.chapters.is_empty() {
            md.push_str("\n## Chapters\n\n");
            for c in &self.chapters {
                md.push_str(&format!("- **{}** {}", clock(c.start), c.title));
                if !c.summary.is_empty() {
                    md.push_str(&format!(": {}", c.summary));
                }
                md.push('\n');
            }
        }
        md
    }
}

#[derive(Clone, Debug, Default)]
pub struct VideoOptions {
    /// Seconds between frames; spread evenly over the video when not set.
    pub every_secs: Option<f64>,
    /// At most this many frames (default 24, at most 60).
    pub max_frames: Option<usize>,
    /// Keep the frames as blobs, for showing next to the summary.
    pub keep_frames: bool,
    /// What to pay attention to.
    pub focus: Option<String>,
}

/// When to take frames: every `every` seconds, or evenly, at most `max`.
pub fn frame_times(duration: f64, every: Option<f64>, max: usize) -> Vec<f64> {
    if duration <= 0.0 {
        return vec![0.0];
    }
    let max = max.clamp(1, 60);
    let n = match every {
        Some(e) if e > 0.0 => ((duration / e).floor() as usize).clamp(1, max),
        _ => ((duration / 10.0).ceil() as usize).clamp(1, max),
    };
    let step = duration / n as f64;
    (0..n).map(|i| (i as f64 + 0.5) * step).collect()
}

const FRAMES_SYSTEM: &str = "You describe frames from a video for someone who cannot see them. For each frame say in one or two sentences what is shown: people and what they are doing, the setting, and any readable text (slide titles, code, screens, captions). Don't guess at what isn't visible. Answer only with JSON.";
const FRAMES_PER_CALL: usize = 6;

/// Frames described by `model`, in batches.
async fn describe_frames(
    models: &ModelRegistry,
    model: &str,
    frames: &[(f64, Vec<u8>)],
    meter: &Meter,
) -> Result<Vec<FrameNote>> {
    let mut out = Vec::with_capacity(frames.len());
    for batch in frames.chunks(FRAMES_PER_CALL) {
        let mut content = vec![ContentBlock::text(format!(
            "{} frames follow, each after its time. Answer as {{\"frames\": [{{\"at\": \"m:ss\", \"description\": \"...\"}}]}}, one entry per frame in order.",
            batch.len()
        ))];
        for (at, jpeg) in batch {
            content.push(ContentBlock::text(format!("Frame at {}:", clock(*at))));
            content.push(ContentBlock::Image {
                source: MediaSource::Base64 {
                    media_type: "image/jpeg".into(),
                    data: base64::engine::general_purpose::STANDARD.encode(jpeg),
                },
            });
        }
        let text = ask(models, model, FRAMES_SYSTEM, content, true, meter).await?;
        let v = parse_json(&text)?;
        let notes: Vec<String> = v["frames"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|f| text_field(f, "description").unwrap_or_default())
            .collect();
        for (i, (at, _)) in batch.iter().enumerate() {
            out.push(FrameNote {
                at: *at,
                description: notes.get(i).cloned().unwrap_or_default(),
                blob_id: None,
            });
        }
    }
    Ok(out)
}

const SUMMARY_SYSTEM: &str = "You summarize videos from what is shown and what is said. Keep names, numbers and claims exact; never add facts that are in neither. Write in Markdown where asked. Answer only with JSON.";
/// Characters of transcript read at once; longer ones are condensed first.
const TRANSCRIPT_CHARS: usize = 50_000;

/// A summary of a video: its frames described with `models.vision`, then
/// those and the transcript summarized with `models.chat`.
pub async fn summarize_video(
    models: &ModelRegistry,
    mm: &MediaModels,
    input: &MediaInput,
    transcript: Option<&Transcript>,
    opts: &VideoOptions,
    store: Option<&SharedStore>,
    meter: &Meter,
) -> Result<VideoSummary> {
    ffmpeg::require()?;
    let dir = tempfile::tempdir().map_err(invalid)?;
    let path = input.write_to(dir.path())?;
    let probe = ffmpeg::probe(&path).await?;
    if !probe.video {
        return Err(invalid("this file has no video"));
    }
    let duration = probe.duration.unwrap_or(0.0);
    let times = frame_times(duration, opts.every_secs, opts.max_frames.unwrap_or(24));
    let frames = ffmpeg::frames(&path, dir.path(), &times, 768).await?;
    if frames.is_empty() {
        return Err(invalid("could not take any frames from the video"));
    }
    let mut notes = describe_frames(models, mm.vision()?, &frames, meter).await?;
    if opts.keep_frames
        && let Some(store) = store
    {
        for (n, (at, jpeg)) in notes.iter_mut().zip(&frames) {
            match store
                .put_blob(NewBlob {
                    media_type: "image/jpeg".into(),
                    source: Some("video_frame".into()),
                    metadata: json!({"at": at, "description": n.description}),
                    data: jpeg.clone(),
                    ..Default::default()
                })
                .await
            {
                Ok(b) => n.blob_id = Some(b.id),
                Err(e) => tracing::warn!(error = %e, "could not keep a frame"),
            }
        }
    }

    let chat = mm.chat()?;
    let mut said = transcript.map(Transcript::timed_text).unwrap_or_default();
    if said.len() > TRANSCRIPT_CHARS {
        let parts = pieces(&said, TRANSCRIPT_CHARS);
        let n = parts.len();
        let mut condensed = Vec::with_capacity(n);
        for (i, p) in parts.iter().enumerate() {
            condensed.push(
                ask(
                    models,
                    chat,
                    "You condense transcripts into timed notes, keeping every name, number and claim.",
                    vec![ContentBlock::text(format!(
                        "Part {} of {n} of a video's transcript. Write its key points as a list, each starting with its time in [m:ss].\n\n{p}",
                        i + 1
                    ))],
                    false,
                    meter,
                )
                .await?,
            );
        }
        said = condensed.join("\n");
    }
    let shown = notes
        .iter()
        .map(|n| format!("[{}] {}", clock(n.at), n.description))
        .collect::<Vec<_>>()
        .join("\n");
    let focus = opts
        .focus
        .as_deref()
        .map(|f| format!(" Pay most attention to: {f}."))
        .unwrap_or_default();
    let v = ask_json(
        models,
        chat,
        SUMMARY_SYSTEM,
        format!(
            "Summarize this video of {}.{focus} Answer as {{\"title\": \"a short title\", \"summary\": \"Markdown, about 200 words\", \"chapters\": [{{\"start\": \"m:ss\", \"title\": \"...\", \"summary\": \"one sentence\"}}], \"key_points\": [\"...\"]}}, with 3 to 10 chapters in order.\n\nWhat is shown:\n{shown}\n\nWhat is said:\n{}",
            clock(duration),
            if said.trim().is_empty() { "(no speech)".to_owned() } else { said }
        ),
        meter,
    )
    .await?;
    let mut chapters: Vec<Chapter> = v["chapters"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            Some(Chapter {
                start: parse_clock(&c["start"])?,
                title: text_field(c, "title")?,
                summary: text_field(c, "summary").unwrap_or_default(),
            })
        })
        .collect();
    chapters.sort_by(|a, b| a.start.total_cmp(&b.start));
    Ok(VideoSummary {
        title: text_field(&v, "title")
            .or_else(|| input.name.clone())
            .unwrap_or_else(|| "Video".into()),
        summary: text_field(&v, "summary").unwrap_or_default(),
        chapters,
        key_points: strings(&v["key_points"]),
        frames: notes,
        duration_secs: probe.duration,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_spread_over_the_video() {
        assert_eq!(frame_times(40.0, None, 24), vec![5.0, 15.0, 25.0, 35.0]);
        assert_eq!(frame_times(3600.0, None, 24).len(), 24);
        assert_eq!(frame_times(100.0, Some(50.0), 24), vec![25.0, 75.0]);
        assert_eq!(frame_times(0.0, None, 24), vec![0.0]);
    }
}
