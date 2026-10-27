//! Deterministic models for tests and for trying gai without an API key.
//!
//! - `mock/echo` replies with the last user text. A user message of the form
//!   `call:<tool> <json args>` makes it call that tool instead, and after a
//!   tool result it reports what the tool returned. That is enough to drive
//!   the whole agent loop end to end without a real model.
//! - `mock/judge` grades answers for evals without a real model: it scores
//!   by how many of the rubric's (or reference answer's) words the answer
//!   uses, and counts answer sentences backed by the sources.
//! - [`MockModel::scripted`] replays fixed responses and records requests.
//! - Any `mock/<name>` also works as a speech, transcription or image model:
//!   speech is a short silent WAV that carries its text, transcription reads
//!   that text back (or describes the audio), and images are SVG cards
//!   showing the prompt.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gai_core::{ContentBlock, Role, Usage};
use serde_json::Value;

use super::{Dialect, Provider, RemoteModel};
use crate::{
    BlockKind, ChatEvent, ChatModel, ChatRequest, ChatResponse, ChatStream, EmbedResponse,
    EmbeddingModel, GeneratedImage, ImageModel, ImageRequest, Images, LlmError, MediaUsage,
    ModelInfo, ProviderConfig, Segment, SharedChatModel, SharedEmbeddingModel, SharedImageModel,
    SharedSpeechToText, SharedTextToSpeech, Speech, SpeechRequest, SpeechToText, StopReason,
    TextToSpeech, TranscribeRequest, Transcript,
};

pub struct MockDialect;

impl Dialect for MockDialect {
    fn name(&self) -> &str {
        "mock"
    }

    fn build(&self, provider: &str, _cfg: &ProviderConfig) -> Result<Arc<dyn Provider>, LlmError> {
        Ok(Arc::new(MockProvider {
            name: provider.to_owned(),
        }))
    }
}

struct MockProvider {
    name: String,
}

#[async_trait]
impl Provider for MockProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn chat(&self, info: ModelInfo) -> Result<SharedChatModel, LlmError> {
        match info.model.as_str() {
            "echo" => Ok(Arc::new(MockModel {
                info,
                script: Script::Echo,
                requests: Default::default(),
            })),
            "judge" => Ok(Arc::new(MockModel {
                info,
                script: Script::Judge,
                requests: Default::default(),
            })),
            "fields" => Ok(Arc::new(MockModel {
                info,
                script: Script::Fields,
                requests: Default::default(),
            })),
            "obey" => Ok(Arc::new(MockModel {
                info,
                script: Script::Obey,
                requests: Default::default(),
            })),
            "rewriter" => Ok(Arc::new(MockModel {
                info,
                script: Script::Rewrite,
                requests: Default::default(),
            })),
            other => Err(LlmError::NotFound(format!(
                "mock model {other:?}; available: echo, judge, fields, obey, rewriter"
            ))),
        }
    }

    fn embedding(&self, info: ModelInfo) -> Result<SharedEmbeddingModel, LlmError> {
        let dims = info.capabilities.dimensions.unwrap_or(256) as usize;
        Ok(Arc::new(HashEmbedding { info, dims }))
    }

    fn transcription(&self, info: ModelInfo) -> Result<SharedSpeechToText, LlmError> {
        Ok(Arc::new(MockMedia { info }))
    }

    fn speech(&self, info: ModelInfo) -> Result<SharedTextToSpeech, LlmError> {
        Ok(Arc::new(MockMedia { info }))
    }

    fn image(&self, info: ModelInfo) -> Result<SharedImageModel, LlmError> {
        Ok(Arc::new(MockMedia { info }))
    }

    async fn list_models(&self) -> Result<Vec<RemoteModel>, LlmError> {
        Ok(vec![RemoteModel {
            id: "echo".into(),
            ..Default::default()
        }])
    }
}

/// Media models that need no service.
pub struct MockMedia {
    info: ModelInfo,
}

/// The WAV chunk mock speech keeps its text in.
const TEXT_CHUNK: &[u8; 4] = b"gaiT";
const RATE: u32 = 16_000;

