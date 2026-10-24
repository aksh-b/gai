//! A typed HTTP client for a running `gai serve`.
//!
//! ```no_run
//! # async fn demo() -> Result<(), gai::client::ClientError> {
//! use futures::StreamExt;
//! use gai::client::{Client, RunOptions};
//!
//! let gai = Client::new("http://localhost:8484").with_key("gai_...");
//! let outcome = gai.run("researcher", "Summarise the Rust 1.91 release").await?;
//! println!("{}", outcome.text);
//!
//! let mut events = gai.stream(RunOptions::agent("researcher"), "And 1.92?").await?;
//! while let Some(e) = events.next().await {
//!     println!("{:?}", e?.event);
//! }
//! # Ok(()) }
//! ```

use futures::StreamExt;
use futures::stream::BoxStream;
use reqwest::{Method, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use gai_agent::{Decision, EventEnvelope, PendingApproval, RunOutcome};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("could not reach the gai server: {0}")]
    Http(#[from] reqwest::Error),
    /// The server answered with an error status.
    #[error("{status}: {message}")]
    Api { status: StatusCode, message: String },
    #[error("unexpected reply: {0}")]
    Decode(String),
}

pub type Result<T, E = ClientError> = std::result::Result<T, E>;

/// A run's events, as they happen.
pub type Events = BoxStream<'static, Result<EventEnvelope>>;

/// A gai server and the API key to use with it.
#[derive(Clone, Debug)]
pub struct Client {
    base: String,
    key: Option<String>,
    http: reqwest::Client,
}

/// Which agent runs and how; see [`Client::run_with`].
#[derive(Clone, Debug, Default, Serialize)]
pub struct RunOptions {
    /// A stored agent (`name`, `name@v3`, `name@prod`); the built-in
    /// assistant when `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Continue this conversation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persona: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub auto_approve: bool,
}

impl RunOptions {
    pub fn agent(name: impl Into<String>) -> Self {
        Self {
            agent: Some(name.into()),
            ..Default::default()
        }
    }

    pub fn session(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }
}

impl Client {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base: base_url.into().trim_end_matches('/').to_owned(),
            key: None,
            http: reqwest::Client::new(),
        }
    }

    /// `GAI_SERVER` (default `http://127.0.0.1:8484`) and `GAI_API_KEY`.
    pub fn from_env() -> Self {
        let c = Self::new(
            std::env::var("GAI_SERVER").unwrap_or_else(|_| "http://127.0.0.1:8484".into()),
        );
        match std::env::var("GAI_API_KEY") {
            Ok(k) if !k.is_empty() => c.with_key(k),
            _ => c,
        }
    }

    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        let rb = self.http.request(method, format!("{}{path}", self.base));
        match &self.key {
            Some(k) => rb.bearer_auth(k),
            None => rb,
        }
    }

    async fn send<T: DeserializeOwned>(&self, rb: reqwest::RequestBuilder) -> Result<T> {
        let r = rb.send().await?;
        let status = r.status();
        let text = r.text().await?;
        if !status.is_success() {
            let message = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| {
                    v["error"]["message"]
                        .as_str()
                        .or(v["error"].as_str())
                        .map(str::to_owned)
                })
                .unwrap_or(text);
            return Err(ClientError::Api { status, message });
        }
        serde_json::from_str(&text).map_err(|e| ClientError::Decode(format!("{e}: {text}")))
    }

    /// `GET` any API path, e.g. `/v1/runs?limit=5`.
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.send(self.request(Method::GET, path)).await
    }

    /// `POST` JSON to any API path.
    pub async fn post<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> Result<T> {
        self.send(self.request(Method::POST, path).json(body)).await
    }

    /// The server's version, default model and auth mode.
    pub async fn health(&self) -> Result<Value> {
        self.get("/v1/health").await
    }

    /// Stored agents: `{summary, spec}` with the latest spec.
    pub async fn agents(&self) -> Result<Vec<Value>> {
        self.get("/v1/agents").await
    }

    /// Run `agent` on `text` and wait for the outcome.
    pub async fn run(&self, agent: &str, text: &str) -> Result<RunOutcome> {
        self.run_with(RunOptions::agent(agent), text).await
    }

    pub async fn run_with(&self, opts: RunOptions, text: &str) -> Result<RunOutcome> {
        self.post("/v1/runs", &body(&opts, text, "wait")).await
    }

    /// Start a run and follow its events as they happen. The run carries on
    /// on the server if the stream is dropped.
    pub async fn stream(&self, opts: RunOptions, text: &str) -> Result<Events> {
        let r = self
            .request(Method::POST, "/v1/runs")
            .json(&body(&opts, text, "stream"))
            .send()
            .await?;
        if !r.status().is_success() {
            let status = r.status();
            return Err(ClientError::Api {
                status,
                message: r.text().await.unwrap_or_default(),
            });
        }
        Ok(events(r))
    }

    /// Follow a run that is already going (or replay a finished one's events).
    pub async fn follow(&self, run_id: &str) -> Result<Events> {
        let r = self
            .request(Method::GET, &format!("/v1/runs/{run_id}/events"))
            .send()
            .await?;
        if !r.status().is_success() {
            let status = r.status();
            return Err(ClientError::Api {
                status,
                message: r.text().await.unwrap_or_default(),
            });
        }
        Ok(events(r))
    }

    /// A run as stored: status, agent, timings, cost.
    pub async fn get_run(&self, run_id: &str) -> Result<Value> {
        self.get(&format!("/v1/runs/{run_id}")).await
    }

    pub async fn cancel(&self, run_id: &str) -> Result<Value> {
        self.post(&format!("/v1/runs/{run_id}/cancel"), &json!({}))
            .await
    }

    /// Tool calls waiting for a person.
    pub async fn approvals(&self) -> Result<Vec<PendingApproval>> {
        self.get("/v1/approvals").await
    }

    /// Approve or deny a waiting tool call.
    pub async fn decide(&self, run_id: &str, call_id: &str, decision: &Decision) -> Result<Value> {
        self.post(&format!("/v1/runs/{run_id}/approvals/{call_id}"), decision)
            .await
    }
}

fn body(opts: &RunOptions, text: &str, mode: &str) -> Value {
    let mut b = serde_json::to_value(opts).expect("options serialize");
    b["input"] = json!(text);
    b["mode"] = json!(mode);
    b
}

fn events(r: reqwest::Response) -> Events {
    use eventsource_stream::Eventsource;
    r.bytes_stream()
        .eventsource()
        .filter_map(|e| async move {
            match e {
                Ok(ev) if ev.data.trim().is_empty() => None,
                Ok(ev) => Some(
                    serde_json::from_str::<EventEnvelope>(&ev.data)
                        .map_err(|e| ClientError::Decode(format!("{e}: {}", ev.data))),
                ),
                Err(e) => Some(Err(ClientError::Decode(e.to_string()))),
            }
        })
        .boxed()
}
