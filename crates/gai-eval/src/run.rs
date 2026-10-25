//! Running an eval set against a target and keeping the results.

use std::path::Path;
use std::time::Instant;

use futures::StreamExt;
use gai_agent::{AgentSpec, RunRequest, Runtime};
use gai_store::{EvalResult, EvalRun, StepKind};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::score::{Evidence, Observed};
use crate::{EvalError, EvalSet};

/// Pass share used when a set names no threshold.
const DEFAULT_THRESHOLD: f64 = 0.8;

/// What to evaluate.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Target {
    /// An agent reference (`name`, `name@v3`, `name@prod`, a file);
    /// defaults to the set's `agent`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Run the agent on this model instead of its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct EvalOptions {
    /// Cases run at once.
    pub concurrency: usize,
    /// Model for `judge` and `grounded`, over the set's `judge_model`.
    pub judge_model: Option<String>,
    /// Only these case ids.
    pub cases: Vec<String>,
    /// Only cases with one of these tags.
    pub tags: Vec<String>,
}

impl Default for EvalOptions {
    fn default() -> Self {
        Self {
            concurrency: 4,
            judge_model: None,
            cases: Vec::new(),
            tags: Vec::new(),
        }
    }
}

/// Totals for an evaluation, stored as its `summary`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Summary {
    /// Mean case score, 0 to 1.
    pub score: f64,
    /// Share of cases that passed.
    pub pass_rate: f64,
    pub passed: u32,
    pub total: u32,
    /// Cases that could not run (the agent failed).
    pub errors: u32,
    /// What the agent runs cost (judging not included).
    pub cost: f64,
    pub mean_latency_ms: u64,
    pub p95_latency_ms: u64,
    /// The pass share needed, and whether it was reached.
    pub threshold: f64,
    pub ok: bool,
}

impl Summary {
    pub fn of(results: &[EvalResult], threshold: f64) -> Self {
        let total = results.len() as u32;
        if total == 0 {
            return Self {
                threshold,
                ..Default::default()
            };
        }
        let passed = results.iter().filter(|r| r.passed).count() as u32;
        let mut lat: Vec<u64> = results.iter().map(|r| r.latency_ms).collect();
        lat.sort_unstable();
        let p95 = lat[((lat.len() as f64 * 0.95).ceil() as usize)
            .saturating_sub(1)
            .min(lat.len() - 1)];
        let pass_rate = passed as f64 / total as f64;
        Self {
            score: results.iter().map(|r| r.score).sum::<f64>() / total as f64,
            pass_rate,
            passed,
            total,
            errors: results.iter().filter(|r| r.error.is_some()).count() as u32,
            cost: results.iter().map(|r| r.cost).sum(),
            mean_latency_ms: lat.iter().sum::<u64>() / total as u64,
            p95_latency_ms: p95,
            threshold,
            ok: pass_rate + 1e-9 >= threshold,
        }
    }
}

/// An evaluation with its results.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct EvalReport {
    pub run: EvalRun,
    pub summary: Summary,
    pub results: Vec<EvalResult>,
}

/// Progress while an evaluation runs.
#[derive(Clone, Debug)]
pub enum EvalProgress {
    Started { eval_run_id: String, total: usize },
    Case(EvalResult),
}

/// Called as an evaluation starts and as each case finishes.
pub type Progress = std::sync::Arc<dyn Fn(EvalProgress) + Send + Sync>;

/// Runs eval sets with a runtime and keeps the results in its store.
#[derive(Clone)]
pub struct Evaluator {
    rt: Runtime,
}

impl Evaluator {
    pub fn new(rt: Runtime) -> Self {
        Self { rt }
    }

    /// Save a set as its next version.
    pub async fn save(&self, set: &EvalSet, note: Option<&str>) -> Result<u32, EvalError> {
        let mut set = set.clone();
        set.normalize()?;
        if set.name.trim().is_empty() {
            return Err(EvalError::Invalid("a stored eval set needs a name".into()));
        }
        let spec = serde_json::to_value(&set).expect("serializable set");
        Ok(self.rt.store().put_eval_set(&set.name, &spec, note).await?)
    }

