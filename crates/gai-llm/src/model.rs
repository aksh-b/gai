use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use gai_core::{ContentBlock, Message, Pricing, Role, ToolChoice, ToolSpec, Usage};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::LlmError;

/// A provider-neutral chat request. The model is not part of it: a
/// [`ChatModel`] handle is already bound to one model.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChatRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolSpec>,
    #[serde(default)]
    pub tool_choice: ToolChoice,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
    /// Let dialects that support prompt caching mark the stable prefix
    /// (system prompt, tools, prior turns) as cacheable.
    #[serde(default = "yes")]
    pub cache: bool,
    /// Provider-specific fields merged into the request body as-is.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub extra: Map<String, Value>,
}

fn yes() -> bool {
    true
}

impl ChatRequest {
    pub fn new(messages: Vec<Message>) -> Self {
        Self {
            messages,
            cache: true,
            ..Default::default()
        }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self::new(vec![Message::user(text)])
    }

    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    pub fn with_tools(mut self, tools: Vec<ToolSpec>) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_reasoning(mut self, effort: Effort) -> Self {
        self.reasoning = Some(Reasoning {
            effort,
            ..Default::default()
        });
        self
    }
}

/// How hard the model should think. Each dialect maps this onto whatever its
/// provider offers (effort levels, thinking budgets, or nothing).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Reasoning {
    pub effort: Effort,
    /// A token budget, for providers that size thinking by budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u32>,
    /// Return readable reasoning (a summary where that is all the provider
    /// gives) instead of hiding it.
    #[serde(default)]
    pub expose: bool,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Off,
    Low,
    #[default]
    Medium,
    High,
    #[serde(alias = "x-high")]
    Xhigh,
    Max,
}

impl Effort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

impl std::str::FromStr for Effort {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        serde_json::from_value(Value::String(s.to_lowercase()))
            .map_err(|_| format!("unknown effort {s:?}; use off, low, medium, high, xhigh or max"))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    Text,
    Json,
    JsonSchema {
        name: String,
        schema: Value,
        #[serde(default)]
        strict: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChatResponse {
    pub id: String,
    pub model: String,
    pub content: Vec<ContentBlock>,
    pub stop_reason: StopReason,
    pub usage: Usage,
}

impl ChatResponse {
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(ContentBlock::as_text)
            .collect()
    }

    pub fn into_message(self) -> Message {
        Message {
            role: Role::Assistant,
            content: self.content,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    #[default]
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
    /// The provider's safety systems declined the request.
    Refusal,
    /// A long server-side turn paused; send the response back to continue.
    PauseTurn,
    #[serde(untagged)]
    Other(String),
}

/// The kind of block a stream event opens.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BlockKind {
    Text,
    Thinking,
    RedactedThinking { data: String },
    ToolUse { id: String, name: String },
}

/// One streaming event. Every dialect normalises its wire stream to this.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatEvent {
    Start {
        id: String,
        model: String,
    },
    BlockStart {
        index: usize,
        block: BlockKind,
    },
    TextDelta {
        index: usize,
        text: String,
    },
    ReasoningDelta {
        index: usize,
        text: String,
    },
    SignatureDelta {
        index: usize,
        signature: String,
    },
    ToolInputDelta {
        index: usize,
        json: String,
    },
    BlockStop {
        index: usize,
    },
    /// A cumulative usage snapshot; later snapshots supersede earlier ones.
    Usage {
        usage: Usage,
    },
    Stop {
        stop_reason: StopReason,
    },
    /// Sent first by a `router/*` model: which model it picked and why.
    Routed {
        /// The real model, `provider/model`.
        model: String,
        reason: String,
        /// Its prices, so the caller can work out the cost.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pricing: Option<Pricing>,
        /// What routing itself cost (the classifier call), in USD.
        #[serde(default)]
        extra_cost: f64,
    },
}

pub type ChatStream = Pin<Box<dyn Stream<Item = Result<ChatEvent, LlmError>> + Send>>;

/// What a model can do. Unknown fields default to what most chat models
/// support today.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct Capabilities {
    pub tools: bool,
    pub vision: bool,
    pub reasoning: bool,
    pub json_schema: bool,
    pub prompt_caching: bool,
    /// Takes audio input (`ContentBlock::Audio`).
    pub audio: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// Embedding vector size, for embedding models.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<u32>,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            tools: true,
            vision: false,
            reasoning: false,
            json_schema: false,
            prompt_caching: false,
            audio: false,
            context_window: None,
            max_output_tokens: None,
            dimensions: None,
        }
    }
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ModelKind {
    #[default]
    Chat,
    Embedding,
    /// Speech to text.
    Transcription,
    /// Text to speech.
    Speech,
    /// Image generation.
    Image,
}

