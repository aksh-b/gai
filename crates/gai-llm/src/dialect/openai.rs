//! OpenAI Chat Completions (`POST /chat/completions`), spoken by OpenAI and
//! by most other hosted and local model servers.
//!
//! Options (`[providers.<name>.options]`):
//! - `max_tokens_field`: `max_completion_tokens` (OpenAI) or `max_tokens`.
//! - `think_tags` (default false): treat `<think>…</think>` in the content
//!   as reasoning (DeepSeek-R1, Qwen and similar served by Ollama or vLLM).
//! - `reasoning_effort` (default: when the model has the reasoning
//!   capability): send `reasoning_effort`.
//! - `stream_usage` (default true): ask for usage in the final chunk.
//! - `transcription_path` (default `/audio/transcriptions`): where speech
//!   to text lives; local whisper servers sometimes use another path.
//!
//! Besides chat and embeddings, the dialect serves speech to text
//! (`/audio/transcriptions`), text to speech (`/audio/speech`) and image
//! generation (`/images/generations`).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use gai_core::{ContentBlock, Message, Role, ToolChoice, Usage};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::{Dialect, Provider, RemoteModel};
use crate::http::{HttpTarget, send, sse};
use crate::{
    BlockKind, ChatEvent, ChatModel, ChatRequest, ChatStream, Effort, EmbedResponse,
    EmbeddingModel, GeneratedImage, ImageModel, ImageRequest, Images, LlmError, MediaUsage,
    ModelInfo, ProviderConfig, ResponseFormat, SharedChatModel, SharedEmbeddingModel,
    SharedImageModel, SharedSpeechToText, SharedTextToSpeech, Speech, SpeechRequest, SpeechToText,
    StopReason, TextToSpeech, TranscribeRequest, Transcript, audio_media_type, extension_for,
    sniff_image,
};

pub struct OpenAiDialect;

impl Dialect for OpenAiDialect {
    fn name(&self) -> &str {
        "openai"
    }

    fn build(&self, provider: &str, cfg: &ProviderConfig) -> Result<Arc<dyn Provider>, LlmError> {
        let http = HttpTarget::new(
            provider,
            cfg,
            "https://api.openai.com/v1",
            ("authorization", "Bearer"),
        )?;
        Ok(Arc::new(OpenAiProvider {
            name: provider.to_owned(),
            http,
            opts: Arc::new(Options {
                max_tokens_field: cfg
                    .option_str("max_tokens_field")
                    .unwrap_or("max_tokens")
                    .to_owned(),
                think_tags: cfg.option_bool("think_tags").unwrap_or(false),
                reasoning_effort: cfg.option_bool("reasoning_effort"),
                stream_usage: cfg.option_bool("stream_usage").unwrap_or(true),
                transcription_path: cfg
                    .option_str("transcription_path")
                    .unwrap_or("/audio/transcriptions")
                    .to_owned(),
            }),
        }))
    }
}

#[derive(Debug)]
struct Options {
    max_tokens_field: String,
    think_tags: bool,
    reasoning_effort: Option<bool>,
    stream_usage: bool,
    transcription_path: String,
}

struct OpenAiProvider {
    name: String,
    http: HttpTarget,
    opts: Arc<Options>,
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn chat(&self, info: ModelInfo) -> Result<SharedChatModel, LlmError> {
        Ok(Arc::new(OpenAiModel {
            info,
            http: self.http.clone(),
            opts: self.opts.clone(),
        }))
    }

    fn embedding(&self, info: ModelInfo) -> Result<SharedEmbeddingModel, LlmError> {
        Ok(Arc::new(OpenAiEmbedding {
            info,
            http: self.http.clone(),
        }))
    }

    fn transcription(&self, info: ModelInfo) -> Result<SharedSpeechToText, LlmError> {
        Ok(Arc::new(self.media(info)))
    }

    fn speech(&self, info: ModelInfo) -> Result<SharedTextToSpeech, LlmError> {
        Ok(Arc::new(self.media(info)))
    }