/// A mono 16-bit WAV of silence, `secs` long, carrying `text`.
pub fn mock_wav(text: &str, secs: f64) -> Vec<u8> {
    let samples = (secs * RATE as f64) as u32;
    let data_len = samples * 2;
    let mut t = text.as_bytes().to_vec();
    if t.len() % 2 == 1 {
        t.push(0);
    }
    let mut w = Vec::with_capacity(44 + t.len() + 8 + data_len as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(4 + 24 + 8 + t.len() as u32 + 8 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&RATE.to_le_bytes());
    w.extend_from_slice(&(RATE * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(TEXT_CHUNK);
    w.extend_from_slice(&(t.len() as u32).to_le_bytes());
    w.extend_from_slice(&t);
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    w.resize(w.len() + data_len as usize, 0);
    w
}

/// The text a [`mock_wav`] carries.
fn wav_text(audio: &[u8]) -> Option<String> {
    let at = audio.windows(4).position(|w| w == TEXT_CHUNK)?;
    let len = u32::from_le_bytes(audio.get(at + 4..at + 8)?.try_into().ok()?) as usize;
    let raw = audio.get(at + 8..at + 8 + len)?;
    Some(
        String::from_utf8_lossy(raw)
            .trim_end_matches('\0')
            .to_owned(),
    )
}

/// One segment per line, timed by its share of the characters. With
/// `diarize`, a line `Name: words` is `Name` saying `words`.
fn mock_segments(text: &str, seconds: f64, diarize: bool) -> Vec<Segment> {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let total: usize = lines.iter().map(|l| l.len()).sum::<usize>().max(1);
    let per_char = if seconds > 0.0 {
        seconds / total as f64
    } else {
        0.06
    };
    let mut at = 0.0;
    lines
        .into_iter()
        .map(|l| {
            let start = at;
            at += l.len() as f64 * per_char;
            let (speaker, words) = match l.split_once(':') {
                Some((who, rest))
                    if diarize
                        && !who.trim().is_empty()
                        && who.split_whitespace().count() <= 3
                        && who.trim().starts_with(|c: char| c.is_uppercase()) =>
                {
                    (Some(who.trim().to_owned()), rest.trim())
                }
                _ => (None, l.trim()),
            };
            Segment {
                start,
                end: at,
                text: words.to_owned(),
                speaker,
            }
        })
        .collect()
}

#[async_trait]
impl SpeechToText for MockMedia {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    async fn transcribe(&self, req: TranscribeRequest) -> Result<Transcript, LlmError> {
        let text = wav_text(&req.audio)
            .or_else(|| {
                std::str::from_utf8(&req.audio)
                    .ok()
                    .filter(|t| !t.trim().is_empty() && !t.contains('\0'))
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| {
                format!(
                    "(mock transcript of {} bytes of {})",
                    req.audio.len(),
                    req.media_type
                )
            });
        // 16-bit mono at 16 kHz, roughly; enough for pricing tests.
        let seconds = req.audio.len() as f64 / (RATE as f64 * 2.0);
        let segments = mock_segments(&text, seconds, req.diarize);
        Ok(Transcript {
            text,
            language: req.language.or(Some("en".into())),
            duration_secs: Some(seconds),
            usage: MediaUsage {
                seconds,
                ..Default::default()
            },
            segments,
        })
    }
}

#[async_trait]
impl TextToSpeech for MockMedia {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    async fn speak(&self, req: SpeechRequest) -> Result<Speech, LlmError> {
        let secs = (req.text.chars().count() as f64 * 0.06).clamp(0.3, 8.0);
        Ok(Speech {
            audio: mock_wav(&req.text, secs),
            media_type: "audio/wav".into(),
            usage: MediaUsage {
                chars: req.text.chars().count() as u64,
                ..Default::default()
            },
        })
    }
}

#[async_trait]
impl ImageModel for MockMedia {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    async fn generate(&self, req: ImageRequest) -> Result<Images, LlmError> {
        let n = req.n.unwrap_or(1).clamp(1, 8);
        let (w, h) = req
            .size
            .as_deref()
            .and_then(|s| s.split_once('x'))
            .and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?)))
            .unwrap_or((512, 512));
        let images = (0..n)
            .map(|i| GeneratedImage {
                data: svg_card(&req.prompt, i, w, h).into_bytes(),
                media_type: "image/svg+xml".into(),
                revised_prompt: None,
            })
            .collect();
        Ok(Images {
            images,
            usage: MediaUsage {
                images: n,
                ..Default::default()
            },
        })
    }
}

/// A card with the prompt on a colour picked from the prompt.
fn svg_card(prompt: &str, variant: u32, w: u32, h: u32) -> String {
    let hash = prompt
        .bytes()
        .fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619))
        .wrapping_add(variant * 47);
    let hue = hash % 360;
    let escaped = prompt
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in escaped.split_whitespace() {
        if line.len() + word.len() > 28 && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines.truncate(8);
    let text: String = lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            format!(
                r#"<text x="50%" y="{}" text-anchor="middle" font-family="sans-serif" font-size="{}" fill="white">{l}</text>"#,
                (h as i64 / 2 + i as i64 * 30 - lines.len() as i64 * 15 + 15).max(20),
                22
            )
        })
        .collect();
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="hsl({hue},70%,45%)"/><stop offset="1" stop-color="hsl({},70%,30%)"/></linearGradient></defs><rect width="100%" height="100%" fill="url(#g)"/>{text}</svg>"#,
        (hue + 60) % 360
    )
}

