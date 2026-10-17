//! gai agents over A2A: agent cards for discovery and the JSON-RPC task
//! endpoints (`message/send`, `message/stream`, `tasks/get`,
//! `tasks/cancel`). A task is a gai run; its context is the run's session.
//!
//! - `GET /.well-known/agent-card.json`: a card for gai as a whole, one skill
//!   per exposed agent; its endpoint, `POST /a2a`, picks the agent from
//!   `message.metadata.agent` (or `skill`), else `[server.a2a] default_agent`.
//! - `GET /a2a/{agent}/.well-known/agent-card.json` and `POST /a2a/{agent}`:
//!   one agent.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use base64::Engine;
use futures::StreamExt as _;
use gai::McpServeSettings;
use gai::agent::{RunEvent, RunRequest};
use gai::core::{ContentBlock, MediaSource};
use gai::store::{Run, RunStatus};
use serde_json::{Map, Value, json};

use crate::AppState;
use crate::auth::Principal;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/.well-known/agent-card.json", get(catalog_card))
        .route("/.well-known/agent.json", get(catalog_card))
        .route("/a2a", post(rpc_default))
        .route("/a2a/{agent}", post(rpc_agent))
        .route("/a2a/{agent}/.well-known/agent-card.json", get(agent_card))
        .route("/a2a/{agent}/.well-known/agent.json", get(agent_card))
}

/// The address clients use: `[server.a2a] public_url`, else the request's.
fn base_url(s: &AppState, headers: &HeaderMap) -> String {
    if let Some(u) = &s.gai.config().server.a2a.public_url {
        return u.trim_end_matches('/').to_owned();
    }
    let h = |n: &str| headers.get(n).and_then(|v| v.to_str().ok());
    let host = h("x-forwarded-host").or(h("host")).unwrap_or("localhost");
    let proto = h("x-forwarded-proto").unwrap_or("http");
    format!("{proto}://{host}")
}

/// Exposed stored agents with their descriptions and versions.
async fn exposed(s: &AppState) -> Vec<(String, Option<String>, u32)> {
    let cfg = &s.gai.config().server.a2a;
    let Ok(all) = s.rt.store().list_agents().await else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for a in all {
        if McpServeSettings::matches(&cfg.agents, &a.name) {
            let desc =
                s.rt.load_agent(&a.name, None)
                    .await
                    .ok()
                    .and_then(|(x, _)| x.description);
            out.push((a.name, desc, a.latest_version));
        }
    }
    out
}

fn card(name: &str, description: &str, url: &str, version: &str, skills: Vec<Value>) -> Value {
    json!({
        "protocolVersion": "0.3.0",
        "name": name,
        "description": description,
        "url": url,
        "preferredTransport": "JSONRPC",
        "version": version,
        "capabilities": {"streaming": true, "pushNotifications": false, "stateTransitionHistory": false},
        "defaultInputModes": ["text/plain", "application/json", "image/png", "image/jpeg"],
        "defaultOutputModes": ["text/plain", "application/json"],
        "skills": skills,
        "securitySchemes": {"bearer": {"type": "http", "scheme": "bearer", "description": "A gai API key with the run scope"}},
        "security": [{"bearer": []}],
        "provider": {"organization": "gai", "url": url},
    })
}

fn skill(name: &str, desc: &Option<String>) -> Value {
    json!({
        "id": name,
        "name": name,
        "description": desc.clone().unwrap_or_else(|| format!("The gai agent {name}.")),
        "tags": ["gai", "agent"],
    })
}

async fn catalog_card(State(s): State<AppState>, headers: HeaderMap) -> Response {
    if !s.gai.config().server.a2a.enabled {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }
    let base = base_url(&s, &headers);
    let skills = exposed(&s)
        .await
        .iter()
        .map(|(n, d, _)| skill(n, d))
        .collect();
    Json(card(
        "gai",
        "A gai server. Each skill is an agent; name it in message.metadata.agent.",
        &format!("{base}/a2a"),
        env!("CARGO_PKG_VERSION"),
        skills,
    ))
    .into_response()
}