    fn image(&self, info: ModelInfo) -> Result<SharedImageModel, LlmError> {
        Ok(Arc::new(self.media(info)))
    }

    async fn list_models(&self) -> Result<Vec<RemoteModel>, LlmError> {
        #[derive(Deserialize)]
        struct Page {
            data: Vec<Item>,
        }
        #[derive(Deserialize)]
        struct Item {
            id: String,
            #[serde(default)]
            context_length: Option<u32>,
            #[serde(default)]
            name: Option<String>,
        }
        let page: Page = send(self.http.get("/models")).await?.json().await?;
        Ok(page
            .data
            .into_iter()
            .map(|m| RemoteModel {
                id: m.id,
                display_name: m.name,
                context_window: m.context_length,
                max_output_tokens: None,
            })
            .collect())
    }
}

impl OpenAiProvider {
    fn media(&self, info: ModelInfo) -> OpenAiMedia {
        OpenAiMedia {
            info,
            http: self.http.clone(),
            opts: self.opts.clone(),
        }
    }
}

pub struct OpenAiModel {
    info: ModelInfo,
    http: HttpTarget,
    opts: Arc<Options>,
}

impl OpenAiModel {
    fn body(&self, req: &ChatRequest) -> Value {
        let mut messages = Vec::new();
        if let Some(system) = req.system.as_deref().filter(|s| !s.is_empty()) {
            messages.push(json!({"role": "system", "content": system}));
        }
        for m in &req.messages {
            push_message(&mut messages, m);
        }
        let mut body = json!({"model": self.info.model, "messages": messages, "stream": true});
        let o = body.as_object_mut().expect("object");
        if self.opts.stream_usage {
            o.insert("stream_options".into(), json!({"include_usage": true}));
        }
        if !req.tools.is_empty() {
            let tools: Vec<Value> = req
                .tools
                .iter()
                .map(|t| {
                    let mut f = json!({"name": t.name, "description": t.description, "parameters": t.input_schema});
                    if t.strict {
                        f["strict"] = json!(true);
                    }
                    json!({"type": "function", "function": f})
                })
                .collect();
            o.insert("tools".into(), Value::Array(tools));
            let choice = match &req.tool_choice {
                ToolChoice::Auto => None,
                ToolChoice::None => Some(json!("none")),
                ToolChoice::Required => Some(json!("required")),
                ToolChoice::Tool { name } => {
                    Some(json!({"type": "function", "function": {"name": name}}))
                }
            };
            if let Some(c) = choice {
                o.insert("tool_choice".into(), c);
            }
        }
        if let Some(n) = req.max_tokens {
            o.insert(self.opts.max_tokens_field.clone(), json!(n));
        }
        if let Some(t) = req.temperature {
            o.insert("temperature".into(), json!(t));
        }
        if let Some(p) = req.top_p {
            o.insert("top_p".into(), json!(p));
        }
        if !req.stop.is_empty() {
            o.insert("stop".into(), json!(req.stop));
        }
        let send_effort = self
            .opts
            .reasoning_effort
            .unwrap_or(self.info.capabilities.reasoning);
        if let (Some(r), true) = (&req.reasoning, send_effort) {
            let effort = match r.effort {
                Effort::Off => "minimal",
                Effort::Low => "low",
                Effort::Medium => "medium",
                Effort::High | Effort::Xhigh | Effort::Max => "high",
            };
            o.insert("reasoning_effort".into(), json!(effort));
        }
        match &req.response_format {
            Some(ResponseFormat::Json) => _ = o.insert("response_format".into(), json!({"type": "json_object"})),
            Some(ResponseFormat::JsonSchema { name, schema, strict }) => {
                _ = o.insert(
                    "response_format".into(),
                    json!({"type": "json_schema", "json_schema": {"name": name, "schema": schema, "strict": strict}}),
                )
            }
            Some(ResponseFormat::Text) | None => {}
        }
        body
    }
}

