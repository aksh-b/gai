//! `router/<name>`: a virtual chat model that picks a real one per request.
//!
//! ```toml
//! [routers.auto]
//! classifier = "groq/llama-3.1-8b-instant"   # optional: rates difficulty
//! models = [
//!   { model = "ollama/gemma3:1b", level = "easy" },
//!   { model = "anthropic/claude-sonnet-5-5", level = "medium" },
//!   { model = "anthropic/claude-opus-5-5", level = "hard" },
//! ]
//! ```
//!
//! For each conversation the router works out what the request needs
//! (tools, vision, audio, JSON schema output, context size) and how hard it
//! is (the classifier's answer, or rules when there is no classifier), then
//! takes the cheapest model that can do it at that level. When the chosen
//! model fails, or its output is not the JSON the request asked for, it
//! escalates to a stronger model, raising reasoning effort if that model
//! reasons. Every decision is reported as a [`ChatEvent::Routed`] at the
//! start of the stream, so it lands in run traces.

use std::sync::Mutex;

use async_trait::async_trait;
use futures::StreamExt;
use gai_core::{ContentBlock, Pricing, Role};
use serde::{Deserialize, Serialize};

use crate::dialect::mock::response_events;
use crate::middleware::open;
use crate::{
    ChatEvent, ChatModel, ChatRequest, ChatStream, Effort, LlmError, ModelInfo, Reasoning,
    ResponseFormat, SharedChatModel, collect,
};

/// The provider name routers live under.
pub const PROVIDER: &str = "router";
pub const PREFIX: &str = "router/";