    /// A set by reference: a YAML file (saved, so the run can point at a
    /// version), or a stored set, `name` or `name@v2`.
    pub async fn load(&self, reference: &str) -> Result<(EvalSet, u32), EvalError> {
        let path = Path::new(reference);
        if path.is_file() {
            let text = std::fs::read_to_string(path)
                .map_err(|e| EvalError::Invalid(format!("{reference}: {e}")))?;
            let mut set = EvalSet::from_yaml(&text)?;
            if set.name.is_empty() {
                set.name = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
            }
            let v = self.save(&set, Some(&format!("from {reference}"))).await?;
            return Ok((set, v));
        }
        let (name, version) = match reference.split_once('@') {
            Some((n, v)) => (
                n,
                Some(
                    v.trim_start_matches('v')
                        .parse::<u32>()
                        .map_err(|_| EvalError::Invalid(format!("{reference}: use name@v2")))?,
                ),
            ),
            None => (reference, None),
        };
        let stored = self
            .rt
            .store()
            .get_eval_set(name, version)
            .await?
            .ok_or_else(|| EvalError::NotFound(format!("eval set {reference}")))?;
        let mut set: EvalSet = serde_json::from_value(stored.spec)
            .map_err(|e| EvalError::Invalid(format!("stored eval set {reference}: {e}")))?;
        set.normalize()?;
        Ok((set, stored.version))
    }

    /// Run a set against a target, storing each case's result as it
    /// finishes.
    pub async fn run(
        &self,
        reference: &str,
        target: &Target,
        opts: &EvalOptions,
        progress: Option<Progress>,
    ) -> Result<EvalReport, EvalError> {
        let (set, version) = self.load(reference).await?;
        let agent_ref = target
            .agent
            .clone()
            .or_else(|| set.agent.clone())
            .ok_or_else(|| {
                EvalError::Invalid(format!(
                    "eval set {} names no agent; choose one to evaluate",
                    set.name
                ))
            })?;
        let (mut spec, agent_version) = self.rt.resolve_agent(&agent_ref).await?;
        if let Some(m) = &target.model {
            spec.model = Some(m.clone());
        }
        let model = spec
            .model
            .clone()
            .or_else(|| self.rt.default_model())
            .unwrap_or_default();
        let judge_name = opts
            .judge_model
            .clone()
            .or_else(|| set.judge_model.clone())
            .or_else(|| self.rt.default_model());
        let cases: Vec<_> = set
            .cases
            .iter()
            .filter(|c| opts.cases.is_empty() || opts.cases.contains(&c.id))
            .filter(|c| opts.tags.is_empty() || c.tags.iter().any(|t| opts.tags.contains(t)))
            .cloned()
            .collect();
        if cases.is_empty() {
            return Err(EvalError::Invalid("no cases match".into()));
        }

        let store = self.rt.store();
        let run = store
            .create_eval_run(
                &set.name,
                version,
                &json!({"agent": spec.name, "agent_ref": agent_ref, "agent_version": agent_version, "model": model}),
            )
            .await?;
        if let Some(p) = &progress {
            p(EvalProgress::Started {
                eval_run_id: run.id.clone(),
                total: cases.len(),
            });
        }
        let judge = judge_name
            .as_deref()
            .and_then(|m| self.rt.models().chat(m).ok());
        // Owned futures (boxed), so the stream is Send for any caller.
        let jobs: Vec<futures::future::BoxFuture<'static, EvalResult>> = cases
            .iter()
            .map(|case| {
                let (this, spec, set, case) =
                    (self.clone(), spec.clone(), set.clone(), case.clone());
                let (judge, run_id, progress, store) = (
                    judge.clone(),
                    run.id.clone(),
                    progress.clone(),
                    store.clone(),
                );
                Box::pin(async move {
                    let r = this
                        .run_case(&run_id, &set, &case, &spec, agent_version, judge)
                        .await;
                    if let Err(e) = store.put_eval_result(&r).await {
                        tracing::warn!(error = %e, "could not store an eval result");
                    }
                    if let Some(p) = &progress {
                        p(EvalProgress::Case(r.clone()));
                    }
                    r
                }) as futures::future::BoxFuture<'static, EvalResult>
            })
            .collect();
        let mut results: Vec<EvalResult> = futures::stream::iter(jobs)
            .buffer_unordered(opts.concurrency.max(1))
            .collect()
            .await;
        let order: Vec<&str> = cases.iter().map(|c| c.id.as_str()).collect();
        results.sort_by_key(|r| order.iter().position(|id| *id == r.case_id));
        let summary = Summary::of(&results, set.threshold.unwrap_or(DEFAULT_THRESHOLD));
        let summary_json = serde_json::to_value(&summary).expect("summary");
        store
            .finish_eval_run(&run.id, "completed", &summary_json, None)
            .await?;
        let run = store.get_eval_run(&run.id).await?.unwrap_or(run);
        Ok(EvalReport {
            run,
            summary,
            results,
        })
    }

