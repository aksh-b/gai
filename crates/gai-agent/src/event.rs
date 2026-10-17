use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use gai_core::{Message, Usage};
use gai_store::{RunStatus, SharedStore};
use gai_tools::{ToolOutput, ToolPolicy};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;

/// Something that happened during a run. The SSE endpoint, the TUI, the
/// trace store and SDK callers all consume this one stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    RunStarted {
        agent: String,
        agent_version: Option<u32>,
        model: String,
        session_id: String,
    },
    StepStarted {
        step: u32,
    },
    /// A `router/*` model picked a real model for this step.
    ModelRouted {
        step: u32,
        model: String,
        reason: String,
    },
    /// A fork took this step from the run it was forked from instead of
    /// calling the model.
    StepReplayed {
        step: u32,
        from_run_id: String,
    },
    TextDelta {
        text: String,
    },
    ReasoningDelta {
        text: String,
    },
    /// The model's complete turn, including any tool calls.
    AssistantMessage {
        message: Message,
    },
    ToolCall {
        id: String,
        name: String,
        input: Value,
        policy: ToolPolicy,
    },
    ApprovalRequired {
        id: String,
        name: String,
        input: Value,
    },
    ApprovalResolved {
        id: String,
        approved: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    ToolResult {
        id: String,
        name: String,
        output: ToolOutput,
        duration_ms: u64,
    },
    Usage {
        step: u32,
        model: String,
        usage: Usage,
        cost: Option<f64>,
        total_usage: Usage,
        total_cost: f64,
    },
    Compacted {
        summarized_messages: usize,
        summary_chars: usize,
    },
    ToolResultsCleared {
        cleared: usize,
    },
    Warning {
        message: String,
    },
    /// A workflow run began.
    WorkflowStarted {
        workflow: String,
        version: Option<u32>,
        inputs: Value,
    },
    NodeStarted {
        node: String,
        /// `agent`, `tool`, `transform`, `human`, or `map:<kind>`.
        kind: String,
    },
    NodeFinished {
        node: String,
        /// `completed`, `failed` or `skipped`.
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        duration_ms: u64,
    },
    RunFinished {
        status: RunStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        usage: Usage,
        cost: f64,
        steps: u32,
    },
}

impl RunEvent {
    /// Deltas are live-only; everything else is stored, so a client that
    /// reconnects can catch up at message granularity.
    pub fn persisted(&self) -> bool {
        !matches!(self, Self::TextDelta { .. } | Self::ReasoningDelta { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EventEnvelope {
    pub run_id: String,
    pub seq: u64,
    pub ts: DateTime<Utc>,
    #[serde(flatten)]
    pub event: RunEvent,
}

/// Numbers, broadcasts and stores a run's events.
#[derive(Clone)]
pub(crate) struct Emitter {
    run_id: String,
    seq: Arc<AtomicU64>,
    tx: broadcast::Sender<EventEnvelope>,
    store: SharedStore,
}

impl Emitter {
    pub(crate) fn new(
        run_id: &str,
        tx: broadcast::Sender<EventEnvelope>,
        store: SharedStore,
        start_seq: u64,
    ) -> Self {
        Self {
            run_id: run_id.to_owned(),
            seq: Arc::new(AtomicU64::new(start_seq)),
            tx,
            store,
        }
    }

    pub(crate) async fn emit(&self, event: RunEvent) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let env = EventEnvelope {
            run_id: self.run_id.clone(),
            seq,
            ts: Utc::now(),
            event,
        };
        if env.event.persisted() {
            let value = serde_json::to_value(&env).expect("serializable event");
            if let Err(e) = self.store.append_event(&self.run_id, seq, &value).await {
                tracing::warn!(run = %self.run_id, error = %e, "could not store run event");
            }
        }
        let _ = self.tx.send(env);
    }
}