/// Chat Completions splits what gai keeps in one user message: each tool
/// result becomes its own `tool` message, ahead of any other user content.
/// Reasoning is never sent back.
fn push_message(out: &mut Vec<Value>, m: &Message) {
    match m.role {
        Role::User => {
            let mut parts = Vec::new();
            for b in &m.content {
                match b {
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        let text: String = content
                            .iter()
                            .filter_map(ContentBlock::as_text)
                            .collect::<Vec<_>>()
                            .join("\n");
                        let text = if *is_error {
                            format!("Error: {text}")
                        } else {
                            text
                        };
                        out.push(
                            json!({"role": "tool", "tool_call_id": tool_use_id, "content": text}),
                        );
                    }
                    ContentBlock::Text { text } if !text.is_empty() => {
                        parts.push(json!({"type": "text", "text": text}))
                    }
                    ContentBlock::Image { source } => parts
                        .push(json!({"type": "image_url", "image_url": {"url": source.to_url()}})),
                    ContentBlock::Audio {
                        source: gai_core::MediaSource::Base64 { media_type, data },
                    } => parts.push(json!({
                        "type": "input_audio",
                        "input_audio": {"data": data, "format": extension_for(media_type)},
                    })),
                    _ => {}
                }
            }
            match parts.as_slice() {
                [] => {}
                [only] if only["type"] == "text" => {
                    out.push(json!({"role": "user", "content": only["text"]}))
                }
                _ => out.push(json!({"role": "user", "content": parts})),
            }
        }
        Role::Assistant => {
            let text: String = m.content.iter().filter_map(ContentBlock::as_text).collect();
            let calls: Vec<Value> = m
                .tool_uses()
                .map(|(id, name, input)| {
                    let args = match input {
                        Value::String(raw) => raw.clone(),
                        v => v.to_string(),
                    };
                    json!({"id": id, "type": "function", "function": {"name": name, "arguments": args}})
                })
                .collect();
            if text.is_empty() && calls.is_empty() {
                return;
            }
            let mut msg = json!({"role": "assistant", "content": if text.is_empty() { Value::Null } else { json!(text) }});
            if !calls.is_empty() {
                msg["tool_calls"] = Value::Array(calls);
            }
            out.push(msg);
        }
    }
}

#[async_trait]
impl ChatModel for OpenAiModel {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    async fn stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        let body = self.body(&req);
        let resp = send(self.http.post_json("/chat/completions", body, &req.extra)).await?;
        let mut parser = StreamParser::new(self.opts.think_tags);
        let events = sse(resp)
            .map(Some)
            .chain(futures::stream::once(async { None }))
            .flat_map(move |item| {
                let out = match item {
                    Some(Ok(e)) => parser.parse(&e.data),
                    Some(Err(e)) => vec![Err(e)],
                    None => parser.finish(),
                };
                futures::stream::iter(out)
            });
        Ok(Box::pin(events))
    }
}

/// Turns Chat Completions chunks into [`ChatEvent`]s, assigning block
/// indices: a new block opens whenever the kind of content changes.
pub(crate) struct StreamParser {
    started: bool,
    finished: bool,
    next_index: usize,
    open: Option<(Open, usize)>,
    tool_blocks: HashMap<u64, usize>,
    stop_reason: Option<StopReason>,
    think: Option<ThinkSplitter>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Open {
    Text,
    Reasoning,
    Tool(u64),
}

impl StreamParser {
    pub(crate) fn new(think_tags: bool) -> Self {
        Self {
            started: false,
            finished: false,
            next_index: 0,
            open: None,
            tool_blocks: HashMap::new(),
            stop_reason: None,
            think: think_tags.then(ThinkSplitter::default),
        }
    }

