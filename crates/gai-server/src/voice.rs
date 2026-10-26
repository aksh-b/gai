//! Talking to an agent: `GET /v1/voice` upgrades to a WebSocket (browsers
//! pass their API key as the subprotocol `bearer.<key>`, next to `gai`).
//!
//! The first message is `{"type": "start", "agent": "...", "session_id":
//! "...", "voice": "..."}`. Then, in the usual (cascade) mode, the client
//! sends each utterance as binary audio frames followed by
//! `{"type": "commit", "media_type": "audio/webm"}`, or typed turns as
//! `{"type": "text", "text": "..."}`. The server transcribes, runs the agent
//! (keeping one session across turns) and streams back the reply: `delta`
//! text as it is written and `audio` for each sentence as soon as it is
//! spoken. `{"type": "interrupt"}` stops a reply mid-way (barge-in: send it
//! when the user starts talking over the agent).
//!
//! Agents with `media.realtime` (a speech-to-speech model such as
//! `openai/gpt-realtime`) are relayed to the provider's realtime session
//! instead: the client streams 24 kHz 16-bit mono PCM as binary frames, the
//! provider detects turns and barge-in (`speech_started`), and the agent's
//! tools run here.

use std::sync::{Arc, Mutex};

use axum::Extension;
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use axum::routing::get;
use base64::Engine;
use futures::{SinkExt, StreamExt};
use gai::agent::{AgentSpec, MediaSettings, RunEvent, RunRequest};
use gai::core::Usage;
use gai::llm::{SharedTextToSpeech, SpeechRequest, TranscribeRequest};
use gai::tools::{ToolCtx, ToolPolicy};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::AppState;
use crate::auth::{Principal, charge};
use crate::error::ApiResult;
use crate::orgs::Tenant;

pub fn routes() -> Router<AppState> {
    Router::new().route("/v1/voice", get(upgrade))
}

/// Audio a turn may carry: providers take up to 25 MB.
const MAX_TURN_BYTES: usize = 25 * 1024 * 1024;