/// A resolved model: its address, what it can do and what it costs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ModelInfo {
    /// `provider/model`, the address used everywhere in gai.
    pub id: String,
    pub provider: String,
    /// The model name the provider expects on the wire.
    pub model: String,
    #[serde(default)]
    pub kind: ModelKind,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing: Option<Pricing>,
    /// Prices for speech and image models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_pricing: Option<crate::MediaPricing>,
}

impl ModelInfo {
    pub fn new(provider: &str, model: &str) -> Self {
        Self {
            id: format!("{provider}/{model}"),
            provider: provider.to_owned(),
            model: model.to_owned(),
            kind: ModelKind::Chat,
            capabilities: Capabilities::default(),
            pricing: None,
            media_pricing: None,
        }
    }

    pub fn cost(&self, usage: &Usage) -> Option<f64> {
        self.pricing.map(|p| p.cost(usage))
    }
}

#[async_trait]
pub trait ChatModel: Send + Sync {
    fn info(&self) -> &ModelInfo;

    /// Stream a response. Every dialect implements this; [`complete`] folds
    /// the stream, so there is one code path per provider.
    ///
    /// [`complete`]: ChatModel::complete
    async fn stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError>;

    async fn complete(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let stream = self.stream(req).await?;
        collect(stream, &self.info().id).await
    }

    /// Exact input token count from the provider, when it offers one.
    async fn count_tokens(&self, _req: &ChatRequest) -> Result<Option<u64>, LlmError> {
        Ok(None)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EmbedResponse {
    pub vectors: Vec<Vec<f32>>,
    pub usage: Usage,
}

#[async_trait]
pub trait EmbeddingModel: Send + Sync {
    fn info(&self) -> &ModelInfo;
    async fn embed(&self, inputs: &[String]) -> Result<EmbedResponse, LlmError>;
}

pub type SharedChatModel = Arc<dyn ChatModel>;
pub type SharedEmbeddingModel = Arc<dyn EmbeddingModel>;

/// Drain a stream into a complete response.
pub async fn collect(mut stream: ChatStream, origin: &str) -> Result<ChatResponse, LlmError> {
    let mut acc = Accumulator::new(origin);
    while let Some(event) = stream.next().await {
        acc.push(&event?);
    }
    Ok(acc.finish())
}

/// Folds [`ChatEvent`]s into a [`ChatResponse`]. Used by `complete`, by the
/// agent runtime while it forwards events live, and by tests.
#[derive(Debug)]
pub struct Accumulator {
    origin: String,
    id: String,
    model: String,
    blocks: Vec<Partial>,
    usage: Usage,
    stop_reason: Option<StopReason>,
}

#[derive(Debug)]
enum Partial {
    Text(String),
    Thinking {
        text: String,
        signature: Option<String>,
    },
    Redacted(String),
    Tool {
        id: String,
        name: String,
        json: String,
    },
}

impl Accumulator {
    /// `origin` is the `provider/model` id stamped on reasoning blocks.
    pub fn new(origin: &str) -> Self {
        Self {
            origin: origin.to_owned(),
            id: String::new(),
            model: String::new(),
            blocks: Vec::new(),
            usage: Usage::default(),
            stop_reason: None,
        }
    }

    pub fn push(&mut self, event: &ChatEvent) {
        match event {
            ChatEvent::Start { id, model } => {
                self.id.clone_from(id);
                self.model.clone_from(model);
            }
            ChatEvent::BlockStart { index, block } => {
                let partial = match block {
                    BlockKind::Text => Partial::Text(String::new()),
                    BlockKind::Thinking => Partial::Thinking {
                        text: String::new(),
                        signature: None,
                    },
                    BlockKind::RedactedThinking { data } => Partial::Redacted(data.clone()),
                    BlockKind::ToolUse { id, name } => Partial::Tool {
                        id: id.clone(),
                        name: name.clone(),
                        json: String::new(),
                    },
                };
                self.slot(*index, partial);
            }
            ChatEvent::TextDelta { index, text } => {
                match self.block(*index, || Partial::Text(String::new())) {
                    Partial::Text(t) => t.push_str(text),
                    _ => tracing::warn!(index, "text delta for a non-text block"),
                }
            }
            ChatEvent::ReasoningDelta { index, text } => {
                match self.block(*index, || Partial::Thinking {
                    text: String::new(),
                    signature: None,
                }) {
                    Partial::Thinking { text: t, .. } => t.push_str(text),
                    _ => tracing::warn!(index, "reasoning delta for a non-thinking block"),
                }
            }
            ChatEvent::SignatureDelta { index, signature } => {
                if let Some(Partial::Thinking { signature: s, .. }) = self.blocks.get_mut(*index) {
                    s.get_or_insert_with(String::new).push_str(signature);
                }
            }
            ChatEvent::ToolInputDelta { index, json } => {
                if let Some(Partial::Tool { json: j, .. }) = self.blocks.get_mut(*index) {
                    j.push_str(json);
                }
            }
            ChatEvent::BlockStop { .. } => {}
            ChatEvent::Usage { usage } => self.usage.merge_max(usage),
            ChatEvent::Stop { stop_reason } => self.stop_reason = Some(stop_reason.clone()),
            // Reasoning blocks belong to the model that wrote them.
            ChatEvent::Routed { model, .. } => self.origin.clone_from(model),
        }
    }

    fn slot(&mut self, index: usize, partial: Partial) {
        if index < self.blocks.len() {
            self.blocks[index] = partial;
        } else {
            while self.blocks.len() < index {
                self.blocks.push(Partial::Text(String::new()));
            }
            self.blocks.push(partial);
        }
    }

    fn block(&mut self, index: usize, make: impl FnOnce() -> Partial) -> &mut Partial {
        if index >= self.blocks.len() {
            self.slot(index, make());
        }
        &mut self.blocks[index]
    }

    pub fn usage(&self) -> Usage {
        self.usage
    }

    pub fn finish(self) -> ChatResponse {
        let origin = Some(self.origin.clone());
        let content = self
            .blocks
            .into_iter()
            .filter_map(|p| match p {
                Partial::Text(text) if text.is_empty() => None,
                Partial::Text(text) => Some(ContentBlock::Text { text }),
                Partial::Thinking { text, signature } => Some(ContentBlock::Thinking {
                    text,
                    signature,
                    origin: origin.clone(),
                }),
                Partial::Redacted(data) => Some(ContentBlock::RedactedThinking {
                    data,
                    origin: origin.clone(),
                }),
                Partial::Tool { id, name, json } => Some(ContentBlock::ToolUse {
                    id,
                    name,
                    input: parse_tool_input(&json),
                }),
            })
            .collect::<Vec<_>>();
        let stop_reason = self.stop_reason.unwrap_or_else(|| {
            if content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { .. }))
            {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            }
        });
        ChatResponse {
            id: self.id,
            model: self.model,
            content,
            stop_reason,
            usage: self.usage,
        }
    }
}

