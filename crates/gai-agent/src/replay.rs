//! Replay: fork a run from one of its model steps.
//!
//! Every model step journals the full request and response, and every tool
//! step its input and output. A fork is a new run in a copy of the session
//! that takes steps before `from_step` from that recording, without calling
//! the model or any tool, and runs `from_step` onwards live. Overrides
//! change what the live part runs with (model, system prompt, tools, a
//! stored agent version), and step edits change what the recording says.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Mutex;

use gai_core::{ContentBlock, Usage};
use gai_llm::StopReason;
use gai_store::{NewSession, Run, StepKind, StepStatus};
use gai_tools::ToolOutput;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::approval::Decision;
use crate::multi::run_kind;
use crate::runtime::{AgentError, Mode, Resume, RunHandle, RunRequest, Runtime};
use crate::spec::AgentSpec;

/// How to fork a run.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ReplayRequest {
    /// The first model step (1-based) to run live. Earlier steps are
    /// replayed from the recording; 1 re-runs everything.
    pub from_step: u32,
    #[serde(default)]
    pub overrides: ReplayOverrides,
}

/// What the fork runs with instead of the original's settings.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ReplayOverrides {
    /// Model for the live steps (`provider/model` or an alias).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// System prompt template replacing the agent's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Tool list replacing the agent's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// Run this stored version of the agent instead of the original's
    /// snapshot ("re-run this with v13").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_version: Option<u32>,
    /// Treat every `approve` tool as `auto` (default: as the original did).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_approve: Option<bool>,
    /// Changes to replayed steps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub step_edits: Vec<StepEdit>,
}

/// A change to a replayed step (one before `from_step`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct StepEdit {
    pub step: u32,
    /// Replace everything the model said at this step, tool calls included.
    /// Tool calls with new ids run live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<ContentBlock>>,
    /// Replace the text of the model's reply, keeping its tool calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Replace the results of this step's tool calls, by call id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tool_results: BTreeMap<String, ToolOutput>,
}

/// Recorded on a fork as `metadata.fork`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ForkInfo {
    pub run_id: String,
    pub from_step: u32,
    #[serde(default)]
    pub overrides: ReplayOverrides,
}

/// A model step as the original run recorded it.
#[derive(Clone, Debug)]
pub(crate) struct RecordedStep {
    /// The model that produced it.
    pub model: Option<String>,
    pub content: Vec<ContentBlock>,
    pub stop_reason: StopReason,
    pub usage: Usage,
    /// The journaled output, kept as is so replayed steps match exactly.
    pub output: Value,
}

/// A tool call as the original run journaled it.
#[derive(Clone, Debug)]
pub(crate) struct RecordedTool {
    pub input: Value,
    pub output: Value,
    pub result: ToolOutput,
}

/// The recording a fork replays from.
#[derive(Debug)]
pub(crate) struct Replay {
    pub from_run: String,
    pub from_step: u32,
    pub steps: BTreeMap<u32, RecordedStep>,
    pub tools: HashMap<String, RecordedTool>,
    pub approvals: HashMap<String, Decision>,
    /// Summaries of the original's compactions, in order.
    pub compactions: Mutex<VecDeque<String>>,
}

