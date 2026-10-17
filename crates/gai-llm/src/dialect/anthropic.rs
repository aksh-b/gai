//! The Anthropic Messages API (`POST /v1/messages`).
//!
//! Options (`[providers.<name>.options]`):
//! - `betas`: list of `anthropic-beta` header values.
//! - `eager_tool_streaming` (default true): stream tool arguments as they
//!   are generated. Turn off for proxies that reject the field.
//! - `auto_cache` (default true): top-level `cache_control` so the API
//!   caches the conversation prefix automatically.

use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use gai_core::{ContentBlock, Message, Role, ToolChoice, Usage};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::{Dialect, Provider, RemoteModel};
use crate::http::{HttpTarget, send, sse};
use crate::{
    BlockKind, ChatEvent, ChatModel, ChatRequest, ChatStream, Effort, LlmError, ModelInfo,
    ProviderConfig, ResponseFormat, SharedChatModel, StopReason,
};

const API_VERSION: &str = "2023-06-01";

pub struct AnthropicDialect;

impl Dialect for AnthropicDialect {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn build(&self, provider: &str, cfg: &ProviderConfig) -> Result<Arc<dyn Provider>, LlmError> {
        let mut cfg = cfg.clone();
        cfg.headers
            .entry("anthropic-version".into())
            .or_insert_with(|| API_VERSION.into());
        if let Some(betas) = cfg.options.get("betas").and_then(Value::as_array) {
            let joined = betas
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(",");
            if !joined.is_empty() {
                cfg.headers.insert("anthropic-beta".into(), joined);
            }
        }
        let http = HttpTarget::new(
            provider,
            &cfg,
            "https://api.anthropic.com",
            ("x-api-key", ""),
        )?;
        Ok(Arc::new(AnthropicProvider {
            name: provider.to_owned(),
            http,
            eager_tools: cfg.option_bool("eager_tool_streaming").unwrap_or(true),
            auto_cache: cfg.option_bool("auto_cache").unwrap_or(true),
        }))
    }
}

struct AnthropicProvider {
    name: String,
    http: HttpTarget,
    eager_tools: bool,
    auto_cache: bool,
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn chat(&self, info: ModelInfo) -> Result<SharedChatModel, LlmError> {
        Ok(Arc::new(AnthropicModel {
            info,
            http: self.http.clone(),
            eager_tools: self.eager_tools,
            auto_cache: self.auto_cache,
        }))
    }

    async fn list_models(&self) -> Result<Vec<RemoteModel>, LlmError> {
        #[derive(Deserialize)]
        struct Page {
            data: Vec<Item>,
        }
        #[derive(Deserialize)]
        struct Item {
            id: String,
            display_name: Option<String>,
            max_input_tokens: Option<u32>,
            max_tokens: Option<u32>,
        }
        let page: Page = send(self.http.get("/v1/models").query(&[("limit", "1000")]))
            .await?
            .json()
            .await?;
        Ok(page
            .data
            .into_iter()
            .map(|m| RemoteModel {
                id: m.id,
                display_name: m.display_name,
                context_window: m.max_input_tokens,
                max_output_tokens: m.max_tokens,
            })
            .collect())
    }
}

pub struct AnthropicModel {
    info: ModelInfo,
    http: HttpTarget,
    eager_tools: bool,
    auto_cache: bool,
}

/// How a Claude model takes reasoning settings.
#[derive(Debug, PartialEq)]
enum ThinkingStyle {
    /// `thinking: {type: "adaptive"}` plus `output_config.effort`.
    Adaptive,
    /// `thinking: {type: "enabled", budget_tokens}` (older models).
    Budget,
}

fn thinking_style(model: &str) -> ThinkingStyle {
    let adaptive_prefixes = [
        "claude-fable-",
        "claude-mythos-",
        "claude-opus-5",
        "claude-sonnet-5",
        "claude-haiku-5",
        "claude-opus-4-6",
        "claude-opus-4-7",
        "claude-opus-4-8",
        "claude-sonnet-4-6",
    ];
    if adaptive_prefixes.iter().any(|p| model.starts_with(p)) {
        ThinkingStyle::Adaptive
    } else {
        ThinkingStyle::Budget
    }
}

