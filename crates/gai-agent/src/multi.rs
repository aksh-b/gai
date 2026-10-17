//! Runs that start other runs: delegate tools (an agent asks another agent
//! to do a task), child runs that survive restarts without being repeated,
//! and the hooks other engines (workflows) use to run under the runtime.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use gai_store::{Run, RunFilter, RunStatus};
use gai_tools::{SharedTool, ToolCtx, ToolError, ToolOutput, typed_tool};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::event::{Emitter, EventEnvelope, RunEvent};
use crate::runtime::{AgentError, RunOutcome, RunRequest, Runtime};
use crate::spec::AgentSpec;

/// How deep agents may delegate to agents (a workflow node counts as one).
pub const MAX_DEPTH: u64 = 5;

/// An agent another agent can hand tasks to.
///
/// ```yaml
/// delegates:
///   - researcher                     # a stored agent or agent file
///   - agent: writer@prod
///     name: draft                    # the tool becomes ask_draft
///     description: Writes a first draft from research notes.
///   - a2a: https://agents.example.com/a2a/travel   # a remote A2A agent
///     name: travel
///     headers: { Authorization: "Bearer ${TRAVEL_TOKEN}" }
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum DelegateRef {
    Agent(String),
    Spec(DelegateSpec),
    Remote(RemoteDelegate),
}

/// A remote agent reached over A2A.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoteDelegate {
    /// The agent's A2A endpoint, or its agent card URL.
    pub a2a: String,
    /// Tool name suffix: the tool becomes `ask_<name>`.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// HTTP headers; values may use `${VAR}`.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub headers: std::collections::BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DelegateSpec {
    /// Agent reference: `name`, `name@v3`, `name@label` or a file path.
    pub agent: String,
    /// Tool name suffix; defaults to the agent's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// When to use this delegate; defaults to the agent's description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl DelegateRef {
    /// The local agent delegated to, if it is one.
    pub fn spec(&self) -> Option<DelegateSpec> {
        match self {
            Self::Agent(a) => Some(DelegateSpec {
                agent: a.clone(),
                name: None,
                description: None,
            }),
            Self::Spec(s) => Some(s.clone()),
            Self::Remote(_) => None,
        }
    }
}

/// Starts a run of another kind (such as a workflow) from its stored
/// record: after a restart, or when the background queue picks it up. The
/// future ends when the run does. `fresh` is true for queued runs.
pub type RunLauncher = Arc<dyn Fn(Run, bool) -> BoxFuture<'static, ()> + Send + Sync>;

/// The kind of a run that another engine executes (`metadata.kind`).
pub fn run_kind(run: &Run) -> Option<&str> {
    run.metadata["kind"].as_str()
}

/// What a finished run looked like, rebuilt from its record.
pub fn outcome_of(run: &Run) -> RunOutcome {
    RunOutcome {
        run_id: run.id.clone(),
        session_id: run.session_id.clone(),
        status: run.status,
        text: match &run.output {
            Some(Value::String(s)) => s.clone(),
            Some(v) => v.to_string(),
            None => String::new(),
        },
        output: run.output.clone(),
        error: run.error.clone(),
        usage: run.usage,
        cost: run.cost,
        steps: run.steps,
    }
}

/// A run driven by something other than the agent loop (a workflow
/// engine). While it exists the run counts as active here: its events
/// stream to subscribers and it can be cancelled.
pub struct ExternalRun {
    rt: Runtime,
    run_id: String,
    emitter: Emitter,
    pub cancel: CancellationToken,
}

impl ExternalRun {
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub async fn emit(&self, event: RunEvent) {
        self.emitter.emit(event).await;
    }
}

impl Drop for ExternalRun {
    fn drop(&mut self) {
        self.rt.deregister_active(&self.run_id);
    }
}

#[derive(Deserialize, JsonSchema)]
struct DelegateArgs {
    /// The task, stated fully: the other agent sees nothing of this conversation.
    task: String,
    /// Background it needs: facts, constraints, the shape of answer you want.
    #[serde(default)]
    context: Option<String>,
}

/// A tool name made from a delegate's name.
pub fn delegate_tool_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(60)
        .collect();
    format!("ask_{clean}")
}

impl Runtime {
    /// The tool for a remote A2A delegate. Agent specs can come from API
    /// callers, so private addresses are refused unless allowed.
    fn remote_delegate(&self, r: &RemoteDelegate) -> Result<SharedTool, AgentError> {
        let u = reqwest::Url::parse(&r.a2a)
            .map_err(|e| AgentError::Invalid(format!("delegate {}: {e}", r.name)))?;
        if !matches!(u.scheme(), "http" | "https") {
            return Err(AgentError::Invalid(format!(
                "delegate {}: not an http(s) URL",
                r.name
            )));
        }
        if !self.config().tools.allow_private_network
            && gai_tools::builtin::is_private_host(
                u.host_str().unwrap_or_default(),
                u.port_or_known_default().unwrap_or(80),
            )
        {
            return Err(AgentError::Invalid(format!(
                "delegate {}: {} is a private network address (set tools.allow_private_network, or declare it under [a2a_agents])",
                r.name, r.a2a
            )));
        }
        let agent = gai_tools::a2a::A2aAgent::new(
            &r.name,
            gai_tools::a2a::A2aAgentConfig {
                url: r.a2a.clone(),
                headers: r.headers.clone(),
                description: r.description.clone(),
                timeout_secs: None,
            },
            self.http().clone(),
        );
        Ok(gai_tools::a2a::a2a_tool(
            agent,
            &delegate_tool_name(&r.name),
        ))
    }

