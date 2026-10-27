//! Evaluations: does an agent (version, model) do its job?
//!
//! An [`EvalSet`] is a list of cases, each an input plus how to judge the
//! answer: rules ([`Scorer::Contains`], [`Scorer::Regex`], tool use, cost
//! and step limits) and an LLM judge with a rubric. [`Evaluator`] runs every
//! case against a target, scores it, and stores an [`EvalRun`] with its
//! per-case results. [`compare`] lines two evaluations up case by case, and
//! [`gate`] turns that into pass/fail for CI.
//!
//! ```yaml
//! name: support-basics
//! description: The refund and shipping questions we get most.
//! agent: support
//! threshold: 0.9            # share of cases that must pass
//! cases:
//!   - id: refund-window
//!     input: Can I return shoes I bought five weeks ago?
//!     expected: 30 days      # shorthand for `contains: [30 days]`
//!   - id: polite-no
//!     input: Give me a refund now or else.
//!     scorers:
//!       - judge: Declines calmly, explains the policy, offers help.
//!       - not_contains: [sorry for the inconvenience]
//!       - max_steps: 4
//! ```
//!
//! [`EvalRun`]: gai_store::EvalRun

mod compare;
mod optimize;
mod run;
mod score;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub use compare::{CaseDelta, Comparison, Verdict, compare, gate};
pub use optimize::{Attempt, OptimizeOptions, OptimizeProgress, OptimizeReport};
pub use run::{EvalOptions, EvalProgress, EvalReport, Evaluator, Progress, Summary, Target};
pub use score::{ScoreResult, Scorer};

#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    #[error("{0}")]
    Invalid(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error(transparent)]
    Store(#[from] gai_store::StoreError),
    #[error(transparent)]
    Agent(#[from] gai_agent::AgentError),
    #[error(transparent)]
    Llm(#[from] gai_llm::LlmError),
}

/// A named list of cases.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct EvalSet {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The agent evaluated when the run names none (`name`, `name@v3`,
    /// `name@prod`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Share of cases (0 to 1) that must pass for the evaluation to pass.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    /// Scorers applied to every case, before the case's own.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub scorers: Vec<Scorer>,
    /// Model for `judge` and `grounded` scorers; defaults to the default
    /// model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge_model: Option<String>,
    pub cases: Vec<Case>,
}

/// One input and how to judge the answer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Case {
    /// Unique within the set; defaults to the case's position (`1`, `2`, ...).
    pub id: String,
    pub input: String,
    /// The answer, or a phrase it must contain. With no scorers this case
    /// is scored with `contains: [expected]`; a `judge` sees it too.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub scorers: Vec<Scorer>,
    /// Template variables for the agent's instructions.
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub vars: Map<String, Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// YAML read through JSON values, so enums are written as one-key maps
/// (`- contains: [a]`) rather than YAML tags.
pub(crate) fn from_yaml<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, String> {
    let v: serde_yaml::Value = serde_yaml::from_str(text).map_err(|e| e.to_string())?;
    let v = serde_json::to_value(v).map_err(|e| e.to_string())?;
    serde_json::from_value(v).map_err(|e| e.to_string())
}

impl EvalSet {
    /// Parse YAML (or JSON) and check it.
    pub fn from_yaml(text: &str) -> Result<Self, EvalError> {
        let mut s: EvalSet =
            from_yaml(text).map_err(|e| EvalError::Invalid(format!("eval set: {e}")))?;
        s.normalize()?;
        Ok(s)
    }

    /// Fill in case ids and check names, ids and patterns.
    pub fn normalize(&mut self) -> Result<(), EvalError> {
        if self.cases.is_empty() {
            return Err(EvalError::Invalid(
                "an eval set needs at least one case".into(),
            ));
        }
        if let Some(t) = self.threshold
            && !(0.0..=1.0).contains(&t)
        {
            return Err(EvalError::Invalid(
                "threshold is a share from 0 to 1".into(),
            ));
        }
        let mut seen = std::collections::HashSet::new();
        for (i, c) in self.cases.iter_mut().enumerate() {
            if c.id.trim().is_empty() {
                c.id = (i + 1).to_string();
            }
            if !seen.insert(c.id.clone()) {
                return Err(EvalError::Invalid(format!(
                    "two cases are called {:?}",
                    c.id
                )));
            }
            if c.input.trim().is_empty() {
                return Err(EvalError::Invalid(format!("case {:?} has no input", c.id)));
            }
            for s in &c.scorers {
                s.check()
                    .map_err(|e| EvalError::Invalid(format!("case {:?}: {e}", c.id)))?;
            }
        }
        for s in &self.scorers {
            s.check().map_err(EvalError::Invalid)?;
        }
        Ok(())
    }

    /// The scorers a case is judged by.
    pub fn scorers_for<'a>(&'a self, case: &'a Case) -> Vec<Scorer> {
        let mut out: Vec<Scorer> = self.scorers.clone();
        out.extend(case.scorers.iter().cloned());
        if out.is_empty()
            && let Some(e) = &case.expected
        {
            out.push(Scorer::Contains(vec![e.clone()]));
        }
        out
    }
}