    pub(crate) fn parse(&mut self, data: &str) -> Vec<Result<ChatEvent, LlmError>> {
        let data = data.trim();
        if data.is_empty() {
            return vec![];
        }
        if data == "[DONE]" {
            return self.finish();
        }
        let v: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(e) => return vec![Err(LlmError::Decode(format!("{e}: {data}")))],
        };
        if let Some(err) = v.get("error") {
            let message = err["message"].as_str().unwrap_or("stream error").to_owned();
            return vec![Err(LlmError::Provider {
                status: err["code"].as_u64().unwrap_or(500) as u16,
                message,
            })];
        }
        let mut out = Vec::new();
        if !self.started {
            self.started = true;
            out.push(ChatEvent::Start {
                id: v["id"].as_str().unwrap_or_default().to_owned(),
                model: v["model"].as_str().unwrap_or_default().to_owned(),
            });
        }
        if let Some(choice) = v["choices"].get(0) {
            let delta = &choice["delta"];
            for key in ["reasoning_content", "reasoning"] {
                if let Some(r) = delta[key].as_str().filter(|r| !r.is_empty()) {
                    self.emit(&mut out, Open::Reasoning, r);
                }
            }
            if let Some(text) = delta["content"].as_str().filter(|t| !t.is_empty()) {
                let pieces = match self.think.as_mut() {
                    Some(splitter) => splitter.push(text),
                    None => vec![(false, text.to_owned())],
                };
                for (reasoning, piece) in pieces {
                    self.emit(
                        &mut out,
                        if reasoning {
                            Open::Reasoning
                        } else {
                            Open::Text
                        },
                        &piece,
                    );
                }
            }
            if let Some(calls) = delta["tool_calls"].as_array() {
                for call in calls {
                    let tc = call["index"].as_u64().unwrap_or(0);
                    if !self.tool_blocks.contains_key(&tc) {
                        self.close(&mut out);
                        let index = self.next_index;
                        self.next_index += 1;
                        self.tool_blocks.insert(tc, index);
                        self.open = Some((Open::Tool(tc), index));
                        out.push(ChatEvent::BlockStart {
                            index,
                            block: BlockKind::ToolUse {
                                id: call["id"]
                                    .as_str()
                                    .map(str::to_owned)
                                    .unwrap_or_else(|| gai_core::new_id("call")),
                                name: call["function"]["name"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_owned(),
                            },
                        });
                    }
                    if let Some(args) = call["function"]["arguments"]
                        .as_str()
                        .filter(|a| !a.is_empty())
                    {
                        out.push(ChatEvent::ToolInputDelta {
                            index: self.tool_blocks[&tc],
                            json: args.to_owned(),
                        });
                    }
                }
            }
            if let Some(reason) = choice["finish_reason"].as_str() {
                self.stop_reason = Some(match reason {
                    "stop" => StopReason::EndTurn,
                    "tool_calls" | "function_call" => StopReason::ToolUse,
                    "length" => StopReason::MaxTokens,
                    "content_filter" => StopReason::Refusal,
                    other => StopReason::Other(other.to_owned()),
                });
            }
        }
        if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
            let n = |p: &str| u.pointer(p).and_then(Value::as_u64).unwrap_or(0);
            let cached = n("/prompt_tokens_details/cached_tokens");
            out.push(ChatEvent::Usage {
                usage: Usage {
                    input_tokens: n("/prompt_tokens").saturating_sub(cached),
                    output_tokens: n("/completion_tokens"),
                    cache_read_tokens: cached,
                    cache_write_tokens: 0,
                    reasoning_tokens: n("/completion_tokens_details/reasoning_tokens"),
                },
            });
        }
        out.into_iter().map(Ok).collect()
    }

    /// Flush and close. Runs at `[DONE]` or when the stream ends without it.
    pub(crate) fn finish(&mut self) -> Vec<Result<ChatEvent, LlmError>> {
        if self.finished || !self.started {
            return vec![];
        }
        self.finished = true;
        let mut out = Vec::new();
        if let Some(rest) = self.think.as_mut().and_then(ThinkSplitter::flush) {
            self.emit(
                &mut out,
                if rest.0 { Open::Reasoning } else { Open::Text },
                &rest.1,
            );
        }
        self.close(&mut out);
        let stop_reason = self
            .stop_reason
            .take()
            .unwrap_or(if self.tool_blocks.is_empty() {
                StopReason::EndTurn
            } else {
                StopReason::ToolUse
            });
        out.push(ChatEvent::Stop { stop_reason });
        out.into_iter().map(Ok).collect()
    }