async fn upgrade(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    p.check_budget()?;
    // Browsers offer `gai` and their key as subprotocols; answer `gai`.
    Ok(ws
        .protocols(["gai"])
        .max_message_size(MAX_TURN_BYTES)
        .on_upgrade(move |socket| session(s, p, socket)))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Start {
    agent: Option<String>,
    session_id: Option<String>,
    /// A voice for the agent's speech.
    voice: Option<String>,
    language: Option<String>,
    /// `cascade` to transcribe, run and speak even when the agent has a
    /// realtime model.
    mode: Option<String>,
}

#[derive(Deserialize)]
struct ClientMessage {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    media_type: Option<String>,
}

/// Messages to the client, written in order by one task.
#[derive(Clone)]
struct Out(mpsc::Sender<Value>);

impl Out {
    async fn send(&self, v: Value) {
        let _ = self.0.send(v).await;
    }

    async fn error(&self, message: impl std::fmt::Display) {
        self.send(json!({"type": "error", "message": message.to_string()}))
            .await;
    }
}

async fn session(s: AppState, p: Principal, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::channel::<Value>(512);
    let writer = tokio::spawn(async move {
        while let Some(v) = rx.recv().await {
            if sink
                .send(Message::Text(v.to_string().into()))
                .await
                .is_err()
            {
                break;
            }
        }
        let _ = sink.close().await;
    });
    let out = Out(tx);
    let start = loop {
        match stream.next().await {
            Some(Ok(Message::Text(t))) => match serde_json::from_str::<Value>(t.as_str()) {
                Ok(v) if v["type"] == "start" => {
                    break serde_json::from_value::<Start>(v).unwrap_or_default();
                }
                _ => out.error("send {\"type\": \"start\"} first").await,
            },
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            _ => {
                drop(out);
                let _ = writer.await;
                return;
            }
        }
    };
    let found = match &start.agent {
        Some(name) => s.gai.find_agent(name).await,
        None => Ok((s.gai.default_agent(), None)),
    };
    match found {
        Err(e) => out.error(e).await,
        Ok((spec, version)) => {
            let media = spec.media.over(&s.rt.config().media);
            match media.realtime.clone() {
                Some(model) if start.mode.as_deref() != Some("cascade") => {
                    realtime(&s, &p, spec, &model, &start, &mut stream, &out).await
                }
                _ => cascade(&s, &p, spec, version, media, start, &mut stream, &out).await,
            }
        }
    }
    drop(out);
    let _ = writer.await;
}

/// Log a speech model's cost and charge it to the key.
async fn account(s: &AppState, p: &Principal, model: &str, cost: Option<f64>) {
    let store = s.rt.store();
    if let Err(e) = store
        .record_usage(None, model, &Usage::default(), cost)
        .await
    {
        tracing::warn!(error = %e, "could not record voice usage");
    }
    charge(store, p.key_id(), cost.unwrap_or(0.0)).await;
}

// ---- cascade: transcribe, run, speak ----

struct Ctx {
    s: AppState,
    p: Principal,
    spec: AgentSpec,
    version: Option<u32>,
    media: MediaSettings,
    start: Start,
    tts: Option<SharedTextToSpeech>,
    session_id: Mutex<Option<String>>,
    out: Out,
}

enum Turn {
    Text(String),
    Audio(Vec<u8>, String),
}

#[allow(clippy::too_many_arguments)]
async fn cascade(
    s: &AppState,
    p: &Principal,
    spec: AgentSpec,
    version: Option<u32>,
    media: MediaSettings,
    start: Start,
    stream: &mut futures::stream::SplitStream<WebSocket>,
    out: &Out,
) {
    let tts = match &media.speech {
        Some(m) => match s.gai.models().speech(m) {
            Ok(t) => Some(t),
            Err(e) => {
                out.error(format!("speech model {m}: {e}")).await;
                None
            }
        },
        None => None,
    };
    out.send(json!({
        "type": "ready",
        "mode": "cascade",
        "agent": spec.name,
        "session_id": start.session_id,
        "transcription": media.transcription,
        "speech": media.speech,
    }))
    .await;
    let ctx = Arc::new(Ctx {
        s: s.clone(),
        p: p.clone(),
        session_id: Mutex::new(start.session_id.clone()),
        spec,
        version,
        media,
        start,
        tts,
        out: out.clone(),
    });
    let mut audio: Vec<u8> = Vec::new();
    let mut current: Option<(CancellationToken, tokio::task::JoinHandle<()>)> = None;
    let interrupt = |current: &mut Option<(CancellationToken, tokio::task::JoinHandle<()>)>| {
        if let Some((cancel, _)) = current.take() {
            cancel.cancel();
        }
    };
    while let Some(msg) = stream.next().await {
        match msg {
            Ok(Message::Binary(b)) => {
                if audio.len() + b.len() > MAX_TURN_BYTES {
                    out.error("this turn's audio is over 25 MB").await;
                    audio.clear();
                } else {
                    audio.extend_from_slice(&b);
                }
            }
            Ok(Message::Text(t)) => {
                let Ok(m) = serde_json::from_str::<ClientMessage>(t.as_str()) else {
                    out.error("messages are JSON with a type").await;
                    continue;
                };
                let turn = match m.kind.as_str() {
                    "commit" => {
                        if audio.is_empty() {
                            out.error("no audio to commit").await;
                            continue;
                        }
                        Turn::Audio(
                            std::mem::take(&mut audio),
                            m.media_type.unwrap_or_else(|| "audio/webm".into()),
                        )
                    }
                    "text" => match m.text.filter(|t| !t.trim().is_empty()) {
                        Some(t) => Turn::Text(t),
                        None => continue,
                    },
                    "interrupt" => {
                        interrupt(&mut current);
                        continue;
                    }
                    "clear" => {
                        audio.clear();
                        continue;
                    }
                    "stop" => break,
                    other => {
                        out.error(format!("unknown message type {other:?}")).await;
                        continue;
                    }
                };
                if let Err(e) = ctx.p.check_budget() {
                    out.error(e.message()).await;
                    continue;
                }
                interrupt(&mut current);
                let cancel = CancellationToken::new();
                let task = tokio::spawn(reply(ctx.clone(), turn, cancel.clone()));
                current = Some((cancel, task));
            }
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => {}
        }
    }
    // The client left: stop talking, but let a run in flight finish.
    if let Some((cancel, _)) = current {
        cancel.cancel();
    }
}

/// One turn: the user's words in, the agent's reply out as text and speech.
async fn reply(ctx: Arc<Ctx>, turn: Turn, cancel: CancellationToken) {
    let out = &ctx.out;
    let text = match turn {
        Turn::Text(t) => t,
        Turn::Audio(audio, media_type) => {
            let Some(model) = ctx.media.transcription.as_deref() else {
                return out
                    .error("no transcription model: set transcription under [media] in gai.toml")
                    .await;
            };
            let heard = async {
                let m = ctx.s.gai.models().transcription(model)?;
                let t = m
                    .transcribe(TranscribeRequest {
                        audio,
                        media_type,
                        language: ctx.start.language.clone(),
                        ..Default::default()
                    })
                    .await?;
                account(&ctx.s, &ctx.p, &m.info().id, m.info().media_cost(&t.usage)).await;
                Ok::<_, gai::llm::LlmError>(t.text)
            };
            let heard = tokio::select! {
                _ = cancel.cancelled() => return,
                h = heard => h,
            };
            match heard {
                Ok(t) => {
                    out.send(json!({"type": "transcript", "text": t})).await;
                    if t.trim().is_empty() {
                        return;
                    }
                    t
                }
                Err(e) => return out.error(format!("transcription: {e}")).await,
            }
        }
    };
    let mut req = RunRequest::new(ctx.spec.clone(), text);
    req.agent_version = ctx.version;
    req.session_id = ctx.session_id.lock().expect("voice session").clone();
    ctx.p.stamp(&mut req.metadata);
    req.metadata.insert("channel".into(), json!("voice"));
    let mut run = match ctx.s.rt.start(req).await {
        Ok(h) => h,
        Err(e) => return out.error(e).await,
    };
    *ctx.session_id.lock().expect("voice session") = Some(run.session_id.clone());
    out.send(json!({"type": "reply_started", "run_id": run.run_id, "session_id": run.session_id}))
        .await;
    let speaker = ctx.tts.clone().map(|tts| Speaker::start(ctx.clone(), tts));
    let mut sentences = Sentences::default();
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                ctx.s.rt.cancel(&run.run_id);
                if let Some(sp) = speaker {
                    sp.abort();
                }
                out.send(json!({"type": "interrupted", "run_id": run.run_id})).await;
                return;
            }
            e = run.next_event() => match e.map(|e| e.event) {
                None => break,
                Some(RunEvent::TextDelta { text }) => {
                    out.send(json!({"type": "delta", "text": text})).await;
                    if let Some(sp) = &speaker {
                        for s in sentences.push(&text) {
                            sp.say(s);
                        }
                    }
                }
                Some(RunEvent::ToolCall { name, .. }) => {
                    out.send(json!({"type": "tool", "name": name})).await;
                }
                Some(RunEvent::ApprovalRequired { id, name, .. }) => {
                    out.send(json!({"type": "approval_required", "id": id, "name": name, "run_id": run.run_id})).await;
                }
                Some(_) => {}
            }
        }
    }
    let run_id = run.run_id.clone();
    let outcome = run.wait().await;
    if let Some(sp) = speaker {
        if let Some(rest) = sentences.finish() {
            sp.say(rest);
        }
        tokio::select! {
            _ = cancel.cancelled() => {
                out.send(json!({"type": "interrupted", "run_id": run_id})).await;
                return;
            }
            _ = sp.finish() => {}
        }
    }
    out.send(json!({
        "type": "reply_done",
        "run_id": outcome.run_id,
        "session_id": outcome.session_id,
        "status": outcome.status,
        "text": outcome.text,
        "error": outcome.error,
        "cost": outcome.cost,
    }))
    .await;
}