enum Script {
    Echo,
    Judge,
    Fields,
    Obey,
    Rewrite,
    Fixed(Mutex<VecDeque<Result<ChatResponse, LlmError>>>),
}

pub struct MockModel {
    info: ModelInfo,
    script: Script,
    requests: Arc<Mutex<Vec<ChatRequest>>>,
}

impl MockModel {
    /// Replays `responses` in order, one per call; errors once they run out.
    pub fn scripted(responses: Vec<ChatResponse>) -> Self {
        Self::scripted_results(responses.into_iter().map(Ok).collect())
    }

    pub fn scripted_results(responses: Vec<Result<ChatResponse, LlmError>>) -> Self {
        Self {
            info: ModelInfo::new("mock", "scripted"),
            script: Script::Fixed(Mutex::new(responses.into())),
            requests: Default::default(),
        }
    }

    pub fn echo() -> Self {
        Self {
            info: ModelInfo::new("mock", "echo"),
            script: Script::Echo,
            requests: Default::default(),
        }
    }

    pub fn with_info(mut self, info: ModelInfo) -> Self {
        self.info = info;
        self
    }

    /// Requests received so far, for assertions.
    pub fn requests(&self) -> Arc<Mutex<Vec<ChatRequest>>> {
        self.requests.clone()
    }

    fn next(&self, req: &ChatRequest) -> Result<ChatResponse, LlmError> {
        match &self.script {
            Script::Echo => Ok(echo(req)),
            Script::Judge => Ok(text_response(&judge(req))),
            Script::Fields => Ok(text_response(&fields(req))),
            Script::Obey => Ok(obey(req)),
            Script::Rewrite => Ok(text_response(&rewrite(req))),
            Script::Fixed(q) => {
                q.lock().expect("mock lock").pop_front().unwrap_or_else(|| {
                    Err(LlmError::InvalidRequest("mock script exhausted".into()))
                })
            }
        }
    }
}

/// Lowercase words of four letters or more.
fn words(s: &str) -> std::collections::HashSet<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 4)
        .map(str::to_lowercase)
        .collect()
}

/// The text after `label` (a line of its own) up to the next blank-line label.
fn section<'a>(prompt: &'a str, label: &str) -> &'a str {
    let Some(i) = prompt.find(label) else {
        return "";
    };
    let rest = &prompt[i + label.len()..];
    let end = [
        "\n\nAnswer:\n",
        "\n\nRubric",
        "\n\nReference answer:\n",
        "\nAnswer:\n",
    ]
    .iter()
    .filter_map(|m| rest.find(m))
    .min()
    .unwrap_or(rest.len());
    &rest[..end]
}