impl AnthropicModel {
    fn body(&self, req: &ChatRequest, stream: bool) -> Value {
        let caps = &self.info.capabilities;
        let mut max_tokens = req.max_tokens.unwrap_or_else(|| {
            caps.max_output_tokens
                .unwrap_or(16_384)
                .min(if stream { 64_000 } else { 16_000 })
        });
        let mut body = Map::new();
        body.insert("model".into(), json!(self.info.model));

        if let Some(system) = req.system.as_deref().filter(|s| !s.is_empty()) {
            body.insert(
                "system".into(),
                if req.cache {
                    json!([{"type": "text", "text": system, "cache_control": {"type": "ephemeral"}}])
                } else {
                    json!(system)
                },
            );
        }
        body.insert(
            "messages".into(),
            Value::Array(
                req.messages
                    .iter()
                    .filter_map(|m| self.message(m))
                    .collect(),
            ),
        );

        if !req.tools.is_empty() {
            let tools: Vec<Value> = req
                .tools
                .iter()
                .map(|t| {
                    let mut v = json!({"name": t.name, "description": t.description, "input_schema": t.input_schema});
                    if t.strict {
                        v["strict"] = json!(true);
                    }
                    if stream && self.eager_tools {
                        v["eager_input_streaming"] = json!(true);
                    }
                    v
                })
                .collect();
            body.insert("tools".into(), Value::Array(tools));
            match &req.tool_choice {
                ToolChoice::Auto => {}
                ToolChoice::None => _ = body.insert("tool_choice".into(), json!({"type": "none"})),
                ToolChoice::Required => {
                    _ = body.insert("tool_choice".into(), json!({"type": "any"}))
                }
                ToolChoice::Tool { name } => {
                    _ = body.insert("tool_choice".into(), json!({"type": "tool", "name": name}))
                }
            }
        }

        let mut output_config = Map::new();
        if let Some(r) = &req.reasoning {
            match thinking_style(&self.info.model) {
                ThinkingStyle::Adaptive => {
                    // Several current models refuse to turn thinking off, so
                    // "off" means: leave thinking at its default, lowest effort.
                    if r.effort == Effort::Off {
                        output_config.insert("effort".into(), json!("low"));
                    } else {
                        let mut thinking = json!({"type": "adaptive"});
                        if r.expose {
                            thinking["display"] = json!("summarized");
                        }
                        body.insert("thinking".into(), thinking);
                        output_config.insert("effort".into(), json!(r.effort.as_str()));
                    }
                }
                ThinkingStyle::Budget if r.effort != Effort::Off => {
                    let budget = r.budget_tokens.unwrap_or(match r.effort {
                        Effort::Low => 2_048,
                        Effort::Medium => 8_192,
                        Effort::High => 16_384,
                        _ => 32_000,
                    });
                    max_tokens = max_tokens.max(budget + 4_096);
                    body.insert(
                        "thinking".into(),
                        json!({"type": "enabled", "budget_tokens": budget}),
                    );
                }
                ThinkingStyle::Budget => {}
            }
        }
        if let Some(ResponseFormat::JsonSchema { schema, .. }) = &req.response_format {
            output_config.insert(
                "format".into(),
                json!({"type": "json_schema", "schema": schema}),
            );
        }
        if !output_config.is_empty() {
            body.insert("output_config".into(), Value::Object(output_config));
        }

        body.insert("max_tokens".into(), json!(max_tokens));
        if let Some(t) = req.temperature {
            body.insert("temperature".into(), json!(t));
        }
        if let Some(p) = req.top_p {
            body.insert("top_p".into(), json!(p));
        }
        if !req.stop.is_empty() {
            body.insert("stop_sequences".into(), json!(req.stop));
        }
        if req.cache && self.auto_cache {
            body.insert("cache_control".into(), json!({"type": "ephemeral"}));
        }
        if stream {
            body.insert("stream".into(), json!(true));
        }
        Value::Object(body)
    }

    fn message(&self, m: &Message) -> Option<Value> {
        let content: Vec<Value> = m.content.iter().filter_map(|b| self.block(b)).collect();
        if content.is_empty() {
            return None;
        }
        let role = match m.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        Some(json!({"role": role, "content": content}))
    }