/// Speaks sentences in order, one at a time, sending each one's audio as
/// soon as it is ready.
struct Speaker {
    tx: mpsc::UnboundedSender<String>,
    task: tokio::task::JoinHandle<()>,
}

impl Speaker {
    fn start(ctx: Arc<Ctx>, tts: SharedTextToSpeech) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let task = tokio::spawn(async move {
            let mut seq = 0u32;
            let voice = ctx.start.voice.clone().or_else(|| ctx.media.voice.clone());
            while let Some(text) = rx.recv().await {
                let spoken = tts
                    .speak(SpeechRequest {
                        text: text.clone(),
                        voice: voice.clone(),
                        format: Some("mp3".into()),
                        ..Default::default()
                    })
                    .await;
                match spoken {
                    Ok(sp) => {
                        account(
                            &ctx.s,
                            &ctx.p,
                            &tts.info().id,
                            tts.info().media_cost(&sp.usage),
                        )
                        .await;
                        ctx.out
                            .send(json!({
                                "type": "audio",
                                "seq": seq,
                                "media_type": sp.media_type,
                                "text": text,
                                "data": base64::engine::general_purpose::STANDARD.encode(&sp.audio),
                            }))
                            .await;
                        seq += 1;
                    }
                    Err(e) => {
                        ctx.out.error(format!("speech: {e}")).await;
                        break;
                    }
                }
            }
        });
        Self { tx, task }
    }

    fn say(&self, text: String) {
        let _ = self.tx.send(text);
    }

    /// Wait until everything said so far is spoken.
    async fn finish(self) {
        drop(self.tx);
        let _ = self.task.await;
    }

    fn abort(self) {
        self.task.abort();
    }
}