/// `mock/obey`: follows `If the question mentions "X", answer: Y` lines in
/// its instructions, and otherwise echoes. An agent model whose answers
/// depend on its instructions, for offline tests of the prompt optimizer.
fn obey(req: &ChatRequest) -> ChatResponse {
    let question = req
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .map(|m| m.text().to_lowercase())
        .unwrap_or_default();
    for line in req.system.as_deref().unwrap_or_default().lines() {
        let Some(rest) = line.trim().strip_prefix("If the question mentions \"") else {
            continue;
        };
        if let Some((cue, answer)) = rest.split_once("\", answer: ")
            && question.contains(&cue.to_lowercase())
        {
            return text_response(answer.trim());
        }
    }
    echo(req)
}

/// `mock/rewriter`: the prompt optimizer's writer. Adds an `If the question
/// mentions ..., answer: ...` line to the instructions for each failing
/// case with an expected answer, and answers in the optimizer's JSON.
fn rewrite(req: &ChatRequest) -> String {
    let prompt = req
        .messages
        .iter()
        .map(|m| m.text())
        .collect::<Vec<_>>()
        .join("\n");
    let current = prompt
        .split_once("Current instructions:\n<<<\n")
        .and_then(|(_, r)| r.split_once("\n>>>"))
        .map(|(t, _)| t.trim_end())
        .unwrap_or_default();
    let mut lines = vec![current.to_owned()];
    let mut input = "";
    for line in prompt.lines() {
        if let Some(i) = line.trim().strip_prefix("Input: ") {
            input = i;
        } else if let Some(e) = line.trim().strip_prefix("Expected: ") {
            let cue: Vec<&str> = input.split_whitespace().take(3).collect();
            lines.push(format!(
                "If the question mentions \"{}\", answer: {e}",
                cue.join(" ")
            ));
        }
    }
    serde_json::json!({
        "instructions": lines.join("\n").trim(),
        "rationale": "Say the expected answer for the questions that failed.",
    })
    .to_string()
}

/// `mock/judge`: a JSON grade from word overlap.
/// `mock/fields`: fills the JSON Schema in the prompt from the document's
/// `Name: value` lines (a field's name, or its name with spaces, before the
/// colon), and null where none matches. Deterministic, for offline tests.
fn fields(req: &ChatRequest) -> String {
    let prompt = req
        .messages
        .iter()
        .map(|m| m.text())
        .collect::<Vec<_>>()
        .join("\n");
    let schema = section(&prompt, "Schema:\n");
    let schema: Value =
        serde_json::from_str(schema.split("\n\nDocument").next().unwrap_or_default())
            .unwrap_or(Value::Null);
    let doc = prompt
        .split_once("\nDocument")
        .map(|(_, d)| d)
        .unwrap_or_default();
    let mut out = serde_json::Map::new();
    for name in schema["properties"]
        .as_object()
        .into_iter()
        .flat_map(|p| p.keys())
    {
        let wanted = name.replace('_', " ").to_lowercase();
        let value = doc.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            let k = k
                .trim()
                .trim_start_matches(['-', '*', '#', ' '])
                .to_lowercase();
            (k == wanted || k == name.to_lowercase()).then(|| v.trim().to_owned())
        });
        out.insert(
            name.clone(),
            value.map(Value::String).unwrap_or(Value::Null),
        );
    }
    Value::Object(out).to_string()
}

fn judge(req: &ChatRequest) -> String {
    let prompt = req
        .messages
        .iter()
        .map(|m| m.text())
        .collect::<Vec<_>>()
        .join("\n");
    let answer = section(&prompt, "Answer:\n");
    if prompt.starts_with("Sources:") {
        let sources = words(section(&prompt, "Sources:\n"));
        let claims: Vec<&str> = answer
            .split(['.', '!', '?', '\n'])
            .map(str::trim)
            .filter(|s| !words(s).is_empty())
            .collect();
        let unsupported: Vec<&str> = claims
            .iter()
            .copied()
            .filter(|c| {
                let w = words(c);
                w.iter().filter(|x| sources.contains(*x)).count() * 2 < w.len()
            })
            .collect();
        return serde_json::json!({
            "claims": claims.len(),
            "supported": claims.len() - unsupported.len(),
            "unsupported": unsupported,
        })
        .to_string();
    }
    let mut want = words(section(&prompt, "Rubric (what a good answer does):\n"));
    want.extend(words(section(&prompt, "Reference answer:\n")));
    let got = words(answer);
    let hit = want.iter().filter(|w| got.contains(*w)).count();
    let score = if want.is_empty() {
        0
    } else {
        (hit * 10).div_ceil(want.len())
    };
    serde_json::json!({"score": score.min(10), "reason": format!("uses {hit} of {} expected words", want.len())})
        .to_string()
}

