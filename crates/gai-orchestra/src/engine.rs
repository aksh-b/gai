//! Running workflows.
//!
//! A workflow run is an ordinary run record (`metadata.kind = "workflow"`)
//! whose steps are its nodes. Each node's result is journaled when it ends,
//! so a run interrupted by a restart picks up where it was: finished nodes
//! are not repeated, and agent nodes find their child runs again instead of
//! starting new ones.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use futures::StreamExt;
use gai_agent::{
    AgentError, Decision, EventEnvelope, ExternalRun, PendingApproval, RunEvent, RunOutcome,
    RunRequest, Runtime,
};
use gai_core::Usage;
use gai_store::{
    NewRun, NewSession, Run, RunFilter, RunStatus, RunUpdate, StepKind, StepStatus, StoreError,
};
use gai_tools::{ToolCtx, ToolPolicy};
use serde_json::{Map, Value, json};
use tokio::sync::broadcast;
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

use crate::spec::{AgentRef, NodeKind, NodeSpec, WorkflowSpec};
use crate::tmpl;

/// `metadata.kind` of workflow runs.
pub const KIND: &str = "workflow";

#[derive(Debug, thiserror::Error)]
pub enum WorkflowError {
    #[error("invalid workflow: {0}")]
    Invalid(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error(transparent)]
    Agent(#[from] AgentError),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// What to run.
#[derive(Clone, Debug)]
pub struct WorkflowRequest {
    pub spec: WorkflowSpec,
    /// Stored version, recorded on the run.
    pub version: Option<u32>,
    pub inputs: Map<String, Value>,
    /// Passed to agent nodes (user-scoped memory).
    pub user_id: Option<String>,
    /// Run `approve`-policy tools without asking (agent and tool nodes).
    pub auto_approve: bool,
    /// Caller data recorded on the run (e.g. `key_id`).
    pub metadata: Map<String, Value>,
    pub callback_url: Option<String>,
}

impl WorkflowRequest {
    pub fn new(spec: WorkflowSpec, inputs: Map<String, Value>) -> Self {
        Self {
            spec,
            version: None,
            inputs,
            user_id: None,
            auto_approve: false,
            metadata: Map::new(),
            callback_url: None,
        }
    }
}

pub struct WorkflowHandle {
    pub run_id: String,
    pub session_id: String,
    pub events: broadcast::Receiver<EventEnvelope>,
    join: JoinHandle<RunOutcome>,
}

impl WorkflowHandle {
    pub async fn next_event(&mut self) -> Option<EventEnvelope> {
        loop {
            match self.events.recv().await {
                Ok(e) => return Some(e),
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }

    pub async fn wait(self) -> RunOutcome {
        self.join.await.expect("workflow task panicked")
    }

    /// Stop the run's task on the spot, as a crash would (for tests).
    pub fn abort(self) {
        self.join.abort();
    }
}

/// Workflow storage and execution over a [`Runtime`]. Cheap to clone.
#[derive(Clone)]
pub struct Workflows {
    rt: Runtime,
    dirs: Arc<Vec<PathBuf>>,
}

impl Workflows {
    /// Workflows over `rt`, with workflow files in `dirs`. Registers itself
    /// with the runtime, so queued and interrupted workflow runs are picked
    /// up by `start_queue` and `recover`.
    pub fn new(rt: Runtime, dirs: Vec<PathBuf>) -> Self {
        let w = Self {
            rt: rt.clone(),
            dirs: Arc::new(dirs),
        };
        let engine = w.clone();
        rt.set_launcher(
            KIND,
            Arc::new(move |run: Run, _fresh: bool| {
                let engine = engine.clone();
                Box::pin(async move {
                    if let Err(e) = engine.relaunch(run.clone()).await {
                        tracing::warn!(run = %run.id, error = %e, "could not continue workflow run");
                        let update = RunUpdate {
                            status: Some(RunStatus::Failed),
                            error: Some(format!("could not continue: {e}")),
                            ..Default::default()
                        };
                        let _ = engine.rt.store().update_run(&run.id, update).await;
                    }
                })
            }),
        );
        w
    }

    pub fn runtime(&self) -> &Runtime {
        &self.rt
    }

    pub fn dirs(&self) -> &[PathBuf] {
        &self.dirs
    }

    /// Save a workflow as its next version.
    pub async fn save(
        &self,
        spec: &WorkflowSpec,
        note: Option<&str>,
    ) -> Result<u32, WorkflowError> {
        if spec.name.is_empty() {
            return Err(WorkflowError::Invalid(
                "a stored workflow needs a name".into(),
            ));
        }
        spec.graph().map_err(WorkflowError::Invalid)?;
        let value = serde_json::to_value(spec).expect("serializable spec");
        Ok(self
            .rt
            .store()
            .put_workflow(&spec.name, &value, note)
            .await?)
    }

    /// A stored workflow (latest, or a version).
    pub async fn load(
        &self,
        name: &str,
        version: Option<u32>,
    ) -> Result<(WorkflowSpec, u32), WorkflowError> {
        let v = self
            .rt
            .store()
            .get_workflow(name, version)
            .await?
            .ok_or_else(|| WorkflowError::NotFound(format!("workflow {name}")))?;
        let spec: WorkflowSpec = serde_json::from_value(v.spec)
            .map_err(|e| WorkflowError::Invalid(format!("stored workflow {name}: {e}")))?;
        Ok((spec, v.version))
    }

    /// Workflow files in the workflow folders, by name.
    pub fn files(&self) -> Vec<(String, PathBuf)> {
        let mut out = Vec::new();
        for dir in self.dirs.iter() {
            let Ok(rd) = std::fs::read_dir(dir) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if matches!(
                    p.extension().and_then(|x| x.to_str()),
                    Some("yaml" | "yml" | "toml" | "json")
                ) && let Some(stem) = p.file_stem().and_then(|s| s.to_str())
                {
                    out.push((stem.to_owned(), p));
                }
            }
        }
        out.sort();
        out
    }

    /// Find a workflow: a file path, a stored workflow (`name`, `name@v3`
    /// or `name@<label>`), or `<name>.yaml` in a workflow folder.
    pub async fn resolve(
        &self,
        reference: &str,
    ) -> Result<(WorkflowSpec, Option<u32>), WorkflowError> {
        let path = Path::new(reference);
        if path.is_file() {
            return Ok((
                WorkflowSpec::load(path).map_err(WorkflowError::Invalid)?,
                None,
            ));
        }
        let (name, version) = match reference.split_once('@') {
            None => (reference, None),
            Some((n, sel)) => match sel.strip_prefix('v').and_then(|v| v.parse::<u32>().ok()) {
                Some(v) => (n, Some(v)),
                None => {
                    let v = self
                        .rt
                        .store()
                        .list_workflows()
                        .await?
                        .into_iter()
                        .find(|w| w.name == n)
                        .and_then(|w| w.labels.get(sel).copied())
                        .ok_or_else(|| {
                            WorkflowError::NotFound(format!("workflow {n} has no label {sel:?}"))
                        })?;
                    (n, Some(v))
                }
            },
        };
        match self.load(name, version).await {
            Ok((spec, v)) => return Ok((spec, Some(v))),
            Err(WorkflowError::NotFound(_)) if version.is_none() => {}
            Err(e) => return Err(e),
        }
        if let Some((_, p)) = self.files().into_iter().find(|(n, _)| n == name) {
            return Ok((
                WorkflowSpec::load(&p).map_err(WorkflowError::Invalid)?,
                None,
            ));
        }
        Err(WorkflowError::NotFound(format!("workflow {reference}")))
    }

    /// Validate a request and record its run.
    async fn create(&self, req: WorkflowRequest, background: bool) -> Result<Run, WorkflowError> {
        let spec = req.spec;
        spec.graph().map_err(WorkflowError::Invalid)?;
        let inputs = spec
            .resolve_inputs(&req.inputs)
            .map_err(WorkflowError::Invalid)?;
        if let Some(url) = &req.callback_url {
            self.rt.check_callback_url(url)?;
        }
        let name = if spec.name.is_empty() {
            "inline".to_owned()
        } else {
            spec.name.clone()
        };
        let store = self.rt.store();
        let session = store
            .create_session(NewSession {
                agent: Some(format!("workflow:{name}")),
                title: Some(spec.description.clone().unwrap_or_else(|| name.clone())),
                user_id: req.user_id.clone(),
                metadata: Value::Null,
            })
            .await?;
        let mut metadata = req.metadata;
        metadata.insert("kind".into(), json!(KIND));
        metadata.insert(
            "workflow".into(),
            serde_json::to_value(&spec).expect("serializable"),
        );
        metadata.insert("version".into(), json!(req.version));
        metadata.insert("inputs".into(), Value::Object(inputs.clone()));
        metadata.insert(
            "user_id".into(),
            json!(req.user_id.unwrap_or_else(|| "default".into())),
        );
        metadata.insert("auto_approve".into(), json!(req.auto_approve));
        if let Some(url) = req.callback_url {
            metadata.insert("callback_url".into(), json!(url));
        }
        if background {
            metadata.insert("background".into(), json!(true));
        }
        Ok(store
            .create_run(NewRun {
                id: None,
                session_id: session.id,
                agent: format!("workflow:{name}"),
                agent_version: req.version,
                parent_run_id: None,
                input: Value::Object(inputs),
                metadata: Value::Object(metadata),
            })
            .await?)
    }

    /// Start a workflow run and return a handle to its events and outcome.
    pub async fn start(&self, req: WorkflowRequest) -> Result<WorkflowHandle, WorkflowError> {
        let run = self.create(req, false).await?;
        let ext = self.rt.attach(&run.id).await?;
        let events = self.rt.subscribe(&run.id).expect("attached run is active");
        let exec = Exec::new(&self.rt, &run, ext)?;
        Ok(WorkflowHandle {
            run_id: run.id.clone(),
            session_id: run.session_id.clone(),
            events,
            join: tokio::spawn(exec.execute()),
        })
    }

    /// Run a workflow to completion.
    pub async fn run(&self, req: WorkflowRequest) -> Result<RunOutcome, WorkflowError> {
        Ok(self.start(req).await?.wait().await)
    }

    /// Queue a workflow run for the background executor. Returns the run
    /// and session ids.
    pub async fn enqueue(&self, req: WorkflowRequest) -> Result<(String, String), WorkflowError> {
        let run = self.create(req, true).await?;
        self.rt.wake_queue();
        Ok((run.id, run.session_id))
    }

    /// Continue a recorded workflow run (after a restart, or from the queue).
    async fn relaunch(&self, run: Run) -> Result<(), WorkflowError> {
        let ext = self.rt.attach(&run.id).await?;
        let exec = Exec::new(&self.rt, &run, ext)?;
        exec.execute().await;
        Ok(())
    }
}

/// The state of one workflow run while it executes.
struct Exec {
    rt: Runtime,
    run_id: String,
    session_id: String,
    name: String,
    version: Option<u32>,
    spec: WorkflowSpec,
    inputs: Map<String, Value>,
    auto_approve: bool,
    workspace: PathBuf,
    ext: ExternalRun,
    journal: AtomicU32,
    waiting: AtomicUsize,
}

type NodeResult = (String, u32, Result<Value, String>, u64);

impl Exec {
    fn new(rt: &Runtime, run: &Run, ext: ExternalRun) -> Result<Arc<Self>, WorkflowError> {
        let m = &run.metadata;
        let spec: WorkflowSpec = serde_json::from_value(m["workflow"].clone())
            .map_err(|e| WorkflowError::Invalid(format!("run {}: {e}", run.id)))?;
        Ok(Arc::new(Self {
            rt: rt.clone(),
            run_id: run.id.clone(),
            session_id: run.session_id.clone(),
            name: run
                .agent
                .strip_prefix("workflow:")
                .unwrap_or(&run.agent)
                .to_owned(),
            version: run.agent_version,
            inputs: m["inputs"].as_object().cloned().unwrap_or_default(),
            auto_approve: m["auto_approve"].as_bool().unwrap_or(false),
            workspace: rt
                .config()
                .data_dir
                .join("workspaces")
                .join(&run.session_id),
            spec,
            ext,
            journal: AtomicU32::new(0),
            waiting: AtomicUsize::new(0),
        }))
    }

    fn store(&self) -> &gai_store::SharedStore {
        self.rt.store()
    }

    async fn set_status(&self, status: RunStatus) {
        let r = self
            .store()
            .update_run(
                &self.run_id,
                RunUpdate {
                    status: Some(status),
                    ..Default::default()
                },
            )
            .await;
        if let Err(e) = r {
            tracing::warn!(run = %self.run_id, error = %e, "store write failed");
        }
    }

    fn context(&self, done: &HashMap<String, (String, Value)>) -> Value {
        let nodes: Map<String, Value> = self
            .spec
            .nodes
            .iter()
            .map(|n| {
                let (status, output) = done
                    .get(&n.id)
                    .cloned()
                    .unwrap_or_else(|| ("pending".into(), Value::Null));
                (n.id.clone(), json!({"status": status, "output": output}))
            })
            .collect();
        json!({"workflow": self.name, "inputs": self.inputs, "nodes": nodes})
    }

    async fn execute(self: Arc<Self>) -> RunOutcome {
        let store = self.store().clone();
        let resumed = !store
            .list_events(&self.run_id, None, 1)
            .await
            .unwrap_or_default()
            .is_empty();
        self.set_status(RunStatus::Running).await;
        if resumed {
            self.ext
                .emit(RunEvent::Warning {
                    message: "resumed after the server restarted".into(),
                })
                .await;
        } else {
            self.ext
                .emit(RunEvent::WorkflowStarted {
                    workflow: self.name.clone(),
                    version: self.version,
                    inputs: Value::Object(self.inputs.clone()),
                })
                .await;
        }
        let _ = std::fs::create_dir_all(&self.workspace);

        // Checkpoints: nodes that already ended keep their results.
        let mut done: HashMap<String, (String, Value)> = HashMap::new();
        let steps = store.list_steps(&self.run_id).await.unwrap_or_default();
        self.journal.store(
            steps.iter().map(|s| s.seq).max().unwrap_or(0),
            Ordering::SeqCst,
        );
        for s in &steps {
            if s.kind == StepKind::Node
                && s.status != StepStatus::Started
                && let (Some(id), Some(out)) = (&s.name, &s.output)
            {
                let status = out["status"].as_str().unwrap_or("completed").to_owned();
                done.insert(id.clone(), (status, out["output"].clone()));
            }
        }

        let graph = match self.spec.graph() {
            Ok(g) => g,
            Err(e) => return self.finish(RunStatus::Failed, None, Some(e), &done).await,
        };
        let cancel = self.ext.cancel.clone();
        let nodes_cancel = cancel.child_token();
        let timeout = self.spec.limits.timeout_secs;
        let deadline = async move {
            match timeout {
                Some(s) => tokio::time::sleep(Duration::from_secs(s)).await,
                None => std::future::pending().await,
            }
        };
        tokio::pin!(deadline);
        let mut running: JoinSet<NodeResult> = JoinSet::new();
        let mut started: std::collections::HashSet<String> = done.keys().cloned().collect();
        let mut failure: Option<String> = None;
        let mut timed_out = false;

        'outer: loop {
            // Start (or skip) every node whose dependencies are done.
            let mut progressed = true;
            while progressed {
                progressed = false;
                for g in &graph {
                    if started.contains(&g.id)
                        || running.len() >= self.spec.limits.max_concurrency
                        || !g.deps.iter().all(|d| done.contains_key(d))
                    {
                        continue;
                    }
                    let node = self
                        .spec
                        .nodes
                        .iter()
                        .find(|n| n.id == g.id)
                        .expect("graph node")
                        .clone();
                    let ctx = self.context(&done);
                    // Only a node whose every dependency was skipped is skipped
                    // by default, so a node joining two branches still runs.
                    let all_skipped =
                        !g.deps.is_empty() && g.deps.iter().all(|d| done[d].0 == "skipped");
                    let go = match &node.condition {
                        Some(c) => tmpl::truthy(c, &ctx),
                        None => Ok(!all_skipped),
                    };
                    started.insert(g.id.clone());
                    progressed = true;
                    let kind = if node.map.is_some() {
                        format!("map:{}", g.kind.as_str())
                    } else {
                        g.kind.as_str().to_owned()
                    };
                    match go {
                        Ok(false) => {
                            let seq = self.begin_node(&node.id, &kind).await;
                            self.end_node(&node.id, seq, Ok(Value::Null), 0, true).await;
                            done.insert(node.id.clone(), ("skipped".into(), Value::Null));
                        }
                        Ok(true) => {
                            let seq = self.begin_node(&node.id, &kind).await;
                            let (me, tok) = (self.clone(), nodes_cancel.clone());
                            running.spawn(async move {
                                let t = Instant::now();
                                let r = me.run_node(&node, ctx, &tok).await;
                                (node.id.clone(), seq, r, t.elapsed().as_millis() as u64)
                            });
                        }
                        Err(e) => {
                            let seq = self.begin_node(&node.id, &kind).await;
                            self.end_node(&node.id, seq, Err(e.clone()), 0, false).await;
                            done.insert(node.id.clone(), ("failed".into(), Value::Null));
                            if !node.continue_on_error {
                                failure = Some(format!("node {} failed: {e}", node.id));
                                break 'outer;
                            }
                        }
                    }
                }
            }
            if running.is_empty() {
                break;
            }
            tokio::select! {
                Some(joined) = running.join_next() => {
                    let Ok((id, seq, result, ms)) = joined else { continue };
                    let node = self.spec.nodes.iter().find(|n| n.id == id).expect("known node");
                    let failed = result.is_err();
                    let err = result.as_ref().err().cloned();
                    let output = result.as_ref().ok().cloned().unwrap_or(Value::Null);
                    self.end_node(&id, seq, result, ms, false).await;
                    done.insert(id.clone(), (if failed { "failed" } else { "completed" }.into(), output));
                    if failed && !node.continue_on_error && !nodes_cancel.is_cancelled() {
                        failure = Some(format!("node {id} failed: {}", err.unwrap_or_default()));
                        break;
                    }
                }
                _ = cancel.cancelled() => break,
                _ = &mut deadline => { timed_out = true; break; }
            }
        }

        // Stop what is still running, letting child runs wind down.
        nodes_cancel.cancel();
        let drain = async {
            while let Some(joined) = running.join_next().await {
                if let Ok((id, seq, result, ms)) = joined {
                    self.end_node(&id, seq, result, ms, false).await;
                }
            }
        };
        if tokio::time::timeout(Duration::from_secs(30), drain)
            .await
            .is_err()
        {
            running.abort_all();
        }

        let (status, error) = if let Some(f) = failure {
            (RunStatus::Failed, Some(f))
        } else if timed_out {
            (
                RunStatus::BudgetExceeded,
                Some(format!(
                    "time limit of {}s reached",
                    timeout.unwrap_or_default()
                )),
            )
        } else if cancel.is_cancelled() {
            (RunStatus::Cancelled, None)
        } else {
            (RunStatus::Completed, None)
        };
        let output = if status == RunStatus::Completed {
            let ctx = self.context(&done);
            match &self.spec.output {
                Some(o) => match tmpl::fill(o, &ctx) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        return self
                            .finish(RunStatus::Failed, None, Some(format!("output: {e}")), &done)
                            .await;
                    }
                },
                None => self.spec.nodes.iter().rev().find_map(|n| {
                    done.get(&n.id)
                        .filter(|(s, _)| s == "completed")
                        .map(|(_, v)| v.clone())
                }),
            }
        } else {
            None
        };
        self.finish(status, output, error, &done).await
    }