    async fn run_case(
        &self,
        eval_run_id: &str,
        set: &EvalSet,
        case: &crate::Case,
        spec: &AgentSpec,
        agent_version: Option<u32>,
        judge: Option<gai_llm::SharedChatModel>,
    ) -> EvalResult {
        let mut req = RunRequest::new(spec.clone(), case.input.clone());
        req.agent_version = agent_version;
        req.vars = case.vars.clone();
        // Nobody is there to approve tool calls.
        req.auto_approve = true;
        req.metadata.insert("kind".into(), json!("eval"));
        req.metadata.insert("eval_run".into(), json!(eval_run_id));
        req.metadata.insert("eval_case".into(), json!(case.id));
        let started = Instant::now();
        let outcome = self.rt.run(req).await;
        let latency_ms = started.elapsed().as_millis() as u64;
        let mut result = EvalResult {
            eval_run_id: eval_run_id.to_owned(),
            case_id: case.id.clone(),
            run_id: None,
            output: None,
            scores: json!([]),
            score: 0.0,
            passed: false,
            cost: 0.0,
            latency_ms,
            error: None,
        };
        let outcome = match outcome {
            Ok(o) => o,
            Err(e) => {
                result.error = Some(e.to_string());
                return result;
            }
        };
        result.run_id = Some(outcome.run_id.clone());
        result.output = Some(outcome.text.clone());
        result.cost = outcome.cost;
        if let Some(e) = &outcome.error {
            result.error = Some(e.clone());
        }
        let (tools, evidence) = self.tool_trail(&outcome.run_id).await;
        let observed = Observed {
            input: case.input.clone(),
            expected: case.expected.clone(),
            output: outcome.text.clone(),
            steps: outcome.steps,
            cost: outcome.cost,
            latency_ms,
            tools,
            evidence,
        };
        let scorers = set.scorers_for(case);
        let mut scores = Vec::new();
        for s in &scorers {
            let own = s.model().and_then(|m| self.rt.models().chat(m).ok());
            scores.push(s.score(&observed, own.as_ref().or(judge.as_ref())).await);
        }
        if scores.is_empty() {
            // Nothing to check: a case passes when the agent answered.
            result.score = if result.error.is_none() { 1.0 } else { 0.0 };
            result.passed = result.error.is_none();
        } else {
            result.score = scores.iter().map(|s| s.score).sum::<f64>() / scores.len() as f64;
            result.passed = result.error.is_none() && scores.iter().all(|s| s.passed);
        }
        result.scores = serde_json::to_value(&scores).expect("scores");
        result
    }

    /// Tools the run called, and what its retrieval tools returned.
    async fn tool_trail(&self, run_id: &str) -> (Vec<String>, Vec<Evidence>) {
        let steps = self.rt.store().list_steps(run_id).await.unwrap_or_default();
        let mut tools = Vec::new();
        let mut evidence = Vec::new();
        for s in steps.iter().filter(|s| s.kind == StepKind::Tool) {
            let Some(name) = s.name.clone() else { continue };
            let text = s.output.as_ref().map(tool_text).unwrap_or_default();
            match name.as_str() {
                "rag_search" | "memory_search" => evidence.extend(parse_hits(&text)),
                "http_fetch" | "browser_navigate" | "browser_read" | "fs_read" => {
                    let source = s
                        .input
                        .as_ref()
                        .and_then(|i| i["url"].as_str().or(i["path"].as_str()))
                        .map(str::to_owned)
                        .unwrap_or_else(|| name.clone());
                    evidence.push(Evidence {
                        source,
                        title: None,
                        text,
                    });
                }
                _ => {}
            }
            tools.push(name);
        }
        (tools, evidence)
    }