async fn agent_card(
    State(s): State<AppState>,
    Path(agent): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !s.gai.config().server.a2a.enabled {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }
    let base = base_url(&s, &headers);
    match exposed(&s).await.into_iter().find(|(n, _, _)| *n == agent) {
        Some((n, d, v)) => Json(card(
            &n,
            d.as_deref().unwrap_or("A gai agent."),
            &format!("{base}/a2a/{n}"),
            &format!("v{v}"),
            vec![skill(&n, &d)],
        ))
        .into_response(),
        None => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}

fn rpc_ok(id: &Value, result: Value) -> Response {
    Json(json!({"jsonrpc": "2.0", "id": id, "result": result})).into_response()
}

fn rpc_err(id: &Value, code: i64, message: impl Into<String>) -> Response {
    Json(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}}))
        .into_response()
}

const INVALID_PARAMS: i64 = -32602;
const TASK_NOT_FOUND: i64 = -32001;
const TASK_NOT_CANCELABLE: i64 = -32002;

async fn rpc_default(
    State(s): State<AppState>,
    Extension(p): Extension<Principal>,
    Json(req): Json<Value>,
) -> Response {
    let meta = &req["params"]["message"]["metadata"];
    let agent = meta["agent"]
        .as_str()
        .or(meta["skill"].as_str())
        .or(req["params"]["metadata"]["agent"].as_str())
        .map(str::to_owned)
        .or_else(|| s.gai.config().server.a2a.default_agent.clone());
    rpc(s, p, agent, req).await
}

async fn rpc_agent(
    State(s): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(agent): Path<String>,
    Json(req): Json<Value>,
) -> Response {
    rpc(s, p, Some(agent), req).await
}