/// How hard a request is.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Easy,
    Medium,
    Hard,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Easy => "easy",
            Self::Medium => "medium",
            Self::Hard => "hard",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RouterConfig {
    /// A cheap model that rates each new conversation's difficulty. Without
    /// one, rules decide (length, keywords, tools, reasoning effort).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classifier: Option<String>,
    /// The models to choose from. A plain name gets a level from its place
    /// in the list (first easy, last hard).
    pub models: Vec<RouteTarget>,
    /// Try a stronger model when the chosen one fails or returns output
    /// that does not validate (default true).
    #[serde(default = "yes")]
    pub escalate: bool,
    /// Keep the model chosen for a conversation's first request for the
    /// rest of it (default true), which keeps prompt caches warm and
    /// reasoning blocks valid. When false, every request is routed afresh.
    #[serde(default = "yes")]
    pub sticky: bool,
    /// Skip models whose estimated cost for one call is above this (USD).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_per_call: Option<f64>,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(from = "TargetDef")]
pub struct RouteTarget {
    pub model: String,
    pub level: Option<Level>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
enum TargetDef {
    Name(String),
    Full { model: String, level: Option<Level> },
}

impl From<TargetDef> for RouteTarget {
    fn from(d: TargetDef) -> Self {
        match d {
            TargetDef::Name(model) => Self { model, level: None },
            TargetDef::Full { model, level } => Self { model, level },
        }
    }
}

struct Candidate {
    model: SharedChatModel,
    level: Level,
    /// Rough price per million tokens (input + output), for ordering.
    price: f64,
}

/// What a request needs from a model.
#[derive(Debug, Default)]
struct Needs {
    tools: bool,
    vision: bool,
    audio: bool,
    json_schema: bool,
    tokens: u64,
}

impl Needs {
    fn of(req: &ChatRequest) -> Self {
        let mut n = Self {
            tools: !req.tools.is_empty(),
            json_schema: matches!(req.response_format, Some(ResponseFormat::JsonSchema { .. })),
            ..Default::default()
        };
        let mut chars = req.system.as_deref().map_or(0, str::len)
            + req
                .tools
                .iter()
                .map(|t| t.name.len() + t.description.len() + t.input_schema.to_string().len())
                .sum::<usize>();
        fn walk(b: &ContentBlock, n: &mut Needs, chars: &mut usize) {
            match b {
                ContentBlock::Text { text } => *chars += text.len(),
                ContentBlock::Image { .. } => {
                    n.vision = true;
                    *chars += 6_000;
                }
                ContentBlock::Audio { .. } => {
                    n.audio = true;
                    *chars += 8_000;
                }
                ContentBlock::ToolUse { input, .. } => *chars += input.to_string().len(),
                ContentBlock::ToolResult { content, .. } => {
                    for c in content {
                        walk(c, n, chars)
                    }
                }
                ContentBlock::Thinking { text, .. } => *chars += text.len(),
                ContentBlock::RedactedThinking { data, .. } => *chars += data.len() / 2,
            }
        }
        for m in &req.messages {
            for b in &m.content {
                walk(b, &mut n, &mut chars);
            }
        }
        n.tokens = (chars as u64 * 2).div_ceil(7);
        n
    }

    fn describe(&self) -> String {
        let mut parts = Vec::new();
        if self.tools {
            parts.push("tools");
        }
        if self.vision {
            parts.push("vision");
        }
        if self.audio {
            parts.push("audio");
        }
        if self.json_schema {
            parts.push("JSON schema output");
        }
        if parts.is_empty() {
            format!("~{} tokens", self.tokens)
        } else {
            format!("{}, ~{} tokens", parts.join(", "), self.tokens)
        }
    }
}

/// The model a router picked, and why.
#[derive(Clone, Debug)]
struct Choice {
    index: usize,
    reason: String,
}

pub struct RouterModel {
    info: ModelInfo,
    cfg: RouterConfig,
    /// Cheapest first.
    candidates: Vec<Candidate>,
    classifier: Option<SharedChatModel>,
    chosen: Mutex<Option<Choice>>,
}

impl RouterModel {
    pub fn new(
        info: ModelInfo,
        cfg: RouterConfig,
        models: Vec<(RouteTarget, SharedChatModel)>,
        classifier: Option<SharedChatModel>,
    ) -> Self {
        let n = cfg.models.len().max(1);
        let mut candidates: Vec<Candidate> = models
            .into_iter()
            .map(|(t, model)| {
                let pos = cfg
                    .models
                    .iter()
                    .position(|x| x.model == t.model)
                    .unwrap_or(0);
                let level = t.level.unwrap_or(match (pos * 3) / n {
                    0 => Level::Easy,
                    1 => Level::Medium,
                    _ => Level::Hard,
                });
                let price = model
                    .info()
                    .pricing
                    .map(|p| p.input + p.output)
                    .unwrap_or(0.0);
                Candidate {
                    model,
                    level,
                    price,
                }
            })
            .collect();
        // Cheapest first; for equal prices, the weaker model first.
        candidates.sort_by(|a, b| a.price.total_cmp(&b.price).then(a.level.cmp(&b.level)));
        Self {
            info,
            cfg,
            candidates,
            classifier,
            chosen: Mutex::new(None),
        }
    }

    fn capable(&self, c: &Candidate, n: &Needs, req: &ChatRequest) -> bool {
        let x = &c.model.info().capabilities;
        if (n.tools && !x.tools)
            || (n.vision && !x.vision)
            || (n.audio && !x.audio)
            || (n.json_schema && !x.json_schema && c.model.info().provider != "mock")
            || x.context_window.is_some_and(|w| (w as u64) < n.tokens)
        {
            return false;
        }
        match (self.cfg.max_cost_per_call, c.model.info().pricing) {
            (Some(max), Some(p)) => call_cost(&p, n.tokens, req.max_tokens) <= max,
            _ => true,
        }
    }

    /// How hard the request is, and how that was decided. Also returns the
    /// classifier's cost, if it was asked.
    async fn rate(&self, req: &ChatRequest, n: &Needs) -> (Level, String, f64) {
        let rule = rules(req, n);
        let Some(classifier) = &self.classifier else {
            return (rule.0, rule.1, 0.0);
        };
        let text = request_text(req);
        let mut ask = ChatRequest::user(format!(
            "Request:\n{}\n\nTools available: {}",
            clip(&text, 3_000),
            req.tools.len()
        ))
        .with_system(CLASSIFIER_PROMPT);
        ask.max_tokens = Some(60);
        ask.temperature = Some(0.0);
        ask.cache = false;
        match classifier.complete(ask).await {
            Ok(resp) => {
                let cost = classifier.info().cost(&resp.usage).unwrap_or(0.0);
                match parse_level(&resp.text()) {
                    Some(l) => (
                        l,
                        format!("{} rated it {}", classifier.info().id, l.as_str()),
                        cost,
                    ),
                    None => (
                        rule.0,
                        format!("{}; the classifier's answer was unclear", rule.1),
                        cost,
                    ),
                }
            }
            Err(e) => (
                rule.0,
                format!("{}; the classifier failed ({e})", rule.1),
                0.0,
            ),
        }
    }

    /// The cheapest capable model at `level` or above; failing that, the
    /// strongest capable one.
    fn pick(&self, level: Level, n: &Needs, req: &ChatRequest) -> Option<usize> {
        let capable: Vec<usize> = (0..self.candidates.len())
            .filter(|&i| self.capable(&self.candidates[i], n, req))
            .collect();
        capable
            .iter()
            .copied()
            .find(|&i| self.candidates[i].level >= level)
            .or_else(|| {
                capable
                    .iter()
                    .copied()
                    .max_by_key(|&i| self.candidates[i].level)
            })
    }

    /// Where to go after `from` failed: the cheapest capable model of a
    /// higher level, else another capable model of the same level.
    fn escalation(
        &self,
        from: usize,
        tried: &[usize],
        n: &Needs,
        req: &ChatRequest,
    ) -> Option<usize> {
        let level = self.candidates[from].level;
        let open: Vec<usize> = (0..self.candidates.len())
            .filter(|i| !tried.contains(i) && self.capable(&self.candidates[*i], n, req))
            .collect();
        open.iter()
            .copied()
            .find(|&i| self.candidates[i].level > level)
            .or_else(|| {
                open.iter()
                    .copied()
                    .find(|&i| self.candidates[i].level == level)
            })
    }

    fn routed(&self, index: usize, reason: String, extra_cost: f64) -> ChatEvent {
        let info = self.candidates[index].model.info();
        ChatEvent::Routed {
            model: info.id.clone(),
            reason,
            pricing: info.pricing,
            extra_cost,
        }
    }
}

const CLASSIFIER_PROMPT: &str = "You rate how hard a request is for an AI assistant. Answer with JSON only, like {\"difficulty\": \"medium\", \"task\": \"few words\"}. easy: greetings, small talk, simple facts, short rewrites or translations. medium: ordinary writing, explanations, everyday code, a few tool calls. hard: deep reasoning, long analysis, math or proofs, tricky debugging, architecture, many-step research.";

const HARD_HINTS: [&str; 12] = [
    "prove",
    "theorem",
    "architecture",
    "design a",
    "refactor",
    "debug",
    "optimi",
    "analyz",
    "analys",
    "step by step",
    "trade-off",
    "algorithm",
];

/// Difficulty by rules, for when there is no classifier (or it failed).
fn rules(req: &ChatRequest, n: &Needs) -> (Level, String) {
    let text = request_text(req);
    let lower = text.to_lowercase();
    let effort = req.reasoning.as_ref().map(|r| r.effort);
    if effort.is_some_and(|e| e >= Effort::High) {
        return (Level::Hard, "rules: high reasoning effort requested".into());
    }
    if text.len() > 6_000 {
        return (Level::Hard, "rules: a long request".into());
    }
    if let Some(h) = HARD_HINTS.iter().find(|h| lower.contains(*h)) {
        return (Level::Hard, format!("rules: mentions {h:?}"));
    }
    if text.len() < 280 && !n.tools && !n.vision && !text.contains("```") {
        return (Level::Easy, "rules: a short request without tools".into());
    }
    (Level::Medium, "rules: an ordinary request".into())
}

/// The first user message's text: what the conversation is about.
fn request_text(req: &ChatRequest) -> String {
    req.messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User && m.content.iter().any(|b| b.as_text().is_some()))
        .map(|m| m.text())
        .unwrap_or_default()
}

fn parse_level(answer: &str) -> Option<Level> {
    let lower = answer.to_lowercase();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(strip_fences(&lower))
        && let Some(d) = v["difficulty"].as_str()
    {
        return serde_json::from_value(serde_json::Value::String(d.trim().to_owned())).ok();
    }
    ["hard", "medium", "easy"]
        .iter()
        .find(|w| lower.contains(*w))
        .and_then(|w| serde_json::from_value(serde_json::Value::String((*w).to_owned())).ok())
}

fn strip_fences(s: &str) -> &str {
    let t = s.trim();
    t.strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .and_then(|r| r.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(t)
}

fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// Estimated USD for one call: the prompt plus a typical reply.
fn call_cost(p: &Pricing, input_tokens: u64, max_out: Option<u32>) -> f64 {
    let out = max_out.unwrap_or(2_000).min(4_000) as f64;
    p.input * input_tokens as f64 / 1e6 + p.output * out / 1e6
}

/// Whether a reply is the JSON a schema-constrained request asked for.
fn valid_json(text: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(strip_fences(text)).is_ok()
}

#[async_trait]
impl ChatModel for RouterModel {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    async fn stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        let n = Needs::of(&req);
        let sticky = if self.cfg.sticky {
            self.chosen
                .lock()
                .expect("router lock")
                .clone()
                .filter(|c| self.capable(&self.candidates[c.index], &n, &req))
        } else {
            None
        };
        let (mut index, mut reason, extra_cost) = match sticky {
            Some(c) => (
                c.index,
                format!("kept for this conversation ({})", c.reason),
                0.0,
            ),
            None => {
                let (level, why, cost) = self.rate(&req, &n).await;
                let index = self.pick(level, &n, &req).ok_or_else(|| {
                    LlmError::InvalidRequest(format!(
                        "no model of {} can handle this request (needs {})",
                        self.info.id,
                        n.describe()
                    ))
                })?;
                let picked = &self.candidates[index];
                let reason = if picked.level == level {
                    format!(
                        "{why}; cheapest {} model that fits ({})",
                        level.as_str(),
                        n.describe()
                    )
                } else if picked.level > level {
                    format!(
                        "{why}; cheapest model at {} or above that fits ({}): a {} one",
                        level.as_str(),
                        n.describe(),
                        picked.level.as_str()
                    )
                } else {
                    format!(
                        "{why}; no {} model fits ({}), so the strongest that does",
                        level.as_str(),
                        n.describe()
                    )
                };
                (index, reason, cost)
            }
        };

        let mut tried = Vec::new();
        let mut req = req;
        loop {
            tried.push(index);
            let candidate = &self.candidates[index];
            let next = |tried: &[usize]| {
                if self.cfg.escalate {
                    self.escalation(index, tried, &n, &req)
                } else {
                    None
                }
            };
            let result = open(candidate.model.as_ref(), req.clone()).await;
            let failure = match result {
                Err(e) if !matches!(e, LlmError::InvalidRequest(_) | LlmError::Cancelled) => {
                    format!("{} failed: {e}", candidate.model.info().id)
                }
                Err(e) => return Err(e),
                Ok(stream) if n.json_schema && next(&tried).is_some() => {
                    // Read the whole reply to check it before passing it on.
                    let resp = collect(stream, &candidate.model.info().id).await?;
                    if valid_json(&resp.text()) {
                        self.remember(index, &reason);
                        let head = self.routed(index, reason, extra_cost);
                        let events = std::iter::once(head).chain(response_events(&resp)).map(Ok);
                        return Ok(Box::pin(futures::stream::iter(events)));
                    }
                    format!("{} did not return valid JSON", candidate.model.info().id)
                }
                Ok(stream) => {
                    self.remember(index, &reason);
                    let head = self.routed(index, reason, extra_cost);
                    return Ok(Box::pin(
                        futures::stream::once(async move { Ok(head) }).chain(stream),
                    ));
                }
            };
            match next(&tried) {
                Some(to) => {
                    tracing::warn!(router = %self.info.id, %failure, to = %self.candidates[to].model.info().id, "escalating");
                    // A stronger model gets to think harder too.
                    if self.candidates[to].model.info().capabilities.reasoning
                        && req
                            .reasoning
                            .as_ref()
                            .is_none_or(|r| r.effort < Effort::High)
                    {
                        req.reasoning = Some(Reasoning {
                            effort: Effort::High,
                            ..req.reasoning.clone().unwrap_or_default()
                        });
                    }
                    reason = format!(
                        "escalated: {failure}; next model up is {}",
                        self.candidates[to].model.info().id
                    );
                    index = to;
                }
                None => {
                    return Err(LlmError::Provider {
                        status: 502,
                        message: format!(
                            "{}: {failure}, and there is no model to escalate to",
                            self.info.id
                        ),
                    });
                }
            }
        }
    }
}

impl RouterModel {
    fn remember(&self, index: usize, reason: &str) {
        if self.cfg.sticky {
            *self.chosen.lock().expect("router lock") = Some(Choice {
                index,
                reason: reason.to_owned(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::dialect::mock::{MockModel, text_response};
    use crate::{Capabilities, LlmConfig, ModelConfig, ModelRegistry, ProviderConfig};
    use gai_core::ToolSpec;

    fn model(
        id: &str,
        price: Option<(f64, f64)>,
        caps: Capabilities,
        script: Vec<Result<crate::ChatResponse, LlmError>>,
    ) -> (Arc<MockModel>, SharedChatModel) {
        let (p, m) = id.split_once('/').unwrap();
        let mut info = ModelInfo::new(p, m);
        info.capabilities = caps;
        info.pricing = price.map(|(i, o)| Pricing::new(i, o));
        let mock = Arc::new(MockModel::scripted_results(script).with_info(info));
        (mock.clone(), mock as SharedChatModel)
    }

    fn caps(tools: bool, vision: bool, json: bool) -> Capabilities {
        Capabilities {
            tools,
            vision,
            json_schema: json,
            context_window: Some(100_000),
            ..Default::default()
        }
    }

    fn target(m: &str, level: Level) -> RouteTarget {
        RouteTarget {
            model: m.into(),
            level: Some(level),
        }
    }

    fn cfg(models: &[(&str, Level)]) -> RouterConfig {
        RouterConfig {
            classifier: None,
            models: models.iter().map(|(m, l)| target(m, *l)).collect(),
            escalate: true,
            sticky: true,
            max_cost_per_call: None,
        }
    }

    async fn first(stream: ChatStream) -> (String, String) {
        let events: Vec<ChatEvent> = stream.map(|e| e.unwrap()).collect().await;
        let ChatEvent::Routed { model, reason, .. } = &events[0] else {
            panic!("no routing event: {events:?}");
        };
        let text: String = events
            .iter()
            .filter_map(|e| match e {
                ChatEvent::TextDelta { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        (model.clone(), format!("{reason} => {text}"))
    }

    #[tokio::test]
    async fn picks_the_cheapest_capable_model_for_the_difficulty() {
        let (_, small) = model(
            "p/small",
            Some((0.1, 0.5)),
            caps(false, false, false),
            vec![Ok(text_response("small")), Ok(text_response("small again"))],
        );
        let (_, mid) = model(
            "p/mid",
            Some((2.0, 10.0)),
            caps(true, true, true),
            vec![Ok(text_response("mid"))],
        );
        let (_, big) = model(
            "p/big",
            Some((5.0, 25.0)),
            caps(true, true, true),
            vec![Ok(text_response("big"))],
        );
        let c = cfg(&[
            ("p/small", Level::Easy),
            ("p/mid", Level::Medium),
            ("p/big", Level::Hard),
        ]);
        let pairs = |c: &RouterConfig, ms: Vec<SharedChatModel>| {
            c.models.iter().cloned().zip(ms).collect::<Vec<_>>()
        };
        let r = RouterModel::new(
            ModelInfo::new("router", "auto"),
            RouterConfig {
                sticky: false,
                ..c.clone()
            },
            pairs(&c, vec![small, mid, big]),
            None,
        );

        // Short and tool-free: easy, so the cheapest.
        let (m, why) = first(r.stream(ChatRequest::user("hi there")).await.unwrap()).await;
        assert_eq!(m, "p/small", "{why}");
        // Needs tools: the small model cannot, so the cheapest that can.
        let tool = ToolSpec {
            name: "t".into(),
            description: "d".into(),
            input_schema: serde_json::json!({"type": "object"}),
            strict: false,
        };
        let req = ChatRequest::user("hi").with_tools(vec![tool]);
        let (m, _) = first(r.stream(req).await.unwrap()).await;
        assert_eq!(m, "p/mid");
        // Hard by rules.
        let (m, why) = first(
            r.stream(ChatRequest::user("Prove that sqrt 2 is irrational"))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(m, "p/big", "{why}");
        assert!(why.contains("prove"), "{why}");
        // Nothing hard left fits a tool-free easy question? Small again.
        let (m, _) = first(r.stream(ChatRequest::user("thanks")).await.unwrap()).await;
        assert_eq!(m, "p/small");
    }

    #[tokio::test]
    async fn escalates_on_failure_and_invalid_json_and_sticks() {
        let (_, small) = model(
            "p/small",
            None,
            caps(true, false, true),
            vec![
                Err(LlmError::Provider {
                    status: 503,
                    message: "down".into(),
                }),
                Ok(text_response("not json")),
            ],
        );
        let (big_mock, big) = model(
            "p/big",
            Some((5.0, 25.0)),
            Capabilities {
                reasoning: true,
                ..caps(true, false, true)
            },
            vec![
                Ok(text_response("from big")),
                Ok(text_response("{\"ok\": true}")),
                Ok(text_response("still big")),
            ],
        );
        let c = cfg(&[("p/small", Level::Easy), ("p/big", Level::Hard)]);
        let r = RouterModel::new(
            ModelInfo::new("router", "auto"),
            c.clone(),
            c.models.iter().cloned().zip(vec![small, big]).collect(),
            None,
        );

        let (m, why) = first(r.stream(ChatRequest::user("hello")).await.unwrap()).await;
        assert_eq!(m, "p/big");
        assert!(
            why.contains("escalated") && why.contains("from big"),
            "{why}"
        );
        // Escalation raised reasoning effort for the stronger model.
        let sent = big_mock.requests().lock().unwrap()[0].clone();
        assert_eq!(sent.reasoning.unwrap().effort, Effort::High);

        // Sticky: the conversation stays on big.
        let mut req = ChatRequest::user("hello");
        req.response_format = Some(ResponseFormat::JsonSchema {
            name: "o".into(),
            schema: serde_json::json!({}),
            strict: true,
        });
        let (m, why) = first(r.stream(req).await.unwrap()).await;
        assert_eq!(m, "p/big", "{why}");
        assert!(why.contains("kept"), "{why}");

        // Not sticky: small answers, but with invalid JSON, so big takes over.
        let r2 = RouterModel::new(
            ModelInfo::new("router", "auto"),
            RouterConfig {
                sticky: false,
                ..c.clone()
            },
            vec![
                (
                    target("p/small", Level::Easy),
                    model(
                        "p/small",
                        None,
                        caps(true, false, true),
                        vec![Ok(text_response("not json"))],
                    )
                    .1,
                ),
                (
                    target("p/big", Level::Hard),
                    model(
                        "p/big",
                        Some((5.0, 25.0)),
                        caps(true, false, true),
                        vec![Ok(text_response("{\"ok\": 1}"))],
                    )
                    .1,
                ),
            ],
            None,
        );
        let mut req = ChatRequest::user("hello");
        req.response_format = Some(ResponseFormat::JsonSchema {
            name: "o".into(),
            schema: serde_json::json!({}),
            strict: true,
        });
        let (m, why) = first(r2.stream(req).await.unwrap()).await;
        assert_eq!(m, "p/big");
        assert!(why.contains("valid JSON"), "{why}");
    }

    #[tokio::test]
    async fn classifier_decides_and_impossible_requests_fail() {
        let (_, cls) = model(
            "p/cls",
            Some((0.05, 0.1)),
            caps(false, false, false),
            vec![Ok(text_response(
                "{\"difficulty\": \"hard\", \"task\": \"x\"}",
            ))],
        );
        let (_, a) = model("p/a", None, caps(false, false, false), vec![]);
        let (_, b) = model(
            "p/b",
            Some((1.0, 1.0)),
            caps(false, false, false),
            vec![Ok(text_response("b"))],
        );
        let c = RouterConfig {
            classifier: Some("p/cls".into()),
            ..cfg(&[("p/a", Level::Easy), ("p/b", Level::Hard)])
        };
        let r = RouterModel::new(
            ModelInfo::new("router", "auto"),
            c.clone(),
            c.models.iter().cloned().zip(vec![a, b]).collect(),
            Some(cls),
        );
        let (m, why) = first(r.stream(ChatRequest::user("hi")).await.unwrap()).await;
        assert_eq!(m, "p/b");
        assert!(why.contains("p/cls rated it hard"), "{why}");

        let mut req = ChatRequest::user("look");
        req.messages[0].content.push(ContentBlock::Image {
            source: gai_core::MediaSource::Url {
                url: "http://x".into(),
            },
        });
        let r = RouterModel::new(
            ModelInfo::new("router", "auto"),
            cfg(&[("p/a", Level::Easy)]),
            vec![(
                target("p/a", Level::Easy),
                model("p/a", None, caps(false, false, false), vec![]).1,
            )],
            None,
        );
        assert!(matches!(
            r.stream(req).await,
            Err(LlmError::InvalidRequest(_))
        ));
        assert_eq!(
            parse_level("```json\n{\"difficulty\": \"Easy\"}\n```"),
            Some(Level::Easy)
        );
        assert_eq!(parse_level("I'd say medium."), Some(Level::Medium));
    }

    #[test]
    fn registry_builds_routers_from_config() {
        let mut cfg = LlmConfig::default();
        let mut p = ProviderConfig::new("mock", "");
        p.models.push(ModelConfig::chat(
            "echo",
            Capabilities {
                vision: true,
                ..Default::default()
            },
            None,
        ));
        cfg.providers.insert("mock".into(), p);
        cfg.routers.insert(
            "auto".into(),
            serde_json::from_value(serde_json::json!({"models": ["mock/echo", {"model": "mock/echo", "level": "hard"}]})).unwrap(),
        );
        let r = ModelRegistry::from_config(cfg).unwrap();
        let m = r.chat("router/auto").unwrap();
        assert_eq!(m.info().id, "router/auto");
        assert!(m.info().capabilities.vision && m.info().capabilities.tools);
        assert!(r.catalog().iter().any(|e| e.info.id == "router/auto"));
        assert!(r.resolve("router/nope").is_err());
    }
}