/// Cuts streamed text into sentences to speak, leaving out Markdown marks.
#[derive(Default)]
struct Sentences {
    buf: String,
}

/// Don't speak tiny fragments ("Hi." waits for the next sentence).
const MIN_SENTENCE: usize = 12;
/// Long stretches without an end are spoken at a pause.
const MAX_SENTENCE: usize = 240;

fn speakable(s: &str) -> Option<String> {
    let cleaned: String = s
        .chars()
        .filter(|c| !matches!(c, '*' | '#' | '`' | '_' | '>' | '|'))
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    (!cleaned.is_empty()).then_some(cleaned)
}

impl Sentences {
    fn push(&mut self, text: &str) -> Vec<String> {
        self.buf.push_str(text);
        let mut out = Vec::new();
        loop {
            let chars: Vec<(usize, char)> = self.buf.char_indices().collect();
            let mut cut = None;
            for (i, &(at, c)) in chars.iter().enumerate() {
                let end = at + c.len_utf8();
                let next_is_space = chars.get(i + 1).is_some_and(|(_, n)| n.is_whitespace());
                if (c == '\n' && end > 1)
                    || (matches!(c, '.' | '!' | '?' | ';' | ':')
                        && next_is_space
                        && end >= MIN_SENTENCE)
                {
                    cut = Some(end);
                    break;
                }
            }
            if cut.is_none() && self.buf.len() > MAX_SENTENCE {
                cut = self.buf[..MAX_SENTENCE]
                    .rfind([',', ' '])
                    .map(|i| i + 1)
                    .or(Some(
                        (0..=MAX_SENTENCE)
                            .rev()
                            .find(|i| self.buf.is_char_boundary(*i))
                            .unwrap_or(0),
                    ));
            }
            let Some(at) = cut else { break };
            let head: String = self.buf.drain(..at).collect();
            out.extend(speakable(&head));
        }
        out
    }

    fn finish(&mut self) -> Option<String> {
        speakable(&std::mem::take(&mut self.buf))
    }
}

// ---- realtime: relay to a provider's speech-to-speech session ----

type Upstream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(s: &AppState, model: &str) -> Result<(Upstream, String), String> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
    let target = s.gai.models().realtime(model).map_err(|e| e.to_string())?;
    let mut req = target
        .url
        .as_str()
        .into_client_request()
        .map_err(|e| e.to_string())?;
    for (k, v) in &target.headers {
        if let (Ok(k), Ok(v)) = (HeaderName::try_from(k.as_str()), HeaderValue::from_str(v)) {
            req.headers_mut().insert(k, v);
        }
    }
    let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| e.to_string())?
    .with_root_certificates(rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    })
    .with_no_client_auth();
    let (ws, _) = tokio_tungstenite::connect_async_tls_with_config(
        req,
        None,
        false,
        Some(tokio_tungstenite::Connector::Rustls(Arc::new(tls))),
    )
    .await
    .map_err(|e| format!("could not reach {}: {e}", target.url))?;
    Ok((ws, target.model))
}

/// The agent's tools that run without approval, as realtime functions.
fn realtime_tools(s: &AppState, spec: &AgentSpec) -> Vec<gai::tools::SharedTool> {
    spec.tools
        .iter()
        .filter_map(|name| s.rt.tool(name))
        .filter(|t| {
            let name = t.spec().name;
            spec.tool_policy
                .get(&name)
                .copied()
                .unwrap_or_else(|| t.default_policy())
                == ToolPolicy::Auto
        })
        .collect()
}