    fn block(&self, b: &ContentBlock) -> Option<Value> {
        // Reasoning only goes back to the model that produced it; others
        // would reject or ignore it.
        let own = |origin: &Option<String>| origin.as_deref() == Some(self.info.id.as_str());
        Some(match b {
            ContentBlock::Text { text } if text.is_empty() => return None,
            ContentBlock::Text { text } => json!({"type": "text", "text": text}),
            ContentBlock::Image { source } => json!({"type": "image", "source": media(source)}),
            // The Messages API takes no audio input.
            ContentBlock::Audio { .. } => {
                json!({"type": "text", "text": "[audio omitted: this model cannot hear]"})
            }
            ContentBlock::ToolUse { id, name, input } => {
                let input = if input.is_object() {
                    input.clone()
                } else {
                    json!({})
                };
                json!({"type": "tool_use", "id": id, "name": name, "input": input})
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                let inner: Vec<Value> = content.iter().filter_map(|c| self.block(c)).collect();
                let mut v =
                    json!({"type": "tool_result", "tool_use_id": tool_use_id, "content": inner});
                if *is_error {
                    v["is_error"] = json!(true);
                }
                v
            }
            ContentBlock::Thinking {
                text,
                signature,
                origin,
            } if own(origin) => {
                json!({"type": "thinking", "thinking": text, "signature": signature.clone().unwrap_or_default()})
            }
            ContentBlock::RedactedThinking { data, origin } if own(origin) => {
                json!({"type": "redacted_thinking", "data": data})
            }
            ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. } => return None,
        })
    }
}

fn media(source: &gai_core::MediaSource) -> Value {
    match source {
        gai_core::MediaSource::Base64 { media_type, data } => {
            json!({"type": "base64", "media_type": media_type, "data": data})
        }
        gai_core::MediaSource::Url { url } => json!({"type": "url", "url": url}),
        // Resolved by the runtime before requests are sent.
        gai_core::MediaSource::Blob { .. } => json!({"type": "url", "url": source.to_url()}),
    }
}

#[async_trait]
impl ChatModel for AnthropicModel {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    async fn stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        let body = self.body(&req, true);
        let resp = send(self.http.post_json("/v1/messages", body, &req.extra)).await?;
        let mut parser = StreamParser::default();
        let events = sse(resp).flat_map(move |item| {
            let out: Vec<Result<ChatEvent, LlmError>> = match item {
                Ok(e) => parser.parse(&e.event, &e.data),
                Err(e) => vec![Err(e)],
            };
            futures::stream::iter(out)
        });
        Ok(Box::pin(events))
    }

    async fn count_tokens(&self, req: &ChatRequest) -> Result<Option<u64>, LlmError> {
        let mut body = self.body(req, false);
        if let Value::Object(o) = &mut body {
            o.remove("max_tokens");
            o.remove("cache_control");
        }
        #[derive(Deserialize)]
        struct Count {
            input_tokens: u64,
        }
        let c: Count = send(
            self.http
                .post_json("/v1/messages/count_tokens", body, &Map::new()),
        )
        .await?
        .json()
        .await?;
        Ok(Some(c.input_tokens))
    }
}

/// Turns Messages API SSE events into [`ChatEvent`]s.
#[derive(Default)]
pub(crate) struct StreamParser {
    stop_reason: Option<StopReason>,
    usage: Usage,
}