    fn emit(&mut self, out: &mut Vec<ChatEvent>, kind: Open, text: &str) {
        let index = match self.open {
            Some((k, i)) if k == kind => i,
            _ => {
                self.close(out);
                let i = self.next_index;
                self.next_index += 1;
                self.open = Some((kind, i));
                let block = if kind == Open::Reasoning {
                    BlockKind::Thinking
                } else {
                    BlockKind::Text
                };
                out.push(ChatEvent::BlockStart { index: i, block });
                i
            }
        };
        let text = text.to_owned();
        out.push(if kind == Open::Reasoning {
            ChatEvent::ReasoningDelta { index, text }
        } else {
            ChatEvent::TextDelta { index, text }
        });
    }

    fn close(&mut self, out: &mut Vec<ChatEvent>) {
        if let Some((_, index)) = self.open.take() {
            out.push(ChatEvent::BlockStop { index });
        }
    }
}

/// Splits streamed text on `<think>` / `</think>`, coping with tags cut
/// across chunks. Yields `(is_reasoning, text)` pieces.
#[derive(Default)]
struct ThinkSplitter {
    in_think: bool,
    pending: String,
}

impl ThinkSplitter {
    const OPEN: &'static str = "<think>";
    const CLOSE: &'static str = "</think>";

    fn push(&mut self, text: &str) -> Vec<(bool, String)> {
        self.pending.push_str(text);
        let mut out = Vec::new();
        loop {
            let tag = if self.in_think {
                Self::CLOSE
            } else {
                Self::OPEN
            };
            if let Some(pos) = self.pending.find(tag) {
                let before: String = self.pending.drain(..pos).collect();
                self.pending.drain(..tag.len());
                if !before.is_empty() {
                    out.push((self.in_think, before));
                }
                self.in_think = !self.in_think;
                continue;
            }
            // Hold back a suffix that could be the start of the tag.
            let keep = (1..tag.len())
                .rev()
                .find(|&n| self.pending.ends_with(&tag[..n]))
                .unwrap_or(0);
            let emit_to = self.pending.len() - keep;
            if emit_to > 0 {
                let piece: String = self.pending.drain(..emit_to).collect();
                out.push((self.in_think, piece));
            }
            return out;
        }
    }

    fn flush(&mut self) -> Option<(bool, String)> {
        (!self.pending.is_empty()).then(|| (self.in_think, std::mem::take(&mut self.pending)))
    }
}

/// Speech, transcription and image models over the OpenAI audio and image
/// endpoints.
pub struct OpenAiMedia {
    info: ModelInfo,
    http: HttpTarget,
    opts: Arc<Options>,
}

#[async_trait]
impl SpeechToText for OpenAiMedia {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    async fn transcribe(&self, req: TranscribeRequest) -> Result<Transcript, LlmError> {
        use reqwest::multipart::{Form, Part};
        #[derive(Deserialize)]
        struct Resp {
            text: String,
            #[serde(default)]
            language: Option<String>,
            #[serde(default)]
            duration: Option<f64>,
            #[serde(default)]
            usage: Option<Value>,
        }
        let filename = req
            .filename
            .clone()
            .unwrap_or_else(|| format!("audio.{}", extension_for(&req.media_type)));
        let part = Part::bytes(req.audio)
            .file_name(filename)
            .mime_str(&req.media_type)
            .map_err(|e| LlmError::InvalidRequest(format!("media type: {e}")))?;
        // Whisper models also report the duration and language; newer
        // transcription models only answer plain `json`.
        let verbose = self.info.model.contains("whisper");
        let mut form = Form::new()
            .part("file", part)
            .text("model", self.info.model.clone())
            .text(
                "response_format",
                if verbose { "verbose_json" } else { "json" },
            );
        if let Some(l) = req.language {
            form = form.text("language", l);
        }
        if let Some(p) = req.prompt {
            form = form.text("prompt", p);
        }
        let r: Resp = send(
            self.http
                .post_multipart(&self.opts.transcription_path, form),
        )
        .await?
        .json()
        .await?;
        let seconds = r
            .duration
            .or_else(|| r.usage.as_ref().and_then(|u| u["seconds"].as_f64()));
        Ok(Transcript {
            text: r.text.trim().to_owned(),
            language: r.language,
            duration_secs: seconds,
            usage: MediaUsage {
                seconds: seconds.unwrap_or(0.0),
                ..Default::default()
            },
        })
    }
}