async fn rpc(s: AppState, p: Principal, agent: Option<String>, req: Value) -> Response {
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    if !s.gai.config().server.a2a.enabled {
        return rpc_err(&id, -32601, "A2A is turned off on this server");
    }
    if req["jsonrpc"] != "2.0" {
        return rpc_err(&id, -32600, "not a JSON-RPC 2.0 request");
    }
    let method = req["method"].as_str().unwrap_or_default();
    let params = &req["params"];
    match method {
        "message/send" | "message/stream" => {
            if let Err(e) = p.check_budget() {
                return rpc_err(&id, -32000, e.message().to_owned());
            }
            let Some(agent) = agent else {
                return rpc_err(
                    &id,
                    INVALID_PARAMS,
                    "name the agent: POST /a2a/{agent}, or message.metadata.agent",
                );
            };
            let started = match start(&s, &p, &agent, params).await {
                Ok(h) => h,
                Err(e) => return rpc_err(&id, INVALID_PARAMS, e),
            };
            if method == "message/stream" {
                return stream(s, id, started).into_response();
            }
            let (run_id, handle) = started;
            let blocking = params["configuration"]["blocking"]
                .as_bool()
                .unwrap_or(true);
            if blocking {
                let wait = Duration::from_secs(s.gai.config().server.a2a.wait_secs.max(1));
                let _ = tokio::time::timeout(wait, handle.wait()).await;
            } else {
                tokio::spawn(handle.wait());
            }
            match s.rt.store().get_run(&run_id).await {
                Ok(Some(run)) => rpc_ok(&id, task(&s, &run)),
                _ => rpc_err(&id, -32603, "the run vanished"),
            }
        }
        "tasks/get" => match own_run(&s, &p, params["id"].as_str()).await {
            Some(run) => rpc_ok(&id, task(&s, &run)),
            None => rpc_err(&id, TASK_NOT_FOUND, "no such task"),
        },
        "tasks/cancel" => match own_run(&s, &p, params["id"].as_str()).await {
            Some(run) if !run.status.is_terminal() && s.rt.cancel(&run.id) => {
                for _ in 0..40 {
                    if let Ok(Some(r)) = s.rt.store().get_run(&run.id).await
                        && r.status.is_terminal()
                    {
                        return rpc_ok(&id, task(&s, &r));
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                rpc_ok(&id, task(&s, &run))
            }
            Some(_) => rpc_err(
                &id,
                TASK_NOT_CANCELABLE,
                "the task has ended or is not running here",
            ),
            None => rpc_err(&id, TASK_NOT_FOUND, "no such task"),
        },
        "tasks/resubscribe"
        | "tasks/pushNotificationConfig/set"
        | "tasks/pushNotificationConfig/get" => {
            rpc_err(&id, -32004, format!("{method} is not supported"))
        }
        other => rpc_err(&id, -32601, format!("unknown method {other}")),
    }
}

/// A run started over A2A, if the caller may see it.
async fn own_run(s: &AppState, p: &Principal, id: Option<&str>) -> Option<Run> {
    let run = s.rt.store().get_run(id?).await.ok()??;
    if run.metadata["source"] != "a2a" {
        return None;
    }
    // A key only sees its own tasks; anonymous callers (open servers) see all.
    match (p.key_id(), run.metadata["key_id"].as_str()) {
        (Some(k), Some(owner)) if k != owner => None,
        _ => Some(run),
    }
}

/// Start a run for an A2A message. Returns the run id and its handle.
async fn start(
    s: &AppState,
    p: &Principal,
    agent: &str,
    params: &Value,
) -> Result<(String, gai::agent::RunHandle), String> {
    let cfg = &s.gai.config().server.a2a;
    if !McpServeSettings::matches(&cfg.agents, agent)
        || s.rt
            .store()
            .get_agent(agent, None)
            .await
            .ok()
            .flatten()
            .is_none()
    {
        return Err(format!("no agent {agent:?} here"));
    }
    let msg = &params["message"];
    let mut input = Vec::new();
    for part in msg["parts"].as_array().into_iter().flatten() {
        match part["kind"].as_str().or(part["type"].as_str()) {
            Some("text") => input.push(ContentBlock::text(
                part["text"].as_str().unwrap_or_default(),
            )),
            Some("data") => input.push(ContentBlock::text(format!(
                "```json\n{}\n```",
                part["data"]
            ))),
            Some("file") => {
                let f = &part["file"];
                let mime = f["mimeType"].as_str().unwrap_or("application/octet-stream");
                match (f["bytes"].as_str(), f["uri"].as_str()) {
                    (Some(b), _) if mime.starts_with("image/") => input.push(ContentBlock::Image {
                        source: MediaSource::Base64 {
                            media_type: mime.into(),
                            data: b.into(),
                        },
                    }),
                    (Some(b), _) if mime.starts_with("text/") || mime == "application/json" => {
                        let text = base64::engine::general_purpose::STANDARD
                            .decode(b)
                            .ok()
                            .map(|d| String::from_utf8_lossy(&d).into_owned())
                            .unwrap_or_default();
                        input.push(ContentBlock::text(format!(
                            "<file name={:?}>\n{text}\n</file>",
                            f["name"].as_str().unwrap_or("file")
                        )));
                    }
                    (_, Some(u)) if mime.starts_with("image/") => input.push(ContentBlock::Image {
                        source: MediaSource::Url { url: u.into() },
                    }),
                    _ => input.push(ContentBlock::text(format!(
                        "[a {mime} file the agent cannot read: {}]",
                        f["name"].as_str().unwrap_or("file")
                    ))),
                }
            }
            _ => {}
        }
    }
    if input.is_empty() {
        return Err("the message has no parts".into());
    }
    let (spec, version) = s.rt.resolve_agent(agent).await.map_err(|e| e.to_string())?;
    let mut req = RunRequest::with_input(spec, input);
    req.agent_version = version;
    // The context continues a conversation, if it is one of ours.
    if let Some(ctx) = msg["contextId"].as_str()
        && s.rt.store().get_session(ctx).await.ok().flatten().is_some()
    {
        req.session_id = Some(ctx.to_owned());
    }
    let mut meta = Map::new();
    meta.insert("source".into(), json!("a2a"));
    if let Some(k) = p.key_id() {
        meta.insert("key_id".into(), json!(k));
    }
    if let Some(m) = msg["messageId"].as_str() {
        meta.insert("a2a_message_id".into(), json!(m));
    }
    req.metadata = meta;
    let h = s.rt.start(req).await.map_err(|e| e.to_string())?;
    Ok((h.run_id.clone(), h))
}

/// The A2A state of a run. A run that says it is working but is not
/// running in this process (it stopped with a crash) is `unknown`.
fn state_of(run: &Run, active: bool) -> &'static str {
    match run.status {
        RunStatus::Queued => "submitted",
        RunStatus::Running | RunStatus::AwaitingApproval if active => "working",
        RunStatus::Running | RunStatus::AwaitingApproval => "unknown",
        RunStatus::Completed => "completed",
        RunStatus::Cancelled => "canceled",
        RunStatus::Failed | RunStatus::BudgetExceeded => "failed",
    }
}

fn output_text(run: &Run) -> String {
    match &run.output {
        Some(Value::String(t)) => t.clone(),
        Some(v) => serde_json::to_string_pretty(v).unwrap_or_default(),
        None => String::new(),
    }
}

fn agent_message(run: &Run, text: &str) -> Value {
    json!({
        "kind": "message",
        "role": "agent",
        "messageId": format!("{}-answer", run.id),
        "taskId": run.id,
        "contextId": run.session_id,
        "parts": [{"kind": "text", "text": text}],
    })
}

/// A run as an A2A task.
fn task(s: &AppState, run: &Run) -> Value {
    let state = state_of(run, s.rt.is_active(&run.id));
    let text = output_text(run);
    let mut status = json!({"state": state, "timestamp": run.updated_at});
    if run.status == RunStatus::AwaitingApproval {
        status["message"] =
            agent_message(run, "Waiting for a person to approve a tool call in gai.");
    } else if let Some(e) = &run.error {
        status["message"] = agent_message(run, e);
    } else if run.status.is_terminal() && !text.is_empty() {
        status["message"] = agent_message(run, &text);
    }
    let mut t = json!({
        "kind": "task",
        "id": run.id,
        "contextId": run.session_id,
        "status": status,
        "metadata": {"gai": {"agent": run.agent, "agent_version": run.agent_version, "cost_usd": run.cost, "steps": run.steps}},
    });
    if run.status == RunStatus::Completed {
        let part = match &run.output {
            Some(v @ (Value::Object(_) | Value::Array(_))) => json!({"kind": "data", "data": v}),
            _ => json!({"kind": "text", "text": text}),
        };
        t["artifacts"] = json!([{"artifactId": format!("{}-answer", run.id), "name": "answer", "parts": [part]}]);
    }
    t
}

/// `message/stream`: the task, then status and artifact updates as SSE.
fn stream(
    s: AppState,
    id: Value,
    (run_id, mut handle): (String, gai::agent::RunHandle),
) -> Sse<impl futures::Stream<Item = Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Value>(256);
    tokio::spawn(async move {
        let send = |result: Value| {
            let tx = tx.clone();
            let id = id.clone();
            async move {
                let _ = tx
                    .send(json!({"jsonrpc": "2.0", "id": id, "result": result}))
                    .await;
            }
        };
        if let Ok(Some(run)) = s.rt.store().get_run(&run_id).await {
            send(task(&s, &run)).await;
        }
        let session = handle.session_id.clone();
        let update = |state: &str, final_: bool, message: Option<Value>| {
            let mut st = json!({"state": state, "timestamp": chrono::Utc::now()});
            if let Some(m) = message {
                st["message"] = m;
            }
            json!({"kind": "status-update", "taskId": run_id, "contextId": session, "status": st, "final": final_})
        };
        let mut chunk = 0u32;
        while let Some(e) = handle.next_event().await {
            match e.event {
                RunEvent::TextDelta { text } => {
                    chunk += 1;
                    send(json!({
                        "kind": "artifact-update", "taskId": run_id, "contextId": session,
                        "artifact": {"artifactId": format!("{run_id}-stream"), "name": "answer (streaming)", "parts": [{"kind": "text", "text": text}]},
                        "append": chunk > 1, "lastChunk": false,
                    }))
                    .await
                }
                RunEvent::ApprovalRequired { name, .. } => {
                    send(update("working", false, Some(json!({
                        "kind": "message", "role": "agent", "messageId": gai::core::new_id("msg"),
                        "parts": [{"kind": "text", "text": format!("Waiting for a person to approve {name} in gai.")}],
                    })))).await
                }
                RunEvent::StepStarted { step: 1 } => send(update("working", false, None)).await,
                _ => {}
            }
        }
        handle.wait().await;
        if let Ok(Some(run)) = s.rt.store().get_run(&run_id).await {
            let t = task(&s, &run);
            if let Some(a) = t["artifacts"].get(0) {
                send(json!({"kind": "artifact-update", "taskId": run_id, "contextId": session, "artifact": a, "lastChunk": true})).await;
            }
            send(update(
                t["status"]["state"].as_str().unwrap_or("unknown"),
                true,
                t["status"].get("message").cloned(),
            ))
            .await;
        }
    });
    let events = tokio_stream_from(rx).map(|v| Ok(Event::default().data(v.to_string())));
    Sse::new(events).keep_alive(KeepAlive::default())
}

fn tokio_stream_from(
    mut rx: tokio::sync::mpsc::Receiver<Value>,
) -> impl futures::Stream<Item = Value> {
    futures::stream::poll_fn(move |cx| rx.poll_recv(cx))
}