    /// An evaluation and its results, from the store.
    pub async fn report(&self, eval_run_id: &str) -> Result<EvalReport, EvalError> {
        let store = self.rt.store();
        let run = store
            .get_eval_run(eval_run_id)
            .await?
            .ok_or_else(|| EvalError::NotFound(format!("evaluation {eval_run_id}")))?;
        let results = store.list_eval_results(eval_run_id).await?;
        // A running evaluation has no stored totals yet: count what is done.
        let summary = match serde_json::from_value::<Summary>(run.summary.clone()) {
            Ok(s) if run.status != "running" => s,
            _ => {
                let threshold = match self
                    .rt
                    .store()
                    .get_eval_set(&run.eval_set, Some(run.eval_version))
                    .await?
                {
                    Some(v) => v.spec["threshold"].as_f64().unwrap_or(DEFAULT_THRESHOLD),
                    None => DEFAULT_THRESHOLD,
                };
                Summary::of(&results, threshold)
            }
        };
        Ok(EvalReport {
            run,
            summary,
            results,
        })
    }
}

/// The text of a stored tool output (`{content: [{type: text, text}]}`).
fn tool_text(v: &Value) -> String {
    match v["content"].as_array() {
        Some(blocks) => blocks
            .iter()
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        None => v
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| v.to_string()),
    }
}

/// `rag_search` results: `[n] title\nsource: path (collection c, chunk k)\ntext`.
fn parse_hits(text: &str) -> Vec<Evidence> {
    let mut out: Vec<Evidence> = Vec::new();
    for block in text.split("\n\n") {
        let mut lines = block.lines();
        let Some(head) = lines.next() else { continue };
        let Some(title) = head
            .strip_prefix('[')
            .and_then(|h| h.split_once("] "))
            .map(|(_, t)| t.trim())
        else {
            continue;
        };
        let Some(source) = lines
            .next()
            .and_then(|l| l.strip_prefix("source: "))
            .map(|l| {
                l.split(" (collection")
                    .next()
                    .unwrap_or(l)
                    .trim()
                    .to_owned()
            })
        else {
            continue;
        };
        out.push(Evidence {
            source,
            title: Some(title.split(" › ").next().unwrap_or(title).to_owned()),
            text: lines.collect::<Vec<_>>().join("\n"),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_search_hits() {
        let text = "[1] Leave policy › Annual leave\nsource: handbook/leave.md (collection hr, chunk 2)\nEveryone gets 25 days.\n\n[2] Pay\nsource: pay.md (collection hr, chunk 0)\nPaid monthly.\n\n";
        let hits = parse_hits(text);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].source, "handbook/leave.md");
        assert_eq!(hits[0].title.as_deref(), Some("Leave policy"));
        assert_eq!(hits[0].text, "Everyone gets 25 days.");
    }

    #[test]
    fn summaries() {
        let mk = |score: f64, latency_ms: u64| EvalResult {
            eval_run_id: "e".into(),
            case_id: score.to_string(),
            run_id: None,
            output: None,
            scores: json!([]),
            score,
            passed: score >= 1.0,
            cost: 0.01,
            latency_ms,
            error: None,
        };
        let s = Summary::of(
            &[mk(1.0, 100), mk(1.0, 200), mk(0.5, 900), mk(1.0, 300)],
            0.75,
        );
        assert_eq!((s.passed, s.total), (3, 4));
        assert!((s.score - 0.875).abs() < 1e-9);
        assert!(s.ok);
        assert_eq!(s.p95_latency_ms, 900);
        assert!(!Summary::of(&[mk(0.0, 1)], 0.8).ok);
    }
}