#[async_trait]
impl TextToSpeech for OpenAiMedia {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    async fn speak(&self, req: SpeechRequest) -> Result<Speech, LlmError> {
        let format = req.format.as_deref().unwrap_or("mp3").to_owned();
        let mut body = json!({
            "model": self.info.model,
            "input": req.text,
            "voice": req.voice.as_deref().unwrap_or("alloy"),
            "response_format": format,
        });
        if let Some(s) = req.speed {
            body["speed"] = json!(s);
        }
        if let Some(i) = &req.instructions {
            body["instructions"] = json!(i);
        }
        let resp = send(self.http.post_json("/audio/speech", body, &Map::new())).await?;
        let media_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .filter(|t| t.starts_with("audio/"))
            .map(str::to_owned)
            .unwrap_or_else(|| audio_media_type(&format).to_owned());
        let audio = resp.bytes().await?.to_vec();
        Ok(Speech {
            audio,
            media_type,
            usage: MediaUsage {
                chars: req.text.chars().count() as u64,
                ..Default::default()
            },
        })
    }
}

#[async_trait]
impl ImageModel for OpenAiMedia {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    async fn generate(&self, req: ImageRequest) -> Result<Images, LlmError> {
        use base64::Engine;
        #[derive(Deserialize)]
        struct Resp {
            data: Vec<Item>,
        }
        #[derive(Deserialize)]
        struct Item {
            #[serde(default)]
            b64_json: Option<String>,
            #[serde(default)]
            url: Option<String>,
            #[serde(default)]
            revised_prompt: Option<String>,
        }
        let n = req.n.unwrap_or(1).max(1);
        let mut body = json!({"model": self.info.model, "prompt": req.prompt, "n": n});
        if let Some(size) = &req.size {
            body["size"] = json!(size);
        }
        if let Some(q) = &req.quality {
            body["quality"] = json!(q);
        }
        // DALL-E answers with URLs unless asked for base64; gpt-image
        // models always send base64 and reject the field.
        if self.info.model.starts_with("dall-e") {
            body["response_format"] = json!("b64_json");
        }
        let r: Resp = send(
            self.http
                .post_json("/images/generations", body, &Map::new()),
        )
        .await?
        .json()
        .await?;
        let mut images = Vec::new();
        for item in r.data {
            let data = match (item.b64_json, item.url) {
                (Some(b), _) => base64::engine::general_purpose::STANDARD
                    .decode(b.trim())
                    .map_err(|e| LlmError::Decode(format!("image data: {e}")))?,
                (None, Some(url)) => send(self.http.fetch(&url)).await?.bytes().await?.to_vec(),
                (None, None) => continue,
            };
            images.push(GeneratedImage {
                media_type: sniff_image(&data).to_owned(),
                data,
                revised_prompt: item.revised_prompt,
            });
        }
        let count = images.len() as u32;
        Ok(Images {
            images,
            usage: MediaUsage {
                images: count,
                ..Default::default()
            },
        })
    }
}

pub struct OpenAiEmbedding {
    info: ModelInfo,
    http: HttpTarget,
}