    async fn begin_node(&self, id: &str, kind: &str) -> u32 {
        let seq = self.journal.fetch_add(1, Ordering::SeqCst) + 1;
        let r = self
            .store()
            .start_step(
                &self.run_id,
                seq,
                StepKind::Node,
                Some(id),
                Some(&json!({"kind": kind})),
            )
            .await;
        if let Err(e) = r {
            tracing::warn!(run = %self.run_id, error = %e, "store write failed");
        }
        self.ext
            .emit(RunEvent::NodeStarted {
                node: id.to_owned(),
                kind: kind.to_owned(),
            })
            .await;
        seq
    }

    async fn end_node(
        &self,
        id: &str,
        seq: u32,
        result: Result<Value, String>,
        ms: u64,
        skipped: bool,
    ) {
        let (status, output, error) = match result {
            _ if skipped => ("skipped", Value::Null, None),
            Ok(v) => ("completed", v, None),
            Err(e) => ("failed", Value::Null, Some(e)),
        };
        let r = self
            .store()
            .finish_step(
                &self.run_id,
                seq,
                if status == "failed" {
                    StepStatus::Failed
                } else {
                    StepStatus::Completed
                },
                Some(&json!({"status": status, "output": output, "error": error})),
                None,
                None,
            )
            .await;
        if let Err(e) = r {
            tracing::warn!(run = %self.run_id, error = %e, "store write failed");
        }
        self.ext
            .emit(RunEvent::NodeFinished {
                node: id.to_owned(),
                status: status.to_owned(),
                output: (!output.is_null()).then_some(output),
                error,
                duration_ms: ms,
            })
            .await;
    }