    /// Find an agent by reference: a path to an agent file; a stored agent
    /// (`name`, `name@v3`, `name@<label>`); or `<name>.yaml|.yml|.toml|.json`
    /// in one of the agent folders. Returns the spec and, for stored agents,
    /// the version.
    pub async fn resolve_agent(
        &self,
        reference: &str,
    ) -> Result<(AgentSpec, Option<u32>), AgentError> {
        let path = Path::new(reference);
        if path.is_file() {
            return Ok((AgentSpec::load(path).map_err(AgentError::NotFound)?, None));
        }
        let (name, selector) = match reference.split_once('@') {
            Some((n, s)) => (n, Some(s)),
            None => (reference, None),
        };
        let version = match selector {
            None => None,
            Some(s) => match s.strip_prefix('v').and_then(|v| v.parse::<u32>().ok()) {
                Some(v) => Some(v),
                None => {
                    let agents = self.store().list_agents().await?;
                    let v = agents
                        .iter()
                        .find(|a| a.name == name)
                        .and_then(|a| a.labels.get(s).copied())
                        .ok_or_else(|| {
                            AgentError::NotFound(format!("agent {name} has no label {s:?}"))
                        })?;
                    Some(v)
                }
            },
        };
        match self.load_agent(name, version).await {
            Ok((spec, v)) => return Ok((spec, Some(v))),
            Err(AgentError::NotFound(_)) if version.is_none() => {}
            Err(e) => return Err(e),
        }
        let dirs: &[PathBuf] = &self.config().agents_dirs;
        for dir in dirs {
            for ext in ["yaml", "yml", "toml", "json"] {
                let p = dir.join(format!("{name}.{ext}"));
                if p.is_file() {
                    return Ok((AgentSpec::load(&p).map_err(AgentError::NotFound)?, None));
                }
            }
        }
        Err(AgentError::NotFound(format!(
            "no agent {reference:?}: not a file, not stored, and not in {}",
            dirs.iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )))
    }

