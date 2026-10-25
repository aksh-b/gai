//! Two evaluations side by side, and the pass/fail gate for CI.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::run::{EvalReport, Summary};

/// One case in both evaluations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CaseDelta {
    pub case_id: String,
    pub baseline: Option<f64>,
    pub candidate: Option<f64>,
    pub baseline_passed: bool,
    pub candidate_passed: bool,
    /// Candidate minus baseline (0 when a side is missing).
    pub delta: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Comparison {
    pub baseline_id: String,
    pub candidate_id: String,
    pub baseline: Summary,
    pub candidate: Summary,
    pub score_delta: f64,
    pub pass_rate_delta: f64,
    pub cost_delta: f64,
    pub latency_delta_ms: i64,
    pub cases: Vec<CaseDelta>,
    /// Cases that passed in the baseline and fail now.
    pub regressions: Vec<String>,
    /// Cases that failed in the baseline and pass now.
    pub fixes: Vec<String>,
}

/// Line `candidate` up against `baseline`, case by case.
pub fn compare(baseline: &EvalReport, candidate: &EvalReport) -> Comparison {
    let mut ids: Vec<String> = baseline.results.iter().map(|r| r.case_id.clone()).collect();
    for r in &candidate.results {
        if !ids.contains(&r.case_id) {
            ids.push(r.case_id.clone());
        }
    }
    let find = |rep: &EvalReport, id: &str| rep.results.iter().find(|r| r.case_id == id).cloned();
    let mut cases = Vec::new();
    let (mut regressions, mut fixes) = (Vec::new(), Vec::new());
    for id in ids {
        let (b, c) = (find(baseline, &id), find(candidate, &id));
        let bp = b.as_ref().is_some_and(|r| r.passed);
        let cp = c.as_ref().is_some_and(|r| r.passed);
        if b.is_some() && c.is_some() {
            if bp && !cp {
                regressions.push(id.clone());
            } else if !bp && cp {
                fixes.push(id.clone());
            }
        }
        cases.push(CaseDelta {
            delta: match (&b, &c) {
                (Some(b), Some(c)) => c.score - b.score,
                _ => 0.0,
            },
            baseline: b.map(|r| r.score),
            candidate: c.map(|r| r.score),
            baseline_passed: bp,
            candidate_passed: cp,
            case_id: id,
        });
    }
    let (bs, cs) = (&baseline.summary, &candidate.summary);
    Comparison {
        baseline_id: baseline.run.id.clone(),
        candidate_id: candidate.run.id.clone(),
        score_delta: cs.score - bs.score,
        pass_rate_delta: cs.pass_rate - bs.pass_rate,
        cost_delta: cs.cost - bs.cost,
        latency_delta_ms: cs.mean_latency_ms as i64 - bs.mean_latency_ms as i64,
        baseline: bs.clone(),
        candidate: cs.clone(),
        cases,
        regressions,
        fixes,
    }
}

/// Whether an evaluation is good enough to ship.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Verdict {
    pub ok: bool,
    pub reasons: Vec<String>,
}

/// Pass when the candidate reaches its set's threshold and, given a
/// baseline, its score dropped by at most `max_drop` (e.g. 0.05).
pub fn gate(candidate: &EvalReport, baseline: Option<&EvalReport>, max_drop: f64) -> Verdict {
    let s = &candidate.summary;
    let mut reasons = Vec::new();
    if !s.ok {
        reasons.push(format!(
            "{} of {} cases passed ({:.0}%), below the {:.0}% needed",
            s.passed,
            s.total,
            s.pass_rate * 100.0,
            s.threshold * 100.0
        ));
    }
    if let Some(b) = baseline {
        let c = compare(b, candidate);
        if c.score_delta < -max_drop - 1e-9 {
            reasons.push(format!(
                "score fell from {:.2} to {:.2} (more than {max_drop:.2})",
                c.baseline.score, c.candidate.score
            ));
            if !c.regressions.is_empty() {
                reasons.push(format!("newly failing: {}", c.regressions.join(", ")));
            }
        }
    }
    Verdict {
        ok: reasons.is_empty(),
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use gai_store::{EvalResult, EvalRun};
    use serde_json::json;

    use super::*;

    fn report(id: &str, cases: &[(&str, f64)]) -> EvalReport {
        let results: Vec<EvalResult> = cases
            .iter()
            .map(|(c, s)| EvalResult {
                eval_run_id: id.into(),
                case_id: (*c).into(),
                run_id: None,
                output: None,
                scores: json!([]),
                score: *s,
                passed: *s >= 1.0,
                cost: 0.0,
                latency_ms: 0,
                error: None,
            })
            .collect();
        EvalReport {
            run: EvalRun {
                id: id.into(),
                eval_set: "s".into(),
                eval_version: 1,
                target: json!({}),
                status: "completed".into(),
                summary: json!({}),
                error: None,
                created_at: Default::default(),
                finished_at: None,
            },
            summary: Summary::of(&results, 0.5),
            results,
        }
    }

    #[test]
    fn finds_regressions_and_gates() {
        let base = report("a", &[("x", 1.0), ("y", 1.0), ("z", 0.0)]);
        let cand = report("b", &[("x", 1.0), ("y", 0.0), ("z", 1.0), ("new", 1.0)]);
        let c = compare(&base, &cand);
        assert_eq!(c.regressions, ["y"]);
        assert_eq!(c.fixes, ["z"]);
        assert_eq!(c.cases.len(), 4);
        assert!(gate(&cand, Some(&base), 0.05).ok);

        let worse = report("c", &[("x", 1.0), ("y", 0.0), ("z", 0.0)]);
        let v = gate(&worse, Some(&base), 0.05);
        assert!(!v.ok);
        assert!(
            v.reasons.iter().any(|r| r.contains("newly failing: y")),
            "{v:?}"
        );
        let failing = report("d", &[("x", 0.0), ("y", 0.0)]);
        assert!(!gate(&failing, None, 0.05).ok);
    }
}