impl Runtime {
    /// Fork a run: copy its session up to where the run began, replay its
    /// steps before `from_step` from the recording, and run the rest live.
    /// `metadata` is recorded on the new run (e.g. the calling API key).
    pub async fn replay(
        &self,
        run_id: &str,
        req: ReplayRequest,
        metadata: Map<String, Value>,
    ) -> Result<RunHandle, AgentError> {
        let store = self.store();
        let run = store
            .get_run(run_id)
            .await?
            .ok_or_else(|| AgentError::NotFound(format!("run {run_id}")))?;
        if let Some(kind) = run_kind(&run) {
            return Err(AgentError::Invalid(format!(
                "run {run_id} is a {kind} run; only agent runs can be replayed"
            )));
        }
        if self.is_active(run_id) {
            return Err(AgentError::Invalid(format!(
                "run {run_id} is still running; replay it once it stops"
            )));
        }
        let fork = ForkInfo {
            run_id: run.id.clone(),
            from_step: req.from_step,
            overrides: req.overrides,
        };
        let replay = self.load_replay(&fork).await?;
        let meta = run.metadata.as_object().cloned().unwrap_or_default();
        let o = &fork.overrides;

        let (mut spec, agent_version) = match o.agent_version {
            Some(v) => {
                let (spec, v) = self.load_agent(&run.agent, Some(v)).await?;
                (spec, Some(v))
            }
            None => {
                let spec: AgentSpec = meta
                    .get("spec")
                    .cloned()
                    .and_then(|v| serde_json::from_value(v).ok())
                    .ok_or_else(|| {
                        AgentError::Invalid(format!("run {run_id} has no agent snapshot"))
                    })?;
                (spec, run.agent_version)
            }
        };
        // The original's resolved model, so a default-model run forks onto
        // the same model even if the default has changed since.
        spec.model = o
            .model
            .clone()
            .or_else(|| {
                (o.agent_version.is_none())
                    .then(|| meta.get("model").and_then(Value::as_str).map(str::to_owned))
                    .flatten()
            })
            .or(spec.model);
        if let Some(s) = &o.system_prompt {
            spec.system = (!s.is_empty()).then(|| s.clone());
        }
        if let Some(t) = &o.tools {
            spec.tools = t.clone();
        }

        let input: Vec<ContentBlock> = serde_json::from_value(run.input.clone())
            .map_err(|e| AgentError::Invalid(format!("run {run_id} input: {e}")))?;
        let first_seq = store
            .list_messages(&run.session_id)
            .await?
            .iter()
            .find(|m| m.run_id.as_deref() == Some(run_id))
            .map(|m| m.seq);
        let original = store.get_session(&run.session_id).await?;
        let session = store
            .fork_session(
                &run.session_id,
                first_seq,
                NewSession {
                    agent: Some(spec.name.clone()),
                    title: Some(format!(
                        "Fork of {} from step {}",
                        original
                            .as_ref()
                            .and_then(|s| s.title.clone())
                            .unwrap_or_else(|| run.id.clone()),
                        fork.from_step
                    )),
                    user_id: original.and_then(|s| s.user_id),
                    metadata: json!({"fork_of_session": run.session_id, "fork_of_run": run.id}),
                },
            )
            .await?;

        let mut rr = RunRequest::with_input(spec, input.clone()).session(&session.id);
        rr.agent_version = agent_version;
        rr.vars = meta
            .get("vars")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        rr.user_id = meta
            .get("user_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        rr.auto_approve = o.auto_approve.unwrap_or_else(|| {
            meta.get("auto_approve")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        });
        rr.metadata = metadata;
        let mut extra = Map::new();
        extra.insert(
            "fork".into(),
            serde_json::to_value(&fork).expect("serializable fork"),
        );
        let launch = match self.create(rr, extra).await {
            Ok(l) => l,
            Err(e) => {
                let _ = store.delete_session(&session.id).await;
                return Err(e);
            }
        };
        Ok(self.launch(launch, Mode::Start(input), Resume::replaying(replay)))
    }

    /// Build what a fork replays from the original run's journal.
    pub(crate) async fn load_replay(&self, fork: &ForkInfo) -> Result<Replay, AgentError> {
        let store = self.store();
        let run: Run = store
            .get_run(&fork.run_id)
            .await?
            .ok_or_else(|| AgentError::NotFound(format!("run {}", fork.run_id)))?;
        let mut steps = BTreeMap::new();
        let mut tools = HashMap::new();
        let mut approvals = HashMap::new();
        let mut compactions = VecDeque::new();
        for st in store.list_steps(&run.id).await? {
            let input = st.input.clone().unwrap_or(Value::Null);
            match (st.kind, st.status) {
                (StepKind::Model, StepStatus::Completed) => {
                    let (Some(n), Some(output)) = (input["step"].as_u64(), st.output.clone())
                    else {
                        continue;
                    };
                    let content = serde_json::from_value(output["content"].clone())
                        .map_err(|e| AgentError::Invalid(format!("recorded step {n}: {e}")))?;
                    let stop_reason = serde_json::from_value(output["stop_reason"].clone())
                        .unwrap_or(StopReason::EndTurn);
                    // A step retried after a context overflow keeps its
                    // number; the last attempt is the one that counted.
                    steps.insert(
                        n as u32,
                        RecordedStep {
                            model: st.name.clone(),
                            content,
                            stop_reason,
                            usage: st.usage.unwrap_or_default(),
                            output,
                        },
                    );
                }
                (StepKind::Tool, StepStatus::Completed | StepStatus::Failed) => {
                    let (Some(id), Some(output)) = (input["call_id"].as_str(), st.output.clone())
                    else {
                        continue;
                    };
                    if let Ok(result) = serde_json::from_value::<ToolOutput>(output.clone()) {
                        tools.insert(
                            id.to_owned(),
                            RecordedTool {
                                input: input.clone(),
                                output,
                                result,
                            },
                        );
                    }
                }
                (StepKind::Approval, StepStatus::Completed) => {
                    if let (Some(id), Some(d)) = (
                        input["call_id"].as_str(),
                        st.output
                            .clone()
                            .and_then(|o| serde_json::from_value::<Decision>(o).ok()),
                    ) {
                        approvals.insert(id.to_owned(), d);
                    }
                }
                (StepKind::Compaction, StepStatus::Completed) => {
                    if let Some(s) = st.output.as_ref().and_then(Value::as_str) {
                        compactions.push_back(s.to_owned());
                    }
                }
                _ => {}
            }
        }

        let last = steps.keys().next_back().copied().unwrap_or(0);
        if fork.from_step == 0 || fork.from_step > last + 1 {
            return Err(AgentError::Invalid(format!(
                "from_step must be between 1 and {} (run {} recorded {last} model step{})",
                last + 1,
                run.id,
                if last == 1 { "" } else { "s" }
            )));
        }
        for edit in &fork.overrides.step_edits {
            if edit.step >= fork.from_step {
                return Err(AgentError::Invalid(format!(
                    "step {} runs live (from_step is {}); only replayed steps can be edited",
                    edit.step, fork.from_step
                )));
            }
            let rec = steps.get_mut(&edit.step).ok_or_else(|| {
                AgentError::Invalid(format!("run {} has no step {}", run.id, edit.step))
            })?;
            let mut changed = false;
            if let Some(c) = &edit.content {
                rec.content = c.clone();
                changed = true;
            }
            if let Some(t) = &edit.text {
                let mut content: Vec<ContentBlock> = rec
                    .content
                    .iter()
                    .filter(|b| !matches!(b, ContentBlock::Text { .. }))
                    .cloned()
                    .collect();
                if !t.is_empty() {
                    content.insert(0, ContentBlock::text(t.clone()));
                }
                rec.content = content;
                changed = true;
            }
            if changed {
                rec.output = json!({"stop_reason": rec.stop_reason, "content": rec.content});
            }
            for (id, result) in &edit.tool_results {
                let output = serde_json::to_value(result).expect("serializable");
                match tools.get_mut(id) {
                    Some(t) => {
                        t.output = output;
                        t.result = result.clone();
                    }
                    None => {
                        let input = rec
                            .content
                            .iter()
                            .find_map(|b| match b {
                                ContentBlock::ToolUse { id: i, input, .. } if i == id => {
                                    Some(input.clone())
                                }
                                _ => None,
                            })
                            .ok_or_else(|| {
                                AgentError::Invalid(format!(
                                    "step {} has no tool call {id}",
                                    edit.step
                                ))
                            })?;
                        tools.insert(
                            id.clone(),
                            RecordedTool {
                                input: json!({"call_id": id, "input": input}),
                                output,
                                result: result.clone(),
                            },
                        );
                    }
                }
            }
        }
        Ok(Replay {
            from_run: run.id,
            from_step: fork.from_step,
            steps,
            tools,
            approvals,
            compactions: Mutex::new(compactions),
        })
    }
}