/// Tool arguments arrive as streamed JSON text. An empty string means no
/// arguments. Invalid JSON is kept as a string so the agent runtime can
/// report it back to the model instead of running the tool on garbage.
pub fn parse_tool_input(json: &str) -> Value {
    if json.trim().is_empty() {
        return Value::Object(Map::new());
    }
    serde_json::from_str(json).unwrap_or_else(|_| Value::String(json.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accumulator_builds_blocks_in_order() {
        let mut acc = Accumulator::new("p/m");
        for e in [
            ChatEvent::Start {
                id: "r1".into(),
                model: "m".into(),
            },
            ChatEvent::BlockStart {
                index: 0,
                block: BlockKind::Thinking,
            },
            ChatEvent::ReasoningDelta {
                index: 0,
                text: "let me ".into(),
            },
            ChatEvent::ReasoningDelta {
                index: 0,
                text: "see".into(),
            },
            ChatEvent::SignatureDelta {
                index: 0,
                signature: "abc".into(),
            },
            ChatEvent::BlockStop { index: 0 },
            ChatEvent::BlockStart {
                index: 1,
                block: BlockKind::Text,
            },
            ChatEvent::TextDelta {
                index: 1,
                text: "Hi".into(),
            },
            ChatEvent::BlockStart {
                index: 2,
                block: BlockKind::ToolUse {
                    id: "t".into(),
                    name: "calc".into(),
                },
            },
            ChatEvent::ToolInputDelta {
                index: 2,
                json: "{\"a\":".into(),
            },
            ChatEvent::ToolInputDelta {
                index: 2,
                json: "2}".into(),
            },
            ChatEvent::Usage {
                usage: Usage {
                    input_tokens: 5,
                    ..Default::default()
                },
            },
            ChatEvent::Usage {
                usage: Usage {
                    output_tokens: 9,
                    ..Default::default()
                },
            },
        ] {
            acc.push(&e);
        }
        let r = acc.finish();
        assert_eq!(r.stop_reason, StopReason::ToolUse);
        assert_eq!(r.text(), "Hi");
        assert_eq!(
            r.content[0],
            ContentBlock::Thinking {
                text: "let me see".into(),
                signature: Some("abc".into()),
                origin: Some("p/m".into())
            }
        );
        assert_eq!(
            r.content[2],
            ContentBlock::ToolUse {
                id: "t".into(),
                name: "calc".into(),
                input: json!({"a": 2})
            }
        );
        assert_eq!((r.usage.input_tokens, r.usage.output_tokens), (5, 9));
    }

    #[test]
    fn effort_parses_case_insensitively() {
        assert_eq!("XHigh".parse::<Effort>().unwrap(), Effort::Xhigh);
        assert!("extreme".parse::<Effort>().is_err());
    }
}
