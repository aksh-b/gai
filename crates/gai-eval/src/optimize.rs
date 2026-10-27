//! The prompt optimizer: rewrite an agent's instructions to raise its
//! score on an eval set.
//!
//! It evaluates the agent as it is, then for a few rounds shows a writing
//! model the instructions and the cases that failed (input, expected
//! answer, what the agent said, why it was marked down), asks for better
//! instructions, and evaluates each candidate. A candidate is kept only
//! when it scores higher than the best so far. Nothing is saved to the
//! agent: the report holds the best instructions, to apply or not.
//!
//! When the agent uses a prompt template (`prompt:`), the template's text
//! is what gets rewritten, so the result can be saved as its next version.

use std::sync::Arc;

use gai_agent::AgentSpec;
use gai_llm::ChatRequest;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::EvalError;
use crate::run::{EvalOptions, EvalReport, Evaluator, Target};
use crate::score::parse_json;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct OptimizeOptions {
    /// Rounds of rewriting (1 to 10).
    pub rounds: u32,
    /// Candidates written and evaluated each round (1 to 5).
    pub candidates: u32,
    /// The model that rewrites the instructions; the default model when
    /// not given.
    pub writer_model: Option<String>,
    /// Stop once the score reaches this (0 to 1).
    pub target_score: f64,
    /// How each evaluation runs (cases, tags, judge model, concurrency).
    #[serde(flatten)]
    pub eval: EvalOptions,
}

impl Default for OptimizeOptions {
    fn default() -> Self {
        Self {
            rounds: 3,
            candidates: 2,
            writer_model: None,
            target_score: 1.0,
            eval: EvalOptions::default(),
        }
    }
}

/// One set of instructions and how it scored.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Attempt {
    /// 0 for the agent as it is.
    pub round: u32,
    pub instructions: String,
    /// What the writer says it changed.
    pub rationale: Option<String>,
    pub eval_run_id: Option<String>,
    /// Mean case score, 0 to 1.
    pub score: f64,
    pub pass_rate: f64,
    pub passed: u32,
    pub total: u32,
    /// It beat everything before it.
    pub kept: bool,
    /// Why it could not be written or evaluated.
    pub error: Option<String>,
}

/// What an optimization found, so far or in the end.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OptimizeReport {
    pub eval_set: String,
    pub agent: String,
    pub agent_version: Option<u32>,
    /// The prompt template rewritten, when the agent uses one
    /// (`name@version`); otherwise the agent's `system` is.
    pub prompt: Option<String>,
    pub baseline: Attempt,
    pub best: Attempt,
    /// Every candidate, in order (the baseline not included).
    pub attempts: Vec<Attempt>,
    /// The best scores higher than the baseline.
    pub improved: bool,
    pub done: bool,
}

/// Called with the report after the baseline and after each candidate.
pub type OptimizeProgress = Arc<dyn Fn(&OptimizeReport) + Send + Sync>;

const WRITER: &str = "You improve the instructions (the system prompt) of an AI assistant so that it does better on a set of test cases. \
Keep what already works and fix what failed. Write general instructions, not a list of the test questions, unless a fact the assistant must know is missing. \
Keep template variables such as {{ name }} exactly as they are. \
Answer with JSON only: {\"instructions\": \"the full new instructions\", \"rationale\": \"one sentence on what you changed\"}.";

/// Case outputs shown to the writer are cut to this many characters.
const MAX_SHOWN: usize = 600;
/// Failing cases shown to the writer.
const MAX_FAILURES: usize = 8;

fn attempt(round: u32, instructions: &str, rationale: Option<String>, r: &EvalReport) -> Attempt {
    Attempt {
        round,
        instructions: instructions.to_owned(),
        rationale,
        eval_run_id: Some(r.run.id.clone()),
        score: r.summary.score,
        pass_rate: r.summary.pass_rate,
        passed: r.summary.passed,
        total: r.summary.total,
        kept: false,
        error: None,
    }
}

fn cut(s: &str) -> String {
    if s.chars().count() <= MAX_SHOWN {
        return s.to_owned();
    }
    let mut t: String = s.chars().take(MAX_SHOWN).collect();
    t.push('…');
    t
}