    async fn finish(
        &self,
        status: RunStatus,
        output: Option<Value>,
        error: Option<String>,
        done: &HashMap<String, (String, Value)>,
    ) -> RunOutcome {
        // Usage and cost are those of the agent runs the workflow started.
        let children = self
            .store()
            .list_runs(RunFilter {
                parent_run_id: Some(self.run_id.clone()),
                limit: Some(10_000),
                ..Default::default()
            })
            .await
            .unwrap_or_default();
        let mut usage = Usage::default();
        let mut cost = 0.0;
        for c in &children {
            usage += c.usage;
            cost += c.cost;
        }
        let steps = done.values().filter(|(s, _)| s != "skipped").count() as u32;
        let r = self
            .store()
            .update_run(
                &self.run_id,
                RunUpdate {
                    status: Some(status),
                    output: output.clone(),
                    error: error.clone(),
                    usage: Some(usage),
                    cost: Some(cost),
                    steps: Some(steps),
                },
            )
            .await;
        if let Err(e) = r {
            tracing::warn!(run = %self.run_id, error = %e, "store write failed");
        }
        self.ext
            .emit(RunEvent::RunFinished {
                status,
                output: output.clone(),
                error: error.clone(),
                usage,
                cost,
                steps,
            })
            .await;
        let outcome = RunOutcome {
            run_id: self.run_id.clone(),
            session_id: self.session_id.clone(),
            status,
            text: match &output {
                Some(Value::String(s)) => s.clone(),
                Some(v) => v.to_string(),
                None => String::new(),
            },
            output,
            error,
            usage,
            cost,
            steps,
        };
        self.rt.finished(&outcome).await;
        outcome
    }