async fn realtime(
    s: &AppState,
    _p: &Principal,
    spec: AgentSpec,
    model: &str,
    start: &Start,
    stream: &mut futures::stream::SplitStream<WebSocket>,
    out: &Out,
) {
    use tokio_tungstenite::tungstenite::Message as Up;
    let (upstream, _) = match connect(s, model).await {
        Ok(u) => u,
        Err(e) => return out.error(e).await,
    };
    let (mut up_tx, mut up_rx) = upstream.split();
    let tools = realtime_tools(s, &spec);
    let functions: Vec<Value> = tools
        .iter()
        .map(|t| {
            let ts = t.spec();
            json!({"type": "function", "name": ts.name, "description": ts.description, "parameters": ts.input_schema})
        })
        .collect();
    let mut output = json!({"format": {"type": "audio/pcm", "rate": 24000}});
    if let Some(v) = &start.voice {
        output["voice"] = json!(v);
    }
    let mut input = json!({
        "format": {"type": "audio/pcm", "rate": 24000},
        "transcription": {"model": "gpt-4o-mini-transcribe"},
        "turn_detection": {"type": "server_vad", "create_response": true, "interrupt_response": true},
    });
    if let Some(l) = &start.language {
        input["transcription"]["language"] = json!(l);
    }
    let update = json!({
        "type": "session.update",
        "session": {
            "type": "realtime",
            "instructions": spec.system.clone().unwrap_or_default(),
            "audio": {"input": input, "output": output},
            "tools": functions,
            "tool_choice": "auto",
        }
    });
    if up_tx
        .send(Up::Text(update.to_string().into()))
        .await
        .is_err()
    {
        return out.error("the realtime session closed").await;
    }
    out.send(json!({
        "type": "ready",
        "mode": "realtime",
        "agent": spec.name,
        "model": model,
        "input": {"format": "pcm16", "rate": 24000},
        "output": {"format": "pcm16", "rate": 24000},
    }))
    .await;

    let workspace =
        s.rt.config()
            .data_dir
            .join("voice")
            .join(gai::core::new_id("ws"));
    let (done_tx, mut done_rx) = mpsc::channel::<(String, String)>(16);
    let mut pending = 0usize;
    let mut calls_this_response = 0usize;
    let mut want_create = false;
    let mut said = String::new();
    let send_up = |v: Value| Up::Text(v.to_string().into());
    loop {
        tokio::select! {
            msg = stream.next() => match msg {
                Some(Ok(Message::Binary(pcm))) => {
                    let ev = json!({
                        "type": "input_audio_buffer.append",
                        "audio": base64::engine::general_purpose::STANDARD.encode(&pcm),
                    });
                    if up_tx.send(send_up(ev)).await.is_err() { break; }
                }
                Some(Ok(Message::Text(t))) => {
                    let Ok(m) = serde_json::from_str::<ClientMessage>(t.as_str()) else { continue };
                    let sent = match m.kind.as_str() {
                        "text" => {
                            let text = m.text.unwrap_or_default();
                            let item = json!({"type": "conversation.item.create", "item": {
                                "type": "message", "role": "user",
                                "content": [{"type": "input_text", "text": text}],
                            }});
                            out.send(json!({"type": "transcript", "text": text})).await;
                            up_tx.send(send_up(item)).await.is_ok()
                                && up_tx.send(send_up(json!({"type": "response.create"}))).await.is_ok()
                        }
                        "interrupt" => up_tx.send(send_up(json!({"type": "response.cancel"}))).await.is_ok(),
                        "commit" => up_tx.send(send_up(json!({"type": "input_audio_buffer.commit"}))).await.is_ok(),
                        "stop" => break,
                        _ => true,
                    };
                    if !sent { break; }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            done = done_rx.recv() => {
                let Some((call_id, result)) = done else { continue };
                let item = json!({"type": "conversation.item.create", "item": {
                    "type": "function_call_output", "call_id": call_id, "output": result,
                }});
                if up_tx.send(send_up(item)).await.is_err() { break; }
                pending = pending.saturating_sub(1);
                if pending == 0 && want_create {
                    want_create = false;
                    if up_tx.send(send_up(json!({"type": "response.create"}))).await.is_err() { break; }
                }
            }
            ev = up_rx.next() => {
                let text = match ev {
                    Some(Ok(Up::Text(t))) => t.to_string(),
                    Some(Ok(Up::Close(_))) | None | Some(Err(_)) => {
                        out.error("the realtime session ended").await;
                        break;
                    }
                    Some(Ok(_)) => continue,
                };
                let Ok(ev) = serde_json::from_str::<Value>(&text) else { continue };
                match ev["type"].as_str().unwrap_or_default() {
                    "input_audio_buffer.speech_started" => {
                        out.send(json!({"type": "speech_started"})).await;
                    }
                    "conversation.item.input_audio_transcription.completed" => {
                        out.send(json!({"type": "transcript", "text": ev["transcript"]})).await;
                    }
                    "response.created" => {
                        calls_this_response = 0;
                        said.clear();
                        out.send(json!({"type": "reply_started"})).await;
                    }
                    "response.output_audio.delta" | "response.audio.delta" => {
                        out.send(json!({"type": "audio", "media_type": "audio/pcm;rate=24000", "data": ev["delta"]})).await;
                    }
                    "response.output_audio_transcript.delta" | "response.audio_transcript.delta"
                    | "response.output_text.delta" | "response.text.delta" => {
                        let d = ev["delta"].as_str().unwrap_or_default();
                        said.push_str(d);
                        out.send(json!({"type": "delta", "text": d})).await;
                    }
                    "response.function_call_arguments.done" => {
                        let name = ev["name"].as_str().unwrap_or_default().to_owned();
                        let call_id = ev["call_id"].as_str().unwrap_or_default().to_owned();
                        out.send(json!({"type": "tool", "name": name})).await;
                        pending += 1;
                        calls_this_response += 1;
                        let tool = tools.iter().find(|t| t.spec().name == name).cloned();
                        let input: Value = ev["arguments"]
                            .as_str()
                            .and_then(|a| serde_json::from_str(a).ok())
                            .unwrap_or_else(|| json!({}));
                        let (done_tx, ctx) = (done_tx.clone(), tool_ctx(s, &spec, &workspace));
                        tokio::spawn(async move {
                            let result = match tool {
                                None => format!("no tool {name}"),
                                Some(t) => match t.call(&ctx, input).await {
                                    Ok(o) => o.content.iter().filter_map(|b| b.as_text()).collect::<Vec<_>>().join("\n"),
                                    Err(e) => format!("error: {e}"),
                                },
                            };
                            let _ = done_tx.send((call_id, result)).await;
                        });
                    }
                    "response.done" => {
                        let usage = &ev["response"]["usage"];
                        let u = Usage {
                            input_tokens: usage["input_tokens"].as_u64().unwrap_or(0),
                            output_tokens: usage["output_tokens"].as_u64().unwrap_or(0),
                            ..Default::default()
                        };
                        if let Err(e) = s.rt.store().record_usage(None, model, &u, None).await {
                            tracing::warn!(error = %e, "could not record realtime usage");
                        }
                        if calls_this_response > 0 {
                            if pending == 0 {
                                if up_tx.send(send_up(json!({"type": "response.create"}))).await.is_err() { break; }
                            } else {
                                want_create = true;
                            }
                        } else {
                            out.send(json!({"type": "reply_done", "text": said, "status": ev["response"]["status"]})).await;
                        }
                    }
                    "error" => {
                        out.error(ev["error"]["message"].as_str().unwrap_or("realtime error")).await;
                    }
                    _ => {}
                }
            }
        }
    }
    let _ = up_tx.close().await;
    let _ = std::fs::remove_dir_all(&workspace);
}

fn tool_ctx(s: &AppState, spec: &AgentSpec, workspace: &std::path::Path) -> ToolCtx {
    let _ = std::fs::create_dir_all(workspace);
    let mut ctx = ToolCtx::for_workspace(workspace);
    ctx.agent = spec.name.clone();
    ctx.http = s.rt.http().clone();
    ctx.settings = Arc::new(s.rt.config().tools.clone());
    ctx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentences_come_out_as_they_end() {
        let mut s = Sentences::default();
        assert!(s.push("Hi. Sure").is_empty(), "too short to speak alone");
        assert_eq!(
            s.push(", I can help with that. Th"),
            vec!["Hi. Sure, I can help with that."]
        );
        assert_eq!(s.push("e **total** is 42.\n"), vec!["The total is 42."]);
        assert_eq!(s.push("- one"), Vec::<String>::new());
        assert_eq!(s.finish(), Some("- one".into()));
        let long = "word ".repeat(80);
        let parts = s.push(&long);
        assert_eq!(parts.len(), 1);
        assert!(parts[0].len() <= MAX_SENTENCE);
        assert!(s.push("1.5 is a number").is_empty(), "no cut inside 1.5");
    }
}