impl StreamParser {
    pub(crate) fn parse(&mut self, event: &str, data: &str) -> Vec<Result<ChatEvent, LlmError>> {
        if event == "ping" || data.is_empty() {
            return vec![];
        }
        let v: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(e) => return vec![Err(LlmError::Decode(format!("{e}: {data}")))],
        };
        let kind = v["type"].as_str().unwrap_or(event);
        let index = v["index"].as_u64().unwrap_or(0) as usize;
        let s = |p: &str| {
            v.pointer(p)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        match kind {
            "message_start" => {
                self.read_usage(&v["message"]["usage"]);
                vec![
                    Ok(ChatEvent::Start {
                        id: s("/message/id"),
                        model: s("/message/model"),
                    }),
                    Ok(ChatEvent::Usage { usage: self.usage }),
                ]
            }
            "content_block_start" => {
                let block = &v["content_block"];
                let kind = match block["type"].as_str() {
                    Some("text") => BlockKind::Text,
                    Some("thinking") => BlockKind::Thinking,
                    Some("redacted_thinking") => BlockKind::RedactedThinking {
                        data: s("/content_block/data"),
                    },
                    Some("tool_use") => BlockKind::ToolUse {
                        id: s("/content_block/id"),
                        name: s("/content_block/name"),
                    },
                    // Server-side tool blocks are not surfaced yet.
                    _ => return vec![],
                };
                let mut out = vec![Ok(ChatEvent::BlockStart { index, block: kind })];
                if let Some(t) = block["text"].as_str().filter(|t| !t.is_empty()) {
                    out.push(Ok(ChatEvent::TextDelta {
                        index,
                        text: t.to_owned(),
                    }));
                }
                out
            }
            "content_block_delta" => {
                let d = &v["delta"];
                let text = |k: &str| d[k].as_str().unwrap_or_default().to_owned();
                match d["type"].as_str() {
                    Some("text_delta") => vec![Ok(ChatEvent::TextDelta {
                        index,
                        text: text("text"),
                    })],
                    Some("thinking_delta") => vec![Ok(ChatEvent::ReasoningDelta {
                        index,
                        text: text("thinking"),
                    })],
                    Some("signature_delta") => vec![Ok(ChatEvent::SignatureDelta {
                        index,
                        signature: text("signature"),
                    })],
                    Some("input_json_delta") => vec![Ok(ChatEvent::ToolInputDelta {
                        index,
                        json: text("partial_json"),
                    })],
                    _ => vec![],
                }
            }
            "content_block_stop" => vec![Ok(ChatEvent::BlockStop { index })],
            "message_delta" => {
                if let Some(r) = v["delta"]["stop_reason"].as_str() {
                    self.stop_reason = Some(stop_reason(r));
                }
                self.read_usage(&v["usage"]);
                vec![Ok(ChatEvent::Usage { usage: self.usage })]
            }
            "message_stop" => {
                vec![Ok(ChatEvent::Stop {
                    stop_reason: self.stop_reason.take().unwrap_or_default(),
                })]
            }
            "error" => {
                let message = s("/error/message");
                let err = match v["error"]["type"].as_str() {
                    Some("rate_limit_error") => LlmError::RateLimited {
                        retry_after: None,
                        message,
                    },
                    Some("overloaded_error") => LlmError::Provider {
                        status: 529,
                        message,
                    },
                    Some("invalid_request_error") => LlmError::InvalidRequest(message),
                    Some("authentication_error" | "permission_error") => LlmError::Auth(message),
                    _ => LlmError::Provider {
                        status: 500,
                        message,
                    },
                };
                vec![Err(err)]
            }
            _ => vec![],
        }
    }

    fn read_usage(&mut self, u: &Value) {
        let n = |k: &str| u[k].as_u64();
        let snapshot = Usage {
            input_tokens: n("input_tokens").unwrap_or(0),
            output_tokens: n("output_tokens").unwrap_or(0),
            cache_read_tokens: n("cache_read_input_tokens").unwrap_or(0),
            cache_write_tokens: n("cache_creation_input_tokens").unwrap_or(0),
            reasoning_tokens: 0,
        };
        self.usage.merge_max(&snapshot);
    }
}