    /// Run a node, retrying as configured.
    async fn run_node(
        &self,
        node: &NodeSpec,
        ctx: Value,
        cancel: &CancellationToken,
    ) -> Result<Value, String> {
        let mut attempt = 0;
        loop {
            let r = match &node.map {
                Some(expr) => self.run_map(node, expr, &ctx, attempt, cancel).await,
                None => self.action(node, &ctx, &node.id, attempt, cancel).await,
            };
            match r {
                Err(e) if attempt < node.retries && !cancel.is_cancelled() => {
                    attempt += 1;
                    self.ext
                        .emit(RunEvent::Warning {
                            message: format!(
                                "node {} failed ({e}); retrying ({attempt}/{})",
                                node.id, node.retries
                            ),
                        })
                        .await;
                }
                other => return other,
            }
        }
    }

    async fn run_map(
        &self,
        node: &NodeSpec,
        expr: &str,
        ctx: &Value,
        attempt: u32,
        cancel: &CancellationToken,
    ) -> Result<Value, String> {
        let items = match tmpl::eval(expr, ctx)? {
            Value::Array(a) => a,
            Value::Null => Vec::new(),
            other => {
                return Err(format!(
                    "map must give a list, got {}",
                    tmpl::render("{{ v }}", &json!({"v": other})).unwrap_or_default()
                ));
            }
        };
        let results: Vec<Result<Value, String>> =
            futures::stream::iter(items.into_iter().enumerate())
                .map(|(i, item)| {
                    let mut c = ctx.clone();
                    c["item"] = item;
                    c["index"] = json!(i);
                    let key = format!("{}[{i}]", node.id);
                    async move { self.action(node, &c, &key, attempt, cancel).await }
                })
                .buffered(node.concurrency.unwrap_or(4))
                .collect()
                .await;
        let mut out = Vec::with_capacity(results.len());
        for (i, r) in results.into_iter().enumerate() {
            out.push(r.map_err(|e| format!("item {i}: {e}"))?);
        }
        Ok(Value::Array(out))
    }

