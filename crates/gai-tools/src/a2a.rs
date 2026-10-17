//! Remote agents over the A2A protocol (JSON-RPC 2.0 over HTTP), as tools.
//!
//! ```toml
//! [a2a_agents.travel]
//! url = "https://agents.example.com/a2a/travel"   # endpoint or card URL
//! headers = { Authorization = "Bearer ${TRAVEL_TOKEN}" }
//! description = "Books trips."
//! ```
//!
//! Each becomes a tool `a2a__<name>` that sends a task with `message/send`
//! and, if the remote answers before finishing, polls `tasks/get` until it
//! does. A `context_id` from one answer continues that conversation.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::OnceCell;

use crate::{SharedTool, ToolError, ToolOutput, typed_tool};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct A2aAgentConfig {
    /// The agent's JSON-RPC endpoint, or its agent card URL
    /// (`…/.well-known/agent-card.json`), which names the endpoint.
    pub url: String,
    /// HTTP headers; values may use `${VAR}`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// When to use the agent; defaults to its card's description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// How long to wait for a task to finish (default 600 s).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// What a remote agent answered.
#[derive(Clone, Debug, Default, Serialize)]
pub struct A2aReply {
    pub text: String,
    pub task_id: Option<String>,
    pub context_id: Option<String>,
    /// The task's final state (`completed`, `failed`...), or `message` when
    /// the agent answered with a plain message.
    pub state: String,
}

pub struct A2aAgent {
    name: String,
    config: A2aAgentConfig,
    http: reqwest::Client,
    /// The endpoint, read from the card when `url` names one.
    endpoint: OnceCell<String>,
}

fn rpc_error(v: &Value) -> Option<String> {
    v.get("error").map(|e| {
        format!(
            "{} (code {})",
            e["message"].as_str().unwrap_or("error"),
            e["code"].as_i64().unwrap_or(0)
        )
    })
}

/// The text of A2A parts (`kind: text`, and data parts as JSON).
fn parts_text(parts: &Value) -> String {
    parts
        .as_array()
        .map(|ps| {
            ps.iter()
                .filter_map(|p| match p["kind"].as_str().or(p["type"].as_str()) {
                    Some("text") => p["text"].as_str().map(str::to_owned),
                    Some("data") => Some(p["data"].to_string()),
                    Some("file") => Some(format!(
                        "[file {}]",
                        p["file"]["name"].as_str().unwrap_or("attachment")
                    )),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// The answer in a task: its artifacts, else its status message.
fn task_text(task: &Value) -> String {
    let from_artifacts: Vec<String> = task["artifacts"]
        .as_array()
        .map(|a| a.iter().map(|x| parts_text(&x["parts"])).collect())
        .unwrap_or_default();
    let joined = from_artifacts.join("\n\n");
    if !joined.trim().is_empty() {
        return joined;
    }
    parts_text(&task["status"]["message"]["parts"])
}

const TERMINAL: [&str; 5] = ["completed", "failed", "canceled", "rejected", "unknown"];

impl A2aAgent {
    pub fn new(name: &str, config: A2aAgentConfig, http: reqwest::Client) -> Arc<Self> {
        Arc::new(Self {
            name: name.to_owned(),
            config,
            http,
            endpoint: OnceCell::new(),
        })
    }

    fn headers(&self) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        for (k, v) in &self.config.headers {
            if let (Ok(k), Ok(v)) = (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                reqwest::header::HeaderValue::from_str(&crate::mcp::expand(v)),
            ) {
                h.insert(k, v);
            }
        }
        h
    }

    /// The JSON-RPC endpoint: `url`, or the `url` in the card it points at.
    async fn endpoint(&self) -> Result<&str, String> {
        self.endpoint
            .get_or_try_init(|| async {
                let url = self.config.url.trim().to_owned();
                if !url.contains("/.well-known/") {
                    return Ok(url);
                }
                let card: Value = self
                    .http
                    .get(&url)
                    .headers(self.headers())
                    .send()
                    .await
                    .map_err(|e| format!("agent card {url}: {e}"))?
                    .json()
                    .await
                    .map_err(|e| format!("agent card {url}: {e}"))?;
                card["url"]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("agent card {url} names no url"))
            })
            .await
            .map(String::as_str)
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let endpoint = self.endpoint().await?;
        let body = json!({"jsonrpc": "2.0", "id": gai_core::new_id("req"), "method": method, "params": params});
        let resp = self
            .http
            .post(endpoint)
            .headers(self.headers())
            .timeout(Duration::from_secs(120))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("{}: {e}", self.name))?;
        let status = resp.status();
        let v: Value = resp
            .json()
            .await
            .map_err(|e| format!("{}: HTTP {status}, {e}", self.name))?;
        if let Some(e) = rpc_error(&v) {
            return Err(format!("{}: {e}", self.name));
        }
        Ok(v["result"].clone())
    }

    /// Send a task and wait for the answer.
    pub async fn send(
        &self,
        text: &str,
        context_id: Option<&str>,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<A2aReply, String> {
        let mut message = json!({
            "kind": "message",
            "role": "user",
            "messageId": gai_core::new_id("msg"),
            "parts": [{"kind": "text", "text": text}],
        });
        if let Some(c) = context_id {
            message["contextId"] = json!(c);
        }
        let mut result = self
            .call(
                "message/send",
                json!({"message": message, "configuration": {"blocking": true, "acceptedOutputModes": ["text/plain", "application/json"]}}),
            )
            .await?;
        if result["kind"] == "message" {
            return Ok(A2aReply {
                text: parts_text(&result["parts"]),
                task_id: None,
                context_id: result["contextId"].as_str().map(str::to_owned),
                state: "message".into(),
            });
        }
        let deadline =
            Instant::now() + Duration::from_secs(self.config.timeout_secs.unwrap_or(600));
        loop {
            let state = result["status"]["state"]
                .as_str()
                .unwrap_or("unknown")
                .to_owned();
            let task_id = result["id"].as_str().map(str::to_owned);
            if TERMINAL.contains(&state.as_str())
                || matches!(state.as_str(), "input-required" | "auth-required")
            {
                return Ok(A2aReply {
                    text: task_text(&result),
                    context_id: result["contextId"].as_str().map(str::to_owned),
                    task_id,
                    state,
                });
            }
            let Some(id) = task_id else {
                return Err(format!("{}: a task without an id", self.name));
            };
            if Instant::now() > deadline {
                return Err(format!(
                    "{}: task {id} is still {state} after the time limit",
                    self.name
                ));
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                _ = cancel.cancelled() => {
                    let _ = self.call("tasks/cancel", json!({"id": id})).await;
                    return Err("cancelled".into());
                }
            }
            result = self.call("tasks/get", json!({"id": id})).await?;
        }
    }
}

#[derive(Deserialize, JsonSchema)]
struct AskArgs {
    /// The task, stated fully: the remote agent sees nothing else.
    task: String,
    /// Continue the conversation an earlier answer gave this id for.
    #[serde(default)]
    context_id: Option<String>,
}

/// The tool for a remote agent.
pub fn a2a_tool(agent: Arc<A2aAgent>, tool_name: &str) -> SharedTool {
    let about = agent
        .config
        .description
        .clone()
        .unwrap_or_else(|| format!("The remote agent {}.", agent.name));
    Arc::new(typed_tool(
        tool_name,
        &format!(
            "Hand a task to a remote agent (over A2A) and get its answer. {about} It sees nothing of this conversation, so state the task fully."
        ),
        move |ctx, a: AskArgs| {
            let agent = agent.clone();
            async move {
                let r = agent
                    .send(&a.task, a.context_id.as_deref(), &ctx.cancel)
                    .await
                    .map_err(ToolError::Failed)?;
                let tail = match &r.context_id {
                    Some(c) => {
                        format!("\n\n(context_id {c}: pass it to continue this conversation)")
                    }
                    None => String::new(),
                };
                let text = if r.text.is_empty() {
                    "(no answer)".to_owned()
                } else {
                    r.text
                };
                Ok(if matches!(r.state.as_str(), "completed" | "message") {
                    ToolOutput::text(format!("{text}{tail}"))
                } else {
                    ToolOutput::error(format!("The remote task ended {}: {text}{tail}", r.state))
                })
            }
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_come_from_artifacts_then_status() {
        let task = json!({
            "status": {"state": "completed", "message": {"parts": [{"kind": "text", "text": "fallback"}]}},
            "artifacts": [{"parts": [{"kind": "text", "text": "the answer"}, {"kind": "data", "data": {"n": 1}}]}]
        });
        assert_eq!(task_text(&task), "the answer\n{\"n\":1}");
        let bare = json!({"status": {"state": "failed", "message": {"parts": [{"kind": "text", "text": "boom"}]}}});
        assert_eq!(task_text(&bare), "boom");
    }
}