/// A plain text response, handy for scripting.
pub fn text_response(text: &str) -> ChatResponse {
    ChatResponse {
        id: gai_core::new_id("mock"),
        model: "mock".into(),
        content: vec![ContentBlock::text(text)],
        stop_reason: StopReason::EndTurn,
        usage: Usage {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        },
    }
}

/// A response that calls one tool, handy for scripting.
pub fn tool_response(name: &str, input: Value) -> ChatResponse {
    ChatResponse {
        id: gai_core::new_id("mock"),
        model: "mock".into(),
        content: vec![ContentBlock::ToolUse {
            id: gai_core::new_id("toolu"),
            name: name.into(),
            input,
        }],
        stop_reason: StopReason::ToolUse,
        usage: Usage {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        },
    }
}

fn echo(req: &ChatRequest) -> ChatResponse {
    let last = req.messages.iter().rev().find(|m| m.role == Role::User);
    let results: Vec<String> = last
        .map(|m| {
            m.content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolResult {
                        content, is_error, ..
                    } => {
                        let text: String =
                            content.iter().filter_map(ContentBlock::as_text).collect();
                        Some(if *is_error {
                            format!("error: {text}")
                        } else {
                            text
                        })
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    if !results.is_empty() {
        return text_response(&format!("The tool returned: {}", results.join("; ")));
    }
    let text = last.map(|m| m.text()).unwrap_or_default();
    if let Some(rest) = text.trim().strip_prefix("call:") {
        let (name, args) = rest.split_once(' ').unwrap_or((rest, "{}"));
        if req.tools.iter().any(|t| t.name == name) {
            let input = crate::parse_tool_input(args);
            return tool_response(name, input);
        }
        return text_response(&format!("No tool named {name:?} is available."));
    }
    text_response(&text)
}

/// The events a real provider would stream for `resp`, with text split into
/// small pieces so consumers see incremental deltas.
pub fn response_events(resp: &ChatResponse) -> Vec<ChatEvent> {
    let mut out = vec![ChatEvent::Start {
        id: resp.id.clone(),
        model: resp.model.clone(),
    }];
    for (index, block) in resp.content.iter().enumerate() {
        match block {
            ContentBlock::Text { text } => {
                out.push(ChatEvent::BlockStart {
                    index,
                    block: BlockKind::Text,
                });
                for piece in chunks(text, 12) {
                    out.push(ChatEvent::TextDelta { index, text: piece });
                }
            }
            ContentBlock::Thinking {
                text, signature, ..
            } => {
                out.push(ChatEvent::BlockStart {
                    index,
                    block: BlockKind::Thinking,
                });
                out.push(ChatEvent::ReasoningDelta {
                    index,
                    text: text.clone(),
                });
                if let Some(s) = signature {
                    out.push(ChatEvent::SignatureDelta {
                        index,
                        signature: s.clone(),
                    });
                }
            }
            ContentBlock::RedactedThinking { data, .. } => {
                out.push(ChatEvent::BlockStart {
                    index,
                    block: BlockKind::RedactedThinking { data: data.clone() },
                });
            }
            ContentBlock::ToolUse { id, name, input } => {
                out.push(ChatEvent::BlockStart {
                    index,
                    block: BlockKind::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                    },
                });
                let json = match input {
                    Value::String(raw) => raw.clone(),
                    v => v.to_string(),
                };
                out.push(ChatEvent::ToolInputDelta { index, json });
            }
            ContentBlock::Image { .. }
            | ContentBlock::Audio { .. }
            | ContentBlock::ToolResult { .. } => continue,
        }
        out.push(ChatEvent::BlockStop { index });
    }
    out.push(ChatEvent::Usage { usage: resp.usage });
    out.push(ChatEvent::Stop {
        stop_reason: resp.stop_reason.clone(),
    });
    out
}

fn chunks(s: &str, size: usize) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    chars.chunks(size).map(|c| c.iter().collect()).collect()
}

#[async_trait]
impl ChatModel for MockModel {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    async fn stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        let resp = self.next(&req);
        self.requests.lock().expect("mock lock").push(req);
        let events = response_events(&resp?);
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

/// Deterministic bag-of-words embeddings: equal texts map to equal vectors
/// and shared words raise cosine similarity. Good enough for RAG tests.
pub struct HashEmbedding {
    info: ModelInfo,
    dims: usize,
}

impl HashEmbedding {
    pub fn new(dims: usize) -> Self {
        Self {
            info: ModelInfo::new("mock", "hash"),
            dims,
        }
    }
}

#[async_trait]
impl EmbeddingModel for HashEmbedding {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    async fn embed(&self, inputs: &[String]) -> Result<EmbedResponse, LlmError> {
        let vectors = inputs
            .iter()
            .map(|text| {
                let mut v = vec![0f32; self.dims];
                for word in text
                    .to_lowercase()
                    .split(|c: char| !c.is_alphanumeric())
                    .filter(|w| !w.is_empty())
                {
                    let h = word.bytes().fold(1469598103934665603u64, |h, b| {
                        (h ^ b as u64).wrapping_mul(1099511628211)
                    });
                    v[(h % self.dims as u64) as usize] += 1.0;
                }
                let norm = v
                    .iter()
                    .map(|x| x * x)
                    .sum::<f32>()
                    .sqrt()
                    .max(f32::EPSILON);
                v.iter().map(|x| x / norm).collect()
            })
            .collect();
        Ok(EmbedResponse {
            vectors,
            usage: Usage::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gai_core::{Message, ToolSpec};
    use serde_json::json;

    #[tokio::test]
    async fn echo_calls_tools_and_reports_results() {
        let m = MockModel::echo();
        let tools = vec![ToolSpec {
            name: "calc".into(),
            description: String::new(),
            input_schema: json!({}),
            strict: false,
        }];
        let r = m
            .complete(ChatRequest::user("call:calc {\"x\": 2}").with_tools(tools.clone()))
            .await
            .unwrap();
        assert_eq!(r.stop_reason, StopReason::ToolUse);
        let (id, _, input) = r
            .clone()
            .into_message()
            .tool_uses()
            .map(|(a, b, c)| (a.to_owned(), b.to_owned(), c.clone()))
            .next()
            .unwrap();
        assert_eq!(input, json!({"x": 2}));

        let req = ChatRequest::new(vec![
            Message::user("call:calc {}"),
            r.into_message(),
            Message {
                role: Role::User,
                content: vec![ContentBlock::tool_result(id, "4", false)],
            },
        ])
        .with_tools(tools);
        assert_eq!(
            m.complete(req).await.unwrap().text(),
            "The tool returned: 4"
        );
    }

    #[tokio::test]
    async fn scripted_replays_and_records() {
        let m = MockModel::scripted(vec![text_response("one")]);
        assert_eq!(
            m.complete(ChatRequest::user("a")).await.unwrap().text(),
            "one"
        );
        assert!(m.complete(ChatRequest::user("b")).await.is_err());
        assert_eq!(m.requests().lock().unwrap().len(), 2);
    }
}

#[cfg(test)]
mod media_tests {
    use super::*;

    #[tokio::test]
    async fn mock_speech_round_trips_through_mock_transcription() {
        let m = MockMedia {
            info: ModelInfo::new("mock", "voice"),
        };
        let s = m
            .speak(SpeechRequest {
                text: "hello world".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(s.audio.starts_with(b"RIFF") && &s.audio[8..12] == b"WAVE");
        let t = m
            .transcribe(TranscribeRequest {
                audio: s.audio,
                media_type: s.media_type,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(t.text, "hello world");
        assert!(t.duration_secs.unwrap() > 0.3);
        let img = m
            .generate(ImageRequest {
                prompt: "a <red> fox".into(),
                n: Some(2),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(img.images.len(), 2);
        let svg = String::from_utf8(img.images[0].data.clone()).unwrap();
        assert!(svg.starts_with("<svg") && svg.contains("&lt;red&gt;"));
    }
}
