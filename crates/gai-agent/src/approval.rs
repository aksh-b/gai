use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{broadcast, oneshot};

/// A human's answer to a tool call that needs approval.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Decision {
    pub approved: bool,
    /// Shown to the model when the call is rejected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Run the tool with this input instead of the model's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
}

impl Decision {
    pub fn approve() -> Self {
        Self {
            approved: true,
            ..Default::default()
        }
    }

    pub fn reject(reason: impl Into<String>) -> Self {
        Self {
            approved: false,
            reason: Some(reason.into()),
            input: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PendingApproval {
    pub run_id: String,
    pub call_id: String,
    pub tool: String,
    pub input: Value,
}

/// `(run_id, call_id)`
type CallKey = (String, String);

/// Tool calls waiting on a human, keyed by run and call.
pub struct ApprovalHub {
    pending: Mutex<HashMap<CallKey, (PendingApproval, oneshot::Sender<Decision>)>>,
    requested: broadcast::Sender<PendingApproval>,
}

impl Default for ApprovalHub {
    fn default() -> Self {
        Self {
            pending: Mutex::default(),
            requested: broadcast::channel(256).0,
        }
    }
}

impl ApprovalHub {
    /// Hear about every call that starts waiting from now on.
    pub fn watch(&self) -> broadcast::Receiver<PendingApproval> {
        self.requested.subscribe()
    }

    /// Register a pending call; the receiver gets the decision.
    pub fn wait(&self, p: PendingApproval) -> oneshot::Receiver<Decision> {
        let (tx, rx) = oneshot::channel();
        let _ = self.requested.send(p.clone());
        self.pending
            .lock()
            .expect("approval lock")
            .insert((p.run_id.clone(), p.call_id.clone()), (p, tx));
        rx
    }

    pub fn forget(&self, run_id: &str, call_id: &str) {
        self.pending
            .lock()
            .expect("approval lock")
            .remove(&(run_id.to_owned(), call_id.to_owned()));
    }

    /// Answer a pending call. Errors when nothing is waiting on it.
    pub fn resolve(&self, run_id: &str, call_id: &str, decision: Decision) -> Result<(), String> {
        let entry = self
            .pending
            .lock()
            .expect("approval lock")
            .remove(&(run_id.to_owned(), call_id.to_owned()));
        match entry {
            Some((_, tx)) => tx
                .send(decision)
                .map_err(|_| "the run stopped waiting".to_owned()),
            None => Err(format!(
                "no pending approval for call {call_id} in run {run_id}"
            )),
        }
    }

    pub fn pending(&self, run_id: Option<&str>) -> Vec<PendingApproval> {
        let map = self.pending.lock().expect("approval lock");
        let mut v: Vec<PendingApproval> = map
            .values()
            .filter(|(p, _)| run_id.is_none_or(|r| p.run_id == r))
            .map(|(p, _)| p.clone())
            .collect();
        v.sort_by(|a, b| (&a.run_id, &a.call_id).cmp(&(&b.run_id, &b.call_id)));
        v
    }
}