    /// Do a node's work once.
    async fn action(
        &self,
        node: &NodeSpec,
        ctx: &Value,
        key: &str,
        attempt: u32,
        cancel: &CancellationToken,
    ) -> Result<Value, String> {
        // A retry is new work, so it must not find the failed attempt's run.
        let key = if attempt > 0 {
            format!("{key}~{attempt}")
        } else {
            key.to_owned()
        };
        match node.kind()? {
            NodeKind::Template => Ok(Value::String(tmpl::render(
                node.template.as_deref().unwrap_or_default(),
                ctx,
            )?)),
            NodeKind::Value => tmpl::fill(node.value.as_ref().expect("value node"), ctx),
            NodeKind::Agent => {
                let (spec, version) = match node.agent.as_ref().expect("agent node") {
                    AgentRef::Name(r) => {
                        self.rt.resolve_agent(r).await.map_err(|e| e.to_string())?
                    }
                    AgentRef::Spec(s) => ((**s).clone(), None),
                };
                let input = tmpl::render(node.input.as_deref().unwrap_or_default(), ctx)?;
                if input.trim().is_empty() {
                    return Err("the agent's input rendered empty".into());
                }
                let mut req = RunRequest::new(spec, input);
                req.agent_version = version;
                req.metadata.insert("node".into(), json!(node.id));
                let o = self
                    .rt
                    .run_child(&self.run_id, &key, req, cancel)
                    .await
                    .map_err(|e| e.to_string())?;
                if o.status == RunStatus::Completed {
                    Ok(o.output.unwrap_or(Value::String(o.text)))
                } else {
                    Err(format!(
                        "agent run {} ended {}{}",
                        o.run_id,
                        o.status.as_str(),
                        o.error.map(|e| format!(": {e}")).unwrap_or_default()
                    ))
                }
            }
            NodeKind::Tool => {
                let name = node.tool.as_deref().expect("tool node");
                let tool = self
                    .rt
                    .tool(name)
                    .ok_or_else(|| format!("unknown tool {name}"))?;
                let mut args = tmpl::fill(node.args.as_ref().unwrap_or(&json!({})), ctx)?;
                if !args.is_object() {
                    return Err("tool args must be an object".into());
                }
                match tool.default_policy() {
                    ToolPolicy::Deny => return Err(format!("{name} may not be called")),
                    ToolPolicy::Approve if !self.auto_approve => {
                        let d = self.ask(&key, name, args.clone(), cancel).await?;
                        if !d.approved {
                            return Err(format!(
                                "the {name} call was declined{}",
                                d.reason.map(|r| format!(": {r}")).unwrap_or_default()
                            ));
                        }
                        if let Some(edited) = d.input {
                            args = edited;
                        }
                    }
                    _ => {}
                }
                let tctx = ToolCtx {
                    run_id: self.run_id.clone(),
                    call_id: key.clone(),
                    session_id: self.session_id.clone(),
                    agent: format!("workflow:{}", self.name),
                    workspace: self.workspace.clone(),
                    cancel: cancel.clone(),
                    http: self.rt.http().clone(),
                    settings: Arc::new(self.rt.config().tools.clone()),
                };
                let out = tokio::select! {
                    r = tool.call(&tctx, args) => r.map_err(|e| e.to_string())?,
                    _ = cancel.cancelled() => return Err("cancelled".into()),
                };
                let out = out.truncated(self.rt.config().tools.max_output_chars);
                let text = out.as_text();
                if out.is_error {
                    return Err(text);
                }
                Ok(serde_json::from_str::<Value>(text.trim()).unwrap_or(Value::String(text)))
            }
            NodeKind::Human => {
                let prompt = tmpl::render(node.human.as_deref().unwrap_or_default(), ctx)?;
                let d = self
                    .ask(&key, "human", json!({"prompt": prompt}), cancel)
                    .await?;
                if !d.approved {
                    return Err(format!(
                        "declined{}",
                        d.reason.map(|r| format!(": {r}")).unwrap_or_default()
                    ));
                }
                Ok(json!({"approved": true, "response": d.input, "reason": d.reason}))
            }
        }
    }

