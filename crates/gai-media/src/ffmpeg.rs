//! ffmpeg and ffprobe, for audio out of video, long audio in pieces, and
//! video frames.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::Value;

use crate::{Result, invalid};

/// Whether `ffmpeg` and `ffprobe` run here, checked once.
pub fn available() -> bool {
    static FOUND: OnceLock<bool> = OnceLock::new();
    *FOUND.get_or_init(|| {
        ["ffmpeg", "ffprobe"].iter().all(|p| {
            std::process::Command::new(p)
                .arg("-version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        })
    })
}

pub(crate) fn require() -> Result<()> {
    if available() {
        Ok(())
    } else {
        Err(invalid(
            "this needs ffmpeg, which is not installed here (apt install ffmpeg, brew install ffmpeg)",
        ))
    }
}

async fn run(program: &str, args: &[&str], timeout: Duration) -> Result<Vec<u8>> {
    let out = tokio::time::timeout(
        timeout,
        tokio::process::Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| invalid(format!("{program} took over {} s", timeout.as_secs())))?
    .map_err(|e| invalid(format!("could not run {program}: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let last = err
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        return Err(invalid(format!("{program} failed: {}", last.trim())));
    }
    Ok(out.stdout)
}

/// What a media file holds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Probe {
    pub duration: Option<f64>,
    pub video: bool,
    pub audio: bool,
}

pub async fn probe(path: &Path) -> Result<Probe> {
    let path = path.to_string_lossy();
    let out = run(
        "ffprobe",
        &[
            "-v",
            "error",
            "-show_entries",
            "format=duration:stream=codec_type,disposition",
            "-of",
            "json",
            &path,
        ],
        Duration::from_secs(60),
    )
    .await?;
    let v: Value = serde_json::from_slice(&out).map_err(invalid)?;
    let streams = v["streams"].as_array().cloned().unwrap_or_default();
    let has = |kind: &str| {
        streams.iter().any(|s| {
            s["codec_type"] == kind
                // A cover picture in an audio file is not video.
                && s["disposition"]["attached_pic"].as_i64() != Some(1)
        })
    };
    Ok(Probe {
        duration: v["format"]["duration"]
            .as_str()
            .and_then(|d| d.parse().ok()),
        video: has("video"),
        audio: has("audio"),
    })
}

/// The audio as mono 16 kHz MP3 in pieces of at most `piece_secs`, with
/// each piece's start time.
pub async fn audio_pieces(
    input: &Path,
    dir: &Path,
    piece_secs: u32,
) -> Result<Vec<(f64, PathBuf)>> {
    let pattern = dir.join("piece%03d.mp3");
    let (input_s, pattern_s, secs) = (
        input.to_string_lossy(),
        pattern.to_string_lossy(),
        piece_secs.to_string(),
    );
    run(
        "ffmpeg",
        &[
            "-nostdin",
            "-hide_banner",
            "-y",
            "-i",
            &input_s,
            "-vn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "libmp3lame",
            "-b:a",
            "48k",
            "-f",
            "segment",
            "-segment_time",
            &secs,
            "-reset_timestamps",
            "1",
            &pattern_s,
        ],
        Duration::from_secs(1800),
    )
    .await?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(invalid)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("piece") && n.ends_with(".mp3"))
        })
        .collect();
    files.sort();
    let mut out = Vec::with_capacity(files.len());
    let mut at = 0.0;
    for f in files {
        let d = probe(&f)
            .await
            .ok()
            .and_then(|p| p.duration)
            .unwrap_or(piece_secs as f64);
        out.push((at, f));
        at += d;
    }
    Ok(out)
}

/// A JPEG frame at each of `times` (seconds), `width` pixels wide.
pub async fn frames(
    input: &Path,
    dir: &Path,
    times: &[f64],
    width: u32,
) -> Result<Vec<(f64, Vec<u8>)>> {
    let input_s = input.to_string_lossy().into_owned();
    let scale = format!("scale={width}:-2");
    let mut out = Vec::with_capacity(times.len());
    for (i, t) in times.iter().enumerate() {
        let file = dir.join(format!("frame{i:03}.jpg"));
        let (at, file_s) = (format!("{t:.2}"), file.to_string_lossy().into_owned());
        let r = run(
            "ffmpeg",
            &[
                "-nostdin",
                "-hide_banner",
                "-y",
                "-ss",
                &at,
                "-i",
                &input_s,
                "-frames:v",
                "1",
                "-vf",
                &scale,
                "-q:v",
                "4",
                &file_s,
            ],
            Duration::from_secs(120),
        )
        .await;
        match r.and_then(|_| std::fs::read(&file).map_err(invalid)) {
            Ok(data) if !data.is_empty() => out.push((*t, data)),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, at = t, "could not take a frame"),
        }
    }
    Ok(out)
}