/// `{% set %}` lines giving template variables their defaults, so a
/// candidate rendered outside its prompt template sees them too.
fn defaults_prelude(vars: &Map<String, Value>) -> String {
    vars.iter()
        .filter(|(k, v)| !v.is_null() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .map(|(k, v)| format!("{{% set {k} = {k} if {k} is defined else {v} %}}"))
        .collect::<Vec<_>>()
        .join("")
}

/// What the writer sees: the instructions, the score, and the failures.
fn brief(
    agent: &AgentSpec,
    instructions: &str,
    best: &EvalReport,
    set: &crate::EvalSet,
    tried: &[Attempt],
) -> String {
    let mut out = String::new();
    if let Some(d) = &agent.description {
        out.push_str(&format!("The assistant: {d}\n\n"));
    }
    out.push_str(&format!(
        "Current instructions:\n<<<\n{instructions}\n>>>\n\nThey score {:.2} and pass {} of {} cases.\n\nFailing cases:\n",
        best.summary.score, best.summary.passed, best.summary.total
    ));
    for r in best.results.iter().filter(|r| !r.passed).take(MAX_FAILURES) {
        let Some(case) = set.cases.iter().find(|c| c.id == r.case_id) else {
            continue;
        };
        out.push_str(&format!("- Case {} (score {:.2})\n", case.id, r.score));
        out.push_str(&format!("  Input: {}\n", cut(&case.input)));
        if let Some(e) = &case.expected {
            out.push_str(&format!("  Expected: {}\n", cut(e)));
        }
        match (&r.error, &r.output) {
            (Some(e), _) => out.push_str(&format!("  Error: {}\n", cut(e))),
            (None, Some(o)) => out.push_str(&format!("  Answer: {}\n", cut(o))),
            _ => {}
        }
        let why: Vec<String> = r
            .scores
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| s["passed"] == json!(false))
            .filter_map(|s| {
                Some(format!(
                    "{}: {}",
                    s["scorer"].as_str()?,
                    s["reason"].as_str().unwrap_or_default()
                ))
            })
            .collect();
        if !why.is_empty() {
            out.push_str(&format!("  Why: {}\n", cut(&why.join("; "))));
        }
    }
    let lost: Vec<&Attempt> = tried
        .iter()
        .filter(|a| !a.kept && a.error.is_none())
        .collect();
    if !lost.is_empty() {
        out.push_str("\nThese rewrites did not score higher, so try something else:\n");
        for a in lost.iter().rev().take(3) {
            out.push_str(&format!(
                "- ({:.2}) {}\n",
                a.score,
                a.rationale.as_deref().unwrap_or("no rationale")
            ));
        }
    }
    out
}