    /// Wait for a person in the approvals inbox.
    async fn ask(
        &self,
        call_id: &str,
        name: &str,
        input: Value,
        cancel: &CancellationToken,
    ) -> Result<Decision, String> {
        if self.waiting.fetch_add(1, Ordering::SeqCst) == 0 {
            self.set_status(RunStatus::AwaitingApproval).await;
        }
        let rx = self.rt.approvals().wait(PendingApproval {
            run_id: self.run_id.clone(),
            call_id: call_id.to_owned(),
            tool: name.to_owned(),
            input: input.clone(),
        });
        self.ext
            .emit(RunEvent::ApprovalRequired {
                id: call_id.to_owned(),
                name: name.to_owned(),
                input,
            })
            .await;
        let decision = tokio::select! {
            d = rx => d.unwrap_or_else(|_| Decision::reject("abandoned")),
            _ = cancel.cancelled() => {
                self.rt.approvals().forget(&self.run_id, call_id);
                Decision::reject("cancelled")
            }
        };
        self.ext
            .emit(RunEvent::ApprovalResolved {
                id: call_id.to_owned(),
                approved: decision.approved,
                reason: decision.reason.clone(),
            })
            .await;
        if self.waiting.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.set_status(RunStatus::Running).await;
        }
        if cancel.is_cancelled() {
            return Err("cancelled".into());
        }
        Ok(decision)
    }
}