    /// Wait for a run to end, wherever it runs in this process.
    pub async fn wait_for(&self, run_id: &str) -> Result<RunOutcome, AgentError> {
        loop {
            let run = self
                .store()
                .get_run(run_id)
                .await?
                .ok_or_else(|| AgentError::NotFound(format!("run {run_id}")))?;
            if run.status.is_terminal() {
                return Ok(outcome_of(&run));
            }
            match self.subscribe(run_id) {
                Some(mut rx) => loop {
                    match rx.recv().await {
                        Ok(EventEnvelope {
                            event: RunEvent::RunFinished { .. },
                            ..
                        })
                        | Err(broadcast::error::RecvError::Closed) => break,
                        _ => {}
                    }
                },
                None => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
    }

    /// Run `req` as a child of `parent_run_id`, identified by `key` (a tool
    /// call id or workflow node). If a child with that key already exists
    /// (the parent is being resumed after a restart), it is reused: a
    /// finished child's outcome is returned as is, an unfinished one is
    /// resumed or waited for. So a child's work is never done twice.
    ///
    /// The child inherits the parent's user, API key and auto-approval, and
    /// is cancelled when `cancel` fires.
    pub async fn run_child(
        &self,
        parent_run_id: &str,
        key: &str,
        mut req: RunRequest,
        cancel: &CancellationToken,
    ) -> Result<RunOutcome, AgentError> {
        let store = self.store();
        let existing = store
            .list_runs(RunFilter {
                parent_run_id: Some(parent_run_id.to_owned()),
                limit: Some(10_000),
                ..Default::default()
            })
            .await?
            .into_iter()
            .find(|r| r.metadata["child_key"] == json!(key));
        let child_id = match existing {
            Some(r) if r.status.is_terminal() => return Ok(outcome_of(&r)),
            Some(r) => {
                if !self.is_active(&r.id) {
                    match self.resume_boxed(r.id.clone()).await {
                        Ok(h) => {
                            tokio::spawn(h.wait());
                        }
                        // Someone else (startup recovery) got there first.
                        Err(_) if self.is_active(&r.id) => {}
                        Err(e) => return Err(e),
                    }
                }
                r.id
            }
            None => {
                let parent = store
                    .get_run(parent_run_id)
                    .await?
                    .ok_or_else(|| AgentError::NotFound(format!("run {parent_run_id}")))?;
                let pm = &parent.metadata;
                let depth = pm["depth"].as_u64().unwrap_or(0) + 1;
                if depth > MAX_DEPTH {
                    return Err(AgentError::Invalid(format!(
                        "delegation is nested more than {MAX_DEPTH} levels deep"
                    )));
                }
                req.parent_run_id = Some(parent_run_id.to_owned());
                req.metadata.insert("child_key".into(), json!(key));
                req.metadata.insert("depth".into(), json!(depth));
                if let Some(k) = pm["key_id"].as_str() {
                    req.metadata.insert("key_id".into(), json!(k));
                }
                if req.user_id.is_none() {
                    req.user_id = pm["user_id"].as_str().map(str::to_owned);
                }
                req.auto_approve |= pm["auto_approve"].as_bool().unwrap_or(false);
                let h = self.start_boxed(req).await?;
                let id = h.run_id.clone();
                tokio::spawn(h.wait());
                id
            }
        };
        tokio::select! {
            o = self.wait_for(&child_id) => o,
            _ = cancel.cancelled() => {
                self.cancel(&child_id);
                self.wait_for(&child_id).await
            }
        }
    }

    /// `ask_<name>` tools for an agent's delegates.
    pub(crate) async fn delegate_tools(
        &self,
        delegates: &[DelegateRef],
    ) -> Result<Vec<SharedTool>, AgentError> {
        let mut out: Vec<SharedTool> = Vec::new();
        for d in delegates {
            let d = match d {
                DelegateRef::Remote(r) => {
                    out.push(self.remote_delegate(r)?);
                    continue;
                }
                local => local.spec().expect("a local delegate"),
            };
            let (spec, version) = self.resolve_agent(&d.agent).await?;
            let tool_name = delegate_tool_name(d.name.as_deref().unwrap_or(&spec.name));
            let about = d
                .description
                .clone()
                .or_else(|| spec.description.clone())
                .unwrap_or_else(|| format!("The {} agent.", spec.name));
            let description = format!(
                "Hand a task to another agent and get its answer. {about} It works on its own with its own tools and sees nothing of this conversation, so state the task fully. Several calls run in parallel."
            );
            let rt = self.clone();
            let tool = typed_tool(
                &tool_name,
                &description,
                move |ctx: ToolCtx, a: DelegateArgs| {
                    let (rt, spec) = (rt.clone(), spec.clone());
                    async move {
                        let task = match a.context.filter(|c| !c.trim().is_empty()) {
                            Some(c) => format!("{}\n\n<context>\n{c}\n</context>", a.task),
                            None => a.task,
                        };
                        let mut req = RunRequest::new(spec, task);
                        req.agent_version = version;
                        let key = if ctx.call_id.is_empty() {
                            gai_core::new_id("call")
                        } else {
                            ctx.call_id.clone()
                        };
                        let o = rt
                            .run_child(&ctx.run_id, &key, req, &ctx.cancel)
                            .await
                            .map_err(|e| ToolError::Failed(e.to_string()))?;
                        if o.status == RunStatus::Completed {
                            Ok(ToolOutput::text(o.text))
                        } else {
                            Ok(ToolOutput::error(format!(
                                "The agent's run {} ended {}: {}{}",
                                o.run_id,
                                o.status.as_str(),
                                o.error.unwrap_or_default(),
                                if o.text.is_empty() {
                                    String::new()
                                } else {
                                    format!("\nIts last answer: {}", o.text)
                                }
                            )))
                        }
                    }
                },
            )
            // A repeat finds the child run by call id instead of starting another.
            .idempotent();
            if out.iter().any(|t| t.spec().name == tool_name) {
                return Err(AgentError::Invalid(format!(
                    "two delegates are both called {tool_name}; give one a `name`"
                )));
            }
            out.push(Arc::new(tool));
        }
        Ok(out)
    }

    // Starting a run can build delegate tools whose calls start runs: the
    // recursion needs a boxed, explicitly `Send` future to type-check.
    fn start_boxed(
        &self,
        req: RunRequest,
    ) -> BoxFuture<'static, Result<crate::RunHandle, AgentError>> {
        let rt = self.clone();
        Box::pin(async move { rt.start(req).await })
    }

    fn resume_boxed(&self, id: String) -> BoxFuture<'static, Result<crate::RunHandle, AgentError>> {
        let rt = self.clone();
        Box::pin(async move { rt.resume(&id).await })
    }

    /// Take over a recorded run from another engine: it becomes active here
    /// and its events continue after those already stored.
    pub async fn attach(&self, run_id: &str) -> Result<ExternalRun, AgentError> {
        let seq = self
            .store()
            .list_events(run_id, None, u32::MAX)
            .await?
            .last()
            .map(|e| e.seq)
            .unwrap_or(0);
        let (tx, _rx, cancel) = self.register_active(run_id);
        Ok(ExternalRun {
            rt: self.clone(),
            run_id: run_id.to_owned(),
            emitter: Emitter::new(run_id, tx, self.store().clone(), seq),
            cancel,
        })
    }
}