impl Evaluator {
    /// Rewrite an agent's instructions to raise its score on an eval set.
    pub async fn optimize(
        &self,
        reference: &str,
        target: &Target,
        opts: &OptimizeOptions,
        progress: Option<OptimizeProgress>,
    ) -> Result<OptimizeReport, EvalError> {
        let (set, version) = self.load(reference).await?;
        let agent_ref = target
            .agent
            .clone()
            .or_else(|| set.agent.clone())
            .ok_or_else(|| {
                EvalError::Invalid(format!(
                    "eval set {} names no agent; choose one to optimize",
                    set.name
                ))
            })?;
        let rt = self.runtime();
        let (mut agent, agent_version) = rt.resolve_agent(&agent_ref).await?;
        if let Some(m) = &target.model {
            agent.model = Some(m.clone());
        }
        let writer_name = opts
            .writer_model
            .clone()
            .or_else(|| rt.default_model())
            .ok_or_else(|| EvalError::Invalid("choose a model to rewrite with".into()))?;
        let writer = rt.models().chat(&writer_name)?;

        // What gets rewritten: the prompt template's text, or `system`.
        let (prompt, instructions, own_system, prelude) = match agent.prompt.clone() {
            Some(r) => {
                let (p, v) = rt.resolve_prompt(&r).await?;
                let defaults: Map<String, Value> = p
                    .variables
                    .iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.default.clone()?)))
                    .collect();
                (
                    Some(format!("{}@v{v}", p.name)),
                    p.text.clone(),
                    agent.system.clone(),
                    defaults_prelude(&defaults),
                )
            }
            None => (
                None,
                agent.system.clone().unwrap_or_default(),
                None,
                String::new(),
            ),
        };
        let candidate = |text: &str| {
            let mut a = agent.clone();
            a.prompt = None;
            a.system = Some(match &own_system {
                Some(own) if !own.trim().is_empty() => {
                    format!("{prelude}{}\n\n{own}", text.trim_end())
                }
                _ => format!("{prelude}{text}"),
            });
            a
        };
        let rounds = opts.rounds.clamp(1, 10);
        let per_round = opts.candidates.clamp(1, 5);
        let eval = |spec: AgentSpec, round: u32| {
            let meta = json!({"agent_ref": agent_ref, "optimizer": {"round": round}});
            self.run_spec(
                (set.clone(), version),
                spec,
                agent_version,
                meta,
                &opts.eval,
                None,
            )
        };

        let base = eval(candidate(&instructions), 0).await?;
        let mut report = OptimizeReport {
            eval_set: set.name.clone(),
            agent: agent.name.clone(),
            agent_version,
            prompt,
            baseline: attempt(0, &instructions, None, &base),
            ..Default::default()
        };
        report.baseline.kept = true;
        report.best = report.baseline.clone();
        let mut best_report = base;
        if let Some(p) = &progress {
            p(&report);
        }

        'rounds: for round in 1..=rounds {
            if report.best.score + 1e-9 >= opts.target_score {
                break;
            }
            for _ in 0..per_round {
                let ask = brief(
                    &agent,
                    &report.best.instructions,
                    &best_report,
                    &set,
                    &report.attempts,
                );
                let mut req = ChatRequest::user(ask).with_system(WRITER);
                req.temperature = Some(0.7);
                req.max_tokens = Some(4000);
                let written = match writer.complete(req).await {
                    Ok(resp) => {
                        let text = resp.text();
                        match parse_json(&text) {
                            Some(v) => match v["instructions"].as_str().map(str::trim) {
                                Some(i) if !i.is_empty() => {
                                    Ok((i.to_owned(), v["rationale"].as_str().map(str::to_owned)))
                                }
                                _ => Err("the writer gave no instructions".to_owned()),
                            },
                            None => {
                                Err(format!("the writer did not answer in JSON: {}", cut(&text)))
                            }
                        }
                    }
                    Err(e) => Err(e.to_string()),
                };
                let (text, rationale) = match written {
                    Ok(w) => w,
                    Err(e) => {
                        report.attempts.push(Attempt {
                            round,
                            error: Some(e),
                            ..Default::default()
                        });
                        if let Some(p) = &progress {
                            p(&report);
                        }
                        continue;
                    }
                };
                if minijinja::Environment::new()
                    .template_from_str(&text)
                    .is_err()
                {
                    report.attempts.push(Attempt {
                        round,
                        instructions: text,
                        rationale,
                        error: Some("the rewrite is not a valid template".into()),
                        ..Default::default()
                    });
                    continue;
                }
                let mut a = match eval(candidate(&text), round).await {
                    Ok(r) => {
                        let a = attempt(round, &text, rationale, &r);
                        if a.score > report.best.score + 1e-9 {
                            best_report = r;
                        }
                        a
                    }
                    Err(e) => Attempt {
                        round,
                        instructions: text,
                        rationale,
                        error: Some(e.to_string()),
                        ..Default::default()
                    },
                };
                if a.error.is_none() && a.score > report.best.score + 1e-9 {
                    a.kept = true;
                    report.best = a.clone();
                }
                report.attempts.push(a);
                if let Some(p) = &progress {
                    p(&report);
                }
                if report.best.score + 1e-9 >= opts.target_score {
                    break 'rounds;
                }
            }
        }
        report.improved = report.best.score > report.baseline.score + 1e-9;
        report.done = true;
        if let Some(p) = &progress {
            p(&report);
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_become_set_lines() {
        let vars = json!({"company": "Acme", "n": 3, "bad key": 1, "none": null});
        let p = defaults_prelude(vars.as_object().unwrap());
        assert_eq!(
            p,
            "{% set company = company if company is defined else \"Acme\" %}{% set n = n if n is defined else 3 %}"
        );
        let out = minijinja::Environment::new()
            .render_str(
                &format!("{p}Hi {{{{ company }}}} {{{{ n }}}}"),
                json!({"n": 5}),
            )
            .unwrap();
        assert_eq!(out, "Hi Acme 5");
    }
}
