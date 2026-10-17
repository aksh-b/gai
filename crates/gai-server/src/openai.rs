//! An OpenAI-compatible surface at `/openai/v1`, so any OpenAI client or
//! SDK can use gai:
//!
//! - `model: "<provider>/<model>"` (or an alias) proxies to that model:
//!   gai acts as a gateway to every configured provider, tool calls included.
//! - `model: "agent:<name>"` runs a gai agent with its tools and returns its
//!   final answer.
//!
//! Point a client at `http://host:8484/openai/v1` with a gai API key.

use std::collections::HashMap;
use std::convert::Infallible;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use futures::StreamExt;
use gai::agent::{RunEvent, RunRequest};
use gai::core::{ContentBlock, MediaSource, Message, Role, ToolChoice, ToolSpec, Usage};
use gai::llm::{
    Accumulator, BlockKind, ChatEvent, ChatRequest, Effort, Reasoning, ResponseFormat, StopReason,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::AppState;
use crate::auth::{Principal, charge};
use crate::error::{ApiError, ApiResult};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/openai/v1/chat/completions", post(chat_completions))
        .route("/openai/v1/models", get(list_models))
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<OaMessage>,
    #[serde(default)]
    pub stream: bool,
    pub stream_options: Option<StreamOptions>,
    pub max_tokens: Option<u32>,
    pub max_completion_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub stop: Option<Stop>,
    #[serde(default)]
    pub tools: Vec<OaTool>,
    pub tool_choice: Option<Value>,
    pub reasoning_effort: Option<String>,
    pub response_format: Option<Value>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct StreamOptions {
    #[serde(default)]
    pub include_usage: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum Stop {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct OaMessage {
    pub role: String,
    #[serde(default)]
    pub content: Option<OaContent>,
    #[serde(default)]
    pub tool_calls: Vec<OaToolCall>,
    pub tool_call_id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum OaContent {
    Text(String),
    Parts(Vec<Value>),
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct OaToolCall {
    pub id: String,
    pub function: OaFunctionCall,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct OaFunctionCall {
    pub name: String,
    #[serde(default)]
    pub arguments: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct OaTool {
    pub function: OaFunction,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct OaFunction {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub parameters: Option<Value>,
    #[serde(default)]
    pub strict: bool,
}

fn content_blocks(c: &Option<OaContent>) -> Vec<ContentBlock> {
    match c {
        None => vec![],
        Some(OaContent::Text(t)) => vec![ContentBlock::text(t.clone())],
        Some(OaContent::Parts(parts)) => parts
            .iter()
            .filter_map(|p| match p["type"].as_str() {
                Some("text") => p["text"].as_str().map(ContentBlock::text),
                Some("image_url") => {
                    let url = p["image_url"]["url"].as_str()?;
                    let source = match url
                        .strip_prefix("data:")
                        .and_then(|r| r.split_once(";base64,"))
                    {
                        Some((media_type, data)) => MediaSource::Base64 {
                            media_type: media_type.into(),
                            data: data.into(),
                        },
                        None => MediaSource::Url { url: url.into() },
                    };
                    Some(ContentBlock::Image { source })
                }
                _ => None,
            })
            .collect(),
    }
}

/// OpenAI messages to a system prompt and gai messages.
fn convert(messages: &[OaMessage]) -> (Option<String>, Vec<Message>) {
    let mut system = Vec::new();
    let mut out: Vec<Message> = Vec::new();
    let mut push = |role: Role, blocks: Vec<ContentBlock>| {
        if blocks.is_empty() {
            return;
        }
        match out.last_mut() {
            Some(last) if last.role == role => last.content.extend(blocks),
            _ => out.push(Message {
                role,
                content: blocks,
            }),
        }
    };
    for m in messages {
        match m.role.as_str() {
            "system" | "developer" => {
                let text: String = content_blocks(&m.content)
                    .iter()
                    .filter_map(|b| b.as_text())
                    .collect();
                system.push(text);
            }
            "assistant" => {
                let mut blocks = content_blocks(&m.content);
                for c in &m.tool_calls {
                    blocks.push(ContentBlock::ToolUse {
                        id: c.id.clone(),
                        name: c.function.name.clone(),
                        input: gai::llm::parse_tool_input(&c.function.arguments),
                    });
                }
                push(Role::Assistant, blocks);
            }
            "tool" => {
                let text: String = content_blocks(&m.content)
                    .iter()
                    .filter_map(|b| b.as_text())
                    .collect();
                push(
                    Role::User,
                    vec![ContentBlock::tool_result(
                        m.tool_call_id.clone().unwrap_or_default(),
                        text,
                        false,
                    )],
                );
            }
            _ => push(Role::User, content_blocks(&m.content)),
        }
    }
    let system = system.join("\n\n");
    ((!system.is_empty()).then_some(system), out)
}

fn finish_reason(r: &StopReason) -> &'static str {
    match r {
        StopReason::ToolUse => "tool_calls",
        StopReason::MaxTokens => "length",
        StopReason::Refusal => "content_filter",
        _ => "stop",
    }
}

fn usage_json(u: &Usage) -> Value {
    json!({
        "prompt_tokens": u.total_input(),
        "completion_tokens": u.output_tokens,
        "total_tokens": u.total(),
        "prompt_tokens_details": {"cached_tokens": u.cache_read_tokens},
        "completion_tokens_details": {"reasoning_tokens": u.reasoning_tokens},
    })
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

async fn chat_completions(
    State(s): State<AppState>,
    Extension(p): Extension<Principal>,
    Json(req): Json<ChatCompletionRequest>,
) -> ApiResult<Response> {
    p.check_budget()?;
    match req.model.strip_prefix("agent:") {
        Some(agent) => agent_completion(s, p, agent.to_owned(), req).await,
        None => model_completion(s, p, req).await,
    }
}

async fn model_completion(
    s: AppState,
    p: Principal,
    req: ChatCompletionRequest,
) -> ApiResult<Response> {
    let model = s.gai.models().chat(&req.model)?;
    let (system, messages) = convert(&req.messages);
    let mut cr = ChatRequest::new(messages);
    cr.system = system;
    cr.max_tokens = req.max_completion_tokens.or(req.max_tokens);
    cr.temperature = req.temperature;
    cr.top_p = req.top_p;
    cr.stop = match req.stop {
        Some(Stop::One(s)) => vec![s],
        Some(Stop::Many(v)) => v,
        None => vec![],
    };
    cr.tools = req
        .tools
        .iter()
        .map(|t| ToolSpec {
            name: t.function.name.clone(),
            description: t.function.description.clone(),
            input_schema: t
                .function
                .parameters
                .clone()
                .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
            strict: t.function.strict,
        })
        .collect();
    cr.tool_choice = match &req.tool_choice {
        Some(Value::String(c)) if c == "none" => ToolChoice::None,
        Some(Value::String(c)) if c == "required" => ToolChoice::Required,
        Some(v) if v["function"]["name"].is_string() => ToolChoice::Tool {
            name: v["function"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        },
        _ => ToolChoice::Auto,
    };
    if let Some(e) = &req.reasoning_effort {
        let effort = match e.as_str() {
            "minimal" | "none" => Effort::Off,
            other => other.parse().unwrap_or_default(),
        };
        cr.reasoning = Some(Reasoning {
            effort,
            ..Default::default()
        });
    }
    cr.response_format = match req
        .response_format
        .as_ref()
        .and_then(|f| f["type"].as_str())
    {
        Some("json_object") => Some(ResponseFormat::Json),
        Some("json_schema") => {
            let f = &req.response_format.as_ref().expect("checked")["json_schema"];
            Some(ResponseFormat::JsonSchema {
                name: f["name"].as_str().unwrap_or("output").to_owned(),
                schema: f["schema"].clone(),
                strict: f["strict"].as_bool().unwrap_or(false),
            })
        }
        _ => None,
    };

    let id = format!(
        "chatcmpl-{}",
        gai::core::new_id("c").trim_start_matches("c_")
    );
    let model_name = req.model.clone();
    let store = s.rt.store().clone();
    let key = p.key_id().map(str::to_owned);
    let info = model.info().clone();
    let mut stream = model.stream(cr).await?;

    if !req.stream {
        let mut acc = Accumulator::new(&info.id);
        while let Some(e) = stream.next().await {
            acc.push(&e?);
        }
        let r = acc.finish();
        let cost = info.cost(&r.usage);
        let _ = store.record_usage(None, &info.id, &r.usage, cost).await;
        charge(&store, key.as_deref(), cost.unwrap_or(0.0)).await;
        let text: String = r.content.iter().filter_map(ContentBlock::as_text).collect();
        let reasoning: String = r
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Thinking { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let calls: Vec<Value> = r
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, name, input } => Some(json!({
                    "id": id, "type": "function",
                    "function": {"name": name, "arguments": match input { Value::String(s) => s.clone(), v => v.to_string() }}
                })),
                _ => None,
            })
            .collect();
        let mut message = json!({"role": "assistant", "content": if text.is_empty() && !calls.is_empty() { Value::Null } else { json!(text) }});
        if !calls.is_empty() {
            message["tool_calls"] = json!(calls);
        }
        if !reasoning.is_empty() {
            message["reasoning_content"] = json!(reasoning);
        }
        return Ok(Json(json!({
            "id": id, "object": "chat.completion", "created": now(), "model": model_name,
            "choices": [{"index": 0, "message": message, "finish_reason": finish_reason(&r.stop_reason)}],
            "usage": usage_json(&r.usage),
        }))
        .into_response());
    }

    let include_usage = req.stream_options.as_ref().is_some_and(|o| o.include_usage);
    let (tx, rx) = mpsc::channel::<Value>(256);
    tokio::spawn(async move {
        let chunk = |delta: Value, finish: Option<&str>| {
            json!({"id": id, "object": "chat.completion.chunk", "created": now(), "model": model_name,
                   "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]})
        };
        let _ = tx
            .send(chunk(json!({"role": "assistant", "content": ""}), None))
            .await;
        let mut usage = Usage::default();
        let mut tool_index: HashMap<usize, usize> = HashMap::new();
        let mut stop = StopReason::EndTurn;
        while let Some(e) = stream.next().await {
            let delta = match e {
                Err(err) => {
                    let _ = tx
                        .send(json!({"error": {"message": err.to_string()}}))
                        .await;
                    break;
                }
                Ok(ChatEvent::TextDelta { text, .. }) => json!({"content": text}),
                Ok(ChatEvent::ReasoningDelta { text, .. }) => json!({"reasoning_content": text}),
                Ok(ChatEvent::BlockStart {
                    index,
                    block: BlockKind::ToolUse { id, name },
                }) => {
                    let n = tool_index.len();
                    tool_index.insert(index, n);
                    json!({"tool_calls": [{"index": n, "id": id, "type": "function", "function": {"name": name, "arguments": ""}}]})
                }
                Ok(ChatEvent::ToolInputDelta { index, json: args }) => {
                    let n = tool_index.get(&index).copied().unwrap_or(0);
                    json!({"tool_calls": [{"index": n, "function": {"arguments": args}}]})
                }
                Ok(ChatEvent::Usage { usage: u }) => {
                    usage.merge_max(&u);
                    continue;
                }
                Ok(ChatEvent::Stop { stop_reason }) => {
                    stop = stop_reason;
                    continue;
                }
                Ok(_) => continue,
            };
            if tx.send(chunk(delta, None)).await.is_err() {
                break;
            }
        }
        let _ = tx.send(chunk(json!({}), Some(finish_reason(&stop)))).await;
        if include_usage {
            let _ = tx
                .send(json!({"id": id, "object": "chat.completion.chunk", "created": now(), "model": model_name,
                             "choices": [], "usage": usage_json(&usage)}))
                .await;
        }
        let cost = info.cost(&usage);
        let _ = store.record_usage(None, &info.id, &usage, cost).await;
        charge(&store, key.as_deref(), cost.unwrap_or(0.0)).await;
    });
    Ok(sse_chunks(rx).into_response())
}

/// Run an agent. Earlier messages become the session's history; the last
/// user message is the input.
async fn agent_completion(
    s: AppState,
    p: Principal,
    agent: String,
    req: ChatCompletionRequest,
) -> ApiResult<Response> {
    let (spec, version) = match agent.as_str() {
        "" | "default" | "assistant" => (gai::default_agent(), None),
        r => s.gai.agent(r).await?,
    };
    let (system, mut messages) = convert(&req.messages);
    let last = messages
        .iter()
        .rposition(|m| m.role == Role::User)
        .ok_or_else(|| ApiError::bad_request("the conversation has no user message"))?;
    let input = messages.remove(last).content;
    messages.truncate(last);
    let store = s.rt.store();
    let session = store
        .create_session(gai::store::NewSession {
            agent: Some(spec.name.clone()),
            title: Some("openai-compatible request".into()),
            ..Default::default()
        })
        .await?;
    if !messages.is_empty() {
        store.append_messages(&session.id, None, &messages).await?;
    }
    let mut run = RunRequest::with_input(spec, input);
    run.agent_version = version;
    run.session_id = Some(session.id);
    if let Some(sys) = system {
        run.vars.insert("client_system".into(), json!(sys));
    }
    if let Some(id) = p.key_id() {
        run.metadata.insert("key_id".into(), json!(id));
    }
    let mut handle = s.rt.start(run).await?;
    let id = format!("chatcmpl-{}", handle.run_id);
    let model_name = req.model.clone();
    // The runtime's finish hook charges the key.
    if !req.stream {
        let o = handle.wait().await;
        if o.status != gai::store::RunStatus::Completed {
            return Err(ApiError::new(
                axum::http::StatusCode::BAD_GATEWAY,
                format!(
                    "agent run {} {}: {}",
                    o.run_id,
                    o.status.as_str(),
                    o.error.unwrap_or_default()
                ),
            ));
        }
        return Ok(Json(json!({
            "id": id, "object": "chat.completion", "created": now(), "model": model_name,
            "choices": [{"index": 0, "message": {"role": "assistant", "content": o.text}, "finish_reason": "stop"}],
            "usage": usage_json(&o.usage),
        }))
        .into_response());
    }

    let include_usage = req.stream_options.as_ref().is_some_and(|o| o.include_usage);
    let (tx, rx) = mpsc::channel::<Value>(256);
    tokio::spawn(async move {
        let chunk = |delta: Value, finish: Option<&str>| {
            json!({"id": id, "object": "chat.completion.chunk", "created": now(), "model": model_name,
                   "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]})
        };
        let _ = tx
            .send(chunk(json!({"role": "assistant", "content": ""}), None))
            .await;
        let mut gone = false;
        while let Some(env) = handle.next_event().await {
            if gone {
                continue; // keep draining so the run finishes normally
            }
            let delta = match env.event {
                RunEvent::TextDelta { text } => json!({"content": text}),
                RunEvent::ReasoningDelta { text } => json!({"reasoning_content": text}),
                _ => continue,
            };
            gone = tx.send(chunk(delta, None)).await.is_err();
        }
        let o = handle.wait().await;
        let _ = tx.send(chunk(json!({}), Some("stop"))).await;
        if include_usage {
            let _ = tx
                .send(json!({"id": id, "object": "chat.completion.chunk", "created": now(), "model": model_name,
                             "choices": [], "usage": usage_json(&o.usage)}))
                .await;
        }
    });
    Ok(sse_chunks(rx).into_response())
}

/// OpenAI-style SSE: bare `data:` lines, ending with `[DONE]`.
fn sse_chunks(
    rx: mpsc::Receiver<Value>,
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let body = futures::stream::unfold(Some(rx), |state| async move {
        let mut rx = state?;
        match rx.recv().await {
            Some(v) => Some((Ok(Event::default().data(v.to_string())), Some(rx))),
            None => Some((Ok(Event::default().data("[DONE]")), None)),
        }
    });
    Sse::new(body).keep_alive(KeepAlive::default())
}

async fn list_models(State(s): State<AppState>) -> ApiResult<Json<Value>> {
    let mut data: Vec<Value> = s
        .gai
        .models()
        .catalog()
        .into_iter()
        .map(|e| json!({"id": e.info.id, "object": "model", "created": 0, "owned_by": e.info.provider}))
        .collect();
    data.push(json!({"id": "agent:assistant", "object": "model", "created": 0, "owned_by": "gai"}));
    for a in s.rt.store().list_agents().await? {
        data.push(json!({"id": format!("agent:{}", a.name), "object": "model", "created": 0, "owned_by": "gai"}));
    }
    Ok(Json(json!({"object": "list", "data": data})))
}