fn stop_reason(s: &str) -> StopReason {
    match s {
        "end_turn" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" | "model_context_window_exceeded" => StopReason::MaxTokens,
        "stop_sequence" => StopReason::StopSequence,
        "refusal" => StopReason::Refusal,
        "pause_turn" => StopReason::PauseTurn,
        other => StopReason::Other(other.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Reasoning, ResponseFormat};
    use gai_core::ToolSpec;

    fn model(name: &str) -> AnthropicModel {
        let cfg = ProviderConfig::new("anthropic", "http://x");
        let mut info = ModelInfo::new("anthropic", name);
        info.capabilities.max_output_tokens = Some(128_000);
        AnthropicModel {
            info,
            http: HttpTarget::new("anthropic", &cfg, "http://x", ("x-api-key", "")).unwrap(),
            eager_tools: true,
            auto_cache: true,
        }
    }

    #[test]
    fn adaptive_models_get_effort_and_cache_breakpoints() {
        let m = model("claude-opus-5-5");
        let mut req = ChatRequest::user("hi")
            .with_system("sys")
            .with_tools(vec![ToolSpec {
                name: "calc".into(),
                description: "math".into(),
                input_schema: json!({"type": "object"}),
                strict: true,
            }]);
        req.reasoning = Some(Reasoning {
            effort: Effort::High,
            budget_tokens: None,
            expose: true,
        });
        let b = m.body(&req, true);
        assert_eq!(
            b["thinking"],
            json!({"type": "adaptive", "display": "summarized"})
        );
        assert_eq!(b["output_config"]["effort"], "high");
        assert_eq!(b["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(b["cache_control"]["type"], "ephemeral");
        assert_eq!(b["tools"][0]["strict"], true);
        assert_eq!(b["tools"][0]["eager_input_streaming"], true);
        assert_eq!(b["max_tokens"], 64_000);
        assert!(b.get("tool_choice").is_none());
    }

    #[test]
    fn off_effort_never_disables_thinking_on_adaptive_models() {
        let m = model("claude-sonnet-5-5");
        let b = m.body(&ChatRequest::user("hi").with_reasoning(Effort::Off), true);
        assert!(b.get("thinking").is_none());
        assert_eq!(b["output_config"]["effort"], "low");
    }

    #[test]
    fn budget_models_get_budget_tokens_below_max_tokens() {
        let m = model("claude-haiku-4-5");
        let mut req = ChatRequest::user("hi").with_reasoning(Effort::High);
        req.max_tokens = Some(1000);
        req.response_format = Some(ResponseFormat::JsonSchema {
            name: "x".into(),
            schema: json!({}),
            strict: true,
        });
        let b = m.body(&req, false);
        assert_eq!(b["thinking"]["budget_tokens"], 16_384);
        assert!(b["max_tokens"].as_u64().unwrap() > 16_384);
        assert_eq!(b["output_config"]["format"]["type"], "json_schema");
    }

    #[test]
    fn foreign_reasoning_is_dropped_and_own_kept() {
        let m = model("claude-opus-5-5");
        let msg = Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Thinking {
                    text: "a".into(),
                    signature: Some("s".into()),
                    origin: Some("openai/x".into()),
                },
                ContentBlock::Thinking {
                    text: "b".into(),
                    signature: Some("s".into()),
                    origin: Some("anthropic/claude-opus-5-5".into()),
                },
                ContentBlock::text("done"),
            ],
        };
        let v = m.message(&msg).unwrap();
        let content = v["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["thinking"], "b");
    }

    #[test]
    fn parses_a_tool_use_stream() {
        let mut p = StreamParser::default();
        let events = [
            (
                "message_start",
                r#"{"type":"message_start","message":{"id":"msg_1","model":"claude-opus-5-5","usage":{"input_tokens":10,"cache_read_input_tokens":4,"output_tokens":1}}}"#,
            ),
            (
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"calc","input":{}}}"#,
            ),
            (
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"a\": 1}"}}"#,
            ),
            (
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            ),
            (
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":20}}"#,
            ),
            ("message_stop", r#"{"type":"message_stop"}"#),
        ];
        let mut acc = crate::Accumulator::new("anthropic/claude-opus-5-5");
        for (e, d) in events {
            for ev in p.parse(e, d) {
                acc.push(&ev.unwrap());
            }
        }
        let r = acc.finish();
        assert_eq!(r.stop_reason, StopReason::ToolUse);
        assert_eq!(
            r.usage,
            Usage {
                input_tokens: 10,
                output_tokens: 20,
                cache_read_tokens: 4,
                ..Default::default()
            }
        );
        assert_eq!(
            r.content,
            vec![ContentBlock::ToolUse {
                id: "toolu_1".into(),
                name: "calc".into(),
                input: json!({"a": 1})
            }]
        );
    }

    #[test]
    fn stream_errors_are_typed() {
        let mut p = StreamParser::default();
        let out = p.parse(
            "error",
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        );
        assert!(matches!(&out[0], Err(e) if e.is_retryable()));
    }
}