#[async_trait]
impl EmbeddingModel for OpenAiEmbedding {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    async fn embed(&self, inputs: &[String]) -> Result<EmbedResponse, LlmError> {
        #[derive(Deserialize)]
        struct Resp {
            data: Vec<Item>,
            #[serde(default)]
            usage: Option<Value>,
        }
        #[derive(Deserialize)]
        struct Item {
            index: usize,
            embedding: Vec<f32>,
        }
        let mut body = json!({"model": self.info.model, "input": inputs});
        if let Some(d) = self.info.capabilities.dimensions {
            body["dimensions"] = json!(d);
        }
        let mut resp: Resp = send(self.http.post_json("/embeddings", body, &Map::new()))
            .await?
            .json()
            .await?;
        resp.data.sort_by_key(|i| i.index);
        let input_tokens = resp
            .usage
            .as_ref()
            .and_then(|u| u["prompt_tokens"].as_u64())
            .unwrap_or(0);
        Ok(EmbedResponse {
            vectors: resp.data.into_iter().map(|i| i.embedding).collect(),
            usage: Usage {
                input_tokens,
                ..Default::default()
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Accumulator, Reasoning};
    use gai_core::ToolSpec;

    fn run(parser: &mut StreamParser, chunks: &[&str]) -> crate::ChatResponse {
        let mut acc = Accumulator::new("p/m");
        for c in chunks {
            for e in parser.parse(c) {
                acc.push(&e.unwrap());
            }
        }
        for e in parser.finish() {
            acc.push(&e.unwrap());
        }
        acc.finish()
    }

    #[test]
    fn parses_text_reasoning_tools_and_usage() {
        let mut p = StreamParser::new(false);
        let r = run(
            &mut p,
            &[
                r#"{"id":"c1","model":"m","choices":[{"index":0,"delta":{"role":"assistant","reasoning_content":"think"}}]}"#,
                r#"{"id":"c1","choices":[{"index":0,"delta":{"content":"Hel"}}]}"#,
                r#"{"id":"c1","choices":[{"index":0,"delta":{"content":"lo"}}]}"#,
                r#"{"id":"c1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"calc","arguments":"{\"a\""}}]}}]}"#,
                r#"{"id":"c1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":":1}"}}]}}]}"#,
                r#"{"id":"c1","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
                r#"{"id":"c1","choices":[],"usage":{"prompt_tokens":100,"completion_tokens":7,"prompt_tokens_details":{"cached_tokens":60}}}"#,
                "[DONE]",
            ],
        );
        assert_eq!(r.stop_reason, StopReason::ToolUse);
        assert_eq!(r.content.len(), 3);
        assert!(matches!(&r.content[0], ContentBlock::Thinking { text, .. } if text == "think"));
        assert_eq!(r.text(), "Hello");
        assert_eq!(
            r.content[2],
            ContentBlock::ToolUse {
                id: "call_1".into(),
                name: "calc".into(),
                input: json!({"a": 1})
            }
        );
        assert_eq!(
            (
                r.usage.input_tokens,
                r.usage.cache_read_tokens,
                r.usage.output_tokens
            ),
            (40, 60, 7)
        );
    }

    #[test]
    fn think_tags_split_across_chunks() {
        let mut p = StreamParser::new(true);
        let mk = |t: &str| {
            json!({"id": "x", "choices": [{"index": 0, "delta": {"content": t}}]}).to_string()
        };
        let chunks: Vec<String> = ["<thi", "nk>pondering</th", "ink>The answer", " is 4"]
            .iter()
            .map(|c| mk(c))
            .collect();
        let refs: Vec<&str> = chunks.iter().map(String::as_str).collect();
        let r = run(&mut p, &refs);
        assert!(
            matches!(&r.content[0], ContentBlock::Thinking { text, .. } if text == "pondering")
        );
        assert_eq!(r.text(), "The answer is 4");
        assert_eq!(r.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn tool_results_become_tool_messages() {
        let mut out = Vec::new();
        push_message(
            &mut out,
            &Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Thinking {
                        text: "x".into(),
                        signature: None,
                        origin: None,
                    },
                    ContentBlock::ToolUse {
                        id: "c1".into(),
                        name: "calc".into(),
                        input: json!({"a": 1}),
                    },
                ],
            },
        );
        push_message(
            &mut out,
            &Message {
                role: Role::User,
                content: vec![
                    ContentBlock::tool_result("c1", "2", false),
                    ContentBlock::text("thanks"),
                ],
            },
        );
        assert_eq!(out[0]["content"], Value::Null);
        assert_eq!(
            out[0]["tool_calls"][0]["function"]["arguments"],
            "{\"a\":1}"
        );
        assert_eq!(
            out[1],
            json!({"role": "tool", "tool_call_id": "c1", "content": "2"})
        );
        assert_eq!(out[2], json!({"role": "user", "content": "thanks"}));
    }

    #[test]
    fn body_maps_reasoning_and_schema() {
        let cfg = ProviderConfig::new("openai", "http://x");
        let mut info = ModelInfo::new("openai", "o-model");
        info.capabilities.reasoning = true;
        let m = OpenAiModel {
            info,
            http: HttpTarget::new("openai", &cfg, "http://x", ("authorization", "Bearer")).unwrap(),
            opts: Arc::new(Options {
                max_tokens_field: "max_completion_tokens".into(),
                think_tags: false,
                reasoning_effort: None,
                stream_usage: true,
                transcription_path: "/audio/transcriptions".into(),
            }),
        };
        let mut req = ChatRequest::user("hi")
            .with_system("sys")
            .with_tools(vec![ToolSpec {
                name: "t".into(),
                description: "d".into(),
                input_schema: json!({"type": "object"}),
                strict: false,
            }]);
        req.max_tokens = Some(50);
        req.reasoning = Some(Reasoning {
            effort: Effort::Max,
            ..Default::default()
        });
        req.tool_choice = ToolChoice::Required;
        let b = m.body(&req);
        assert_eq!(b["messages"][0]["role"], "system");
        assert_eq!(b["max_completion_tokens"], 50);
        assert_eq!(b["reasoning_effort"], "high");
        assert_eq!(b["tool_choice"], "required");
        assert_eq!(b["stream_options"]["include_usage"], true);
    }
}

#[cfg(test)]
mod media_tests {
    use base64::Engine;
    use wiremock::matchers::{body_partial_json, body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::{LlmConfig, ModelRegistry};

    async fn registry(server: &MockServer) -> ModelRegistry {
        let mut cfg = LlmConfig::default();
        cfg.providers.insert(
            "oa".into(),
            ProviderConfig::new("openai", &format!("{}/v1", server.uri())),
        );
        ModelRegistry::from_config(cfg).unwrap()
    }

    #[tokio::test]
    async fn transcribes_speaks_and_draws() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .and(body_string_contains("verbose_json"))
            .and(body_string_contains("whisper-1"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    json!({"text": " Hello. ", "language": "english", "duration": 2.5}),
                ),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/speech"))
            .and(body_partial_json(
                json!({"model": "tts-1", "input": "hi", "voice": "nova", "response_format": "wav"}),
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "audio/wav")
                    .set_body_bytes(b"RIFFdata".to_vec()),
            )
            .mount(&server)
            .await;
        let png = [0x89u8, b'P', b'N', b'G', 1, 2, 3];
        let b64 = base64::engine::general_purpose::STANDARD.encode(png);
        Mock::given(method("POST"))
            .and(path("/v1/images/generations"))
            .and(body_partial_json(
                json!({"model": "dall-e-3", "prompt": "a fox", "response_format": "b64_json"}),
            ))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    json!({"data": [{"b64_json": b64, "revised_prompt": "a red fox"}]}),
                ),
            )
            .mount(&server)
            .await;
        let r = registry(&server).await;

        let t = r
            .transcription("oa/whisper-1")
            .unwrap()
            .transcribe(TranscribeRequest {
                audio: vec![0; 10],
                media_type: "audio/webm".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(t.text, "Hello.");
        assert_eq!(t.usage.seconds, 2.5);

        let sp = r
            .speech("oa/tts-1")
            .unwrap()
            .speak(SpeechRequest {
                text: "hi".into(),
                voice: Some("nova".into()),
                format: Some("wav".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            (sp.audio.as_slice(), sp.media_type.as_str()),
            (&b"RIFFdata"[..], "audio/wav")
        );

        let img = r
            .image("oa/dall-e-3")
            .unwrap()
            .generate(ImageRequest {
                prompt: "a fox".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(img.images[0].data, png);
        assert_eq!(img.images[0].media_type, "image/png");
        assert_eq!(img.images[0].revised_prompt.as_deref(), Some("a red fox"));
        assert_eq!(img.usage.images, 1);
    }
}
