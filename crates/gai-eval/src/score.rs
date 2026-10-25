//! Scorers: rules checked in code, and judgements asked of a model.

use gai_llm::{ChatRequest, SharedChatModel};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How one aspect of an answer is judged. Written as a one-key map in YAML:
/// `- contains: [30 days]`, `- judge: Is polite`, `- max_steps: 4`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Scorer {
    /// The whole answer equals this (ignoring case and surrounding space).
    Equals(String),
    /// The answer contains every phrase (ignoring case).
    Contains(Vec<String>),
    /// The answer contains at least one phrase (ignoring case).
    ContainsAny(Vec<String>),
    /// The answer contains none of these phrases (ignoring case).
    NotContains(Vec<String>),
    /// The answer matches this regular expression.
    Regex(String),
    /// The answer is JSON; listed fields (dotted paths) must equal the values.
    Json(JsonCheck),
    /// The agent called this tool at least once.
    ToolCalled(String),
    /// The agent never called this tool.
    ToolNotCalled(String),
    MaxSteps(u32),
    MaxCostUsd(f64),
    MaxLatencyMs(u64),
    /// A model grades the answer against a rubric.
    Judge(Judge),
    /// A model checks that the answer's claims are backed by what the
    /// agent's tools (knowledge search, web pages) returned.
    Grounded(Grounded),
    /// The answer names at least one source the agent retrieved (a
    /// document, file or URL).
    Cites(bool),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct JsonCheck {
    pub fields: serde_json::Map<String, Value>,
}

/// `judge: <rubric>` or `judge: {rubric, model, pass}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Judge {
    Rubric(String),
    Full {
        rubric: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// Lowest passing grade, 0 to 1 (default 0.7).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pass: Option<f64>,
    },
}

/// `grounded: true` or `grounded: {model, min}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Grounded {
    On(bool),
    Full {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// Lowest share of supported claims that passes (default 0.8).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<f64>,
    },
}

/// What a scorer found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScoreResult {
    /// The scorer, e.g. `contains` or `judge`.
    pub scorer: String,
    /// 0 to 1.
    pub score: f64,
    pub passed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A source the agent's tools returned, for grounding and citations.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Evidence {
    /// A path, URL or collection source.
    pub source: String,
    /// The document's title, when it has one.
    pub title: Option<String>,
    pub text: String,
}

/// What a run produced, as the scorers see it.
#[derive(Clone, Debug, Default)]
pub struct Observed {
    pub input: String,
    pub expected: Option<String>,
    pub output: String,
    pub steps: u32,
    pub cost: f64,
    pub latency_ms: u64,
    pub tools: Vec<String>,
    pub evidence: Vec<Evidence>,
}

fn result(scorer: &str, passed: bool, reason: impl Into<Option<String>>) -> ScoreResult {
    ScoreResult {
        scorer: scorer.into(),
        score: if passed { 1.0 } else { 0.0 },
        passed,
        reason: reason.into(),
    }
}

impl Scorer {
    pub fn name(&self) -> &'static str {
        match self {
            Scorer::Equals(_) => "equals",
            Scorer::Contains(_) => "contains",
            Scorer::ContainsAny(_) => "contains_any",
            Scorer::NotContains(_) => "not_contains",
            Scorer::Regex(_) => "regex",
            Scorer::Json(_) => "json",
            Scorer::ToolCalled(_) => "tool_called",
            Scorer::ToolNotCalled(_) => "tool_not_called",
            Scorer::MaxSteps(_) => "max_steps",
            Scorer::MaxCostUsd(_) => "max_cost_usd",
            Scorer::MaxLatencyMs(_) => "max_latency_ms",
            Scorer::Judge(_) => "judge",
            Scorer::Grounded(_) => "grounded",
            Scorer::Cites(_) => "cites",
        }
    }

    /// Whether this scorer asks a model.
    pub fn uses_model(&self) -> bool {
        matches!(self, Scorer::Judge(_) | Scorer::Grounded(_))
    }

    /// The judge model this scorer names, if any.
    pub fn model(&self) -> Option<&str> {
        match self {
            Scorer::Judge(Judge::Full { model, .. })
            | Scorer::Grounded(Grounded::Full { model, .. }) => model.as_deref(),
            _ => None,
        }
    }

    pub(crate) fn check(&self) -> Result<(), String> {
        match self {
            Scorer::Regex(p) => regex::Regex::new(p)
                .map(|_| ())
                .map_err(|e| format!("regex {p:?}: {e}")),
            Scorer::Contains(v) | Scorer::ContainsAny(v) | Scorer::NotContains(v)
                if v.is_empty() =>
            {
                Err(format!("{} needs at least one phrase", self.name()))
            }
            Scorer::Judge(Judge::Rubric(r) | Judge::Full { rubric: r, .. })
                if r.trim().is_empty() =>
            {
                Err("judge needs a rubric".into())
            }
            _ => Ok(()),
        }
    }

    /// Score `o`. `judge` is the model for `judge` and `grounded` when the
    /// scorer names none.
    pub async fn score(&self, o: &Observed, judge: Option<&SharedChatModel>) -> ScoreResult {
        let name = self.name();
        let lower = o.output.to_lowercase();
        match self {
            Scorer::Equals(want) => {
                let ok = o.output.trim().eq_ignore_ascii_case(want.trim());
                result(
                    name,
                    ok,
                    (!ok).then(|| format!("expected exactly {want:?}")),
                )
            }
            Scorer::Contains(phrases) => {
                let missing: Vec<&String> = phrases
                    .iter()
                    .filter(|p| !lower.contains(&p.to_lowercase()))
                    .collect();
                let score = 1.0 - missing.len() as f64 / phrases.len() as f64;
                ScoreResult {
                    scorer: name.into(),
                    score,
                    passed: missing.is_empty(),
                    reason: (!missing.is_empty()).then(|| format!("missing {missing:?}")),
                }
            }
            Scorer::ContainsAny(phrases) => {
                let ok = phrases.iter().any(|p| lower.contains(&p.to_lowercase()));
                result(name, ok, (!ok).then(|| format!("none of {phrases:?}")))
            }
            Scorer::NotContains(phrases) => {
                let found: Vec<&String> = phrases
                    .iter()
                    .filter(|p| lower.contains(&p.to_lowercase()))
                    .collect();
                result(
                    name,
                    found.is_empty(),
                    (!found.is_empty()).then(|| format!("contains {found:?}")),
                )
            }
            Scorer::Regex(p) => {
                let ok = regex::Regex::new(p).is_ok_and(|r| r.is_match(&o.output));
                result(name, ok, (!ok).then(|| format!("does not match {p:?}")))
            }
            Scorer::Json(check) => {
                let Some(v) = parse_json(&o.output) else {
                    return result(name, false, Some("not JSON".into()));
                };
                let wrong: Vec<String> = check
                    .fields
                    .iter()
                    .filter(|(path, want)| lookup(&v, path) != Some(*want))
                    .map(|(path, want)| {
                        format!(
                            "{path} is {}, not {want}",
                            lookup(&v, path).map_or("missing".into(), |g| g.to_string())
                        )
                    })
                    .collect();
                result(
                    name,
                    wrong.is_empty(),
                    (!wrong.is_empty()).then(|| wrong.join("; ")),
                )
            }
            Scorer::ToolCalled(t) => {
                let ok = o.tools.iter().any(|x| x == t);
                result(name, ok, (!ok).then(|| format!("{t} was not called")))
            }
            Scorer::ToolNotCalled(t) => {
                let ok = !o.tools.iter().any(|x| x == t);
                result(name, ok, (!ok).then(|| format!("{t} was called")))
            }
            Scorer::MaxSteps(n) => {
                let ok = o.steps <= *n;
                result(name, ok, (!ok).then(|| format!("{} steps", o.steps)))
            }
            Scorer::MaxCostUsd(n) => {
                let ok = o.cost <= *n;
                result(name, ok, (!ok).then(|| format!("cost ${:.4}", o.cost)))
            }
            Scorer::MaxLatencyMs(n) => {
                let ok = o.latency_ms <= *n;
                result(name, ok, (!ok).then(|| format!("took {} ms", o.latency_ms)))
            }
            Scorer::Cites(false) => result(name, true, None),
            Scorer::Cites(true) => {
                let named = o.evidence.iter().find(|e| {
                    let s = e.source.to_lowercase();
                    let short = s.rsplit(['/', '\\']).next().unwrap_or(&s);
                    let title = e.title.as_deref().map(str::to_lowercase);
                    (!s.is_empty()
                        && (lower.contains(&s) || (short.len() > 3 && lower.contains(short))))
                        || title.is_some_and(|t| t.len() > 3 && lower.contains(&t))
                });
                match (o.evidence.is_empty(), named) {
                    (true, _) => result(name, false, Some("the agent retrieved no sources".into())),
                    (false, Some(e)) => result(name, true, Some(format!("cites {}", e.source))),
                    (false, None) => result(
                        name,
                        false,
                        Some("names none of the sources it used".into()),
                    ),
                }
            }
            Scorer::Judge(j) => {
                let (rubric, pass) = match j {
                    Judge::Rubric(r) => (r.as_str(), 0.7),
                    Judge::Full { rubric, pass, .. } => (rubric.as_str(), pass.unwrap_or(0.7)),
                };
                let Some(model) = judge else {
                    return result(name, false, Some("no judge model".into()));
                };
                judge_answer(model, o, rubric, pass).await
            }
            Scorer::Grounded(Grounded::On(false)) => result(name, true, None),
            Scorer::Grounded(g) => {
                let min = match g {
                    Grounded::Full { min, .. } => min.unwrap_or(0.8),
                    Grounded::On(_) => 0.8,
                };
                let Some(model) = judge else {
                    return result(name, false, Some("no judge model".into()));
                };
                if o.evidence.is_empty() {
                    return result(
                        name,
                        false,
                        Some("the agent retrieved no sources to ground the answer in".into()),
                    );
                }
                check_grounding(model, o, min).await
            }
        }
    }
}

/// The JSON in `text`, also when it is wrapped in a code fence.
pub(crate) fn parse_json(text: &str) -> Option<Value> {
    let t = text.trim();
    if let Ok(v) = serde_json::from_str(t) {
        return Some(v);
    }
    let start = t.find(['{', '['])?;
    let end = t.rfind(['}', ']'])?;
    serde_json::from_str(&t[start..=end]).ok()
}

fn lookup<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .try_fold(v, |v, key| match key.parse::<usize>() {
            Ok(i) if v.is_array() => v.get(i),
            _ => v.get(key),
        })
}

async fn ask(model: &SharedChatModel, system: &str, prompt: String) -> Result<Value, String> {
    let mut req = ChatRequest::user(prompt).with_system(system);
    req.temperature = Some(0.0);
    req.max_tokens = Some(800);
    let resp = model.complete(req).await.map_err(|e| e.to_string())?;
    parse_json(&resp.text())
        .ok_or_else(|| format!("the judge did not answer in JSON: {}", resp.text()))
}

async fn judge_answer(
    model: &SharedChatModel,
    o: &Observed,
    rubric: &str,
    pass: f64,
) -> ScoreResult {
    let system = "You grade an AI assistant's answer for an automated test. Be strict and consistent. Reply with only a JSON object: {\"score\": <0-10>, \"reason\": \"<one sentence>\"}.";
    let mut prompt = format!(
        "Question:\n{}\n\nAnswer:\n{}\n\nRubric (what a good answer does):\n{rubric}\n",
        o.input, o.output
    );
    if let Some(e) = &o.expected {
        prompt.push_str(&format!("\nReference answer:\n{e}\n"));
    }
    match ask(model, system, prompt).await {
        Ok(v) => {
            let score = (v["score"].as_f64().unwrap_or(0.0) / 10.0).clamp(0.0, 1.0);
            ScoreResult {
                scorer: "judge".into(),
                score,
                passed: score >= pass,
                reason: v["reason"].as_str().map(str::to_owned),
            }
        }
        Err(e) => result("judge", false, Some(e)),
    }
}

async fn check_grounding(model: &SharedChatModel, o: &Observed, min: f64) -> ScoreResult {
    let system = "You check whether an AI assistant's answer is supported by the sources it was given. Count the answer's factual claims and how many the sources support. Reply with only a JSON object: {\"claims\": <n>, \"supported\": <n>, \"unsupported\": [\"<claim>\", ...]}.";
    let mut sources = String::new();
    for (i, e) in o.evidence.iter().enumerate() {
        let text: String = e.text.chars().take(4000).collect();
        let name = match &e.title {
            Some(t) => format!("{t} ({})", e.source),
            None => e.source.clone(),
        };
        sources.push_str(&format!("[{}] {name}\n{text}\n\n", i + 1));
        if sources.len() > 24_000 {
            break;
        }
    }
    let prompt = format!("Sources:\n{sources}\nAnswer:\n{}\n", o.output);
    match ask(model, system, prompt).await {
        Ok(v) => {
            let claims = v["claims"].as_f64().unwrap_or(0.0);
            let supported = v["supported"].as_f64().unwrap_or(0.0);
            let score = if claims <= 0.0 {
                1.0
            } else {
                (supported / claims).clamp(0.0, 1.0)
            };
            let unsupported: Vec<&str> = v["unsupported"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            ScoreResult {
                scorer: "grounded".into(),
                score,
                passed: score >= min,
                reason: Some(if unsupported.is_empty() {
                    format!("{supported} of {claims} claims supported")
                } else {
                    format!("not supported: {}", unsupported.join("; "))
                }),
            }
        }
        Err(e) => result("grounded", false, Some(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(output: &str) -> Observed {
        Observed {
            output: output.into(),
            ..Default::default()
        }
    }

    async fn s(scorer: Scorer, o: &Observed) -> ScoreResult {
        scorer.score(o, None).await
    }

    #[tokio::test]
    async fn rules() {
        let o = obs("Returns are accepted within 30 Days of delivery.");
        assert!(s(Scorer::Contains(vec!["30 days".into()]), &o).await.passed);
        let half = s(
            Scorer::Contains(vec!["30 days".into(), "receipt".into()]),
            &o,
        )
        .await;
        assert_eq!((half.score, half.passed), (0.5, false));
        assert!(
            s(
                Scorer::ContainsAny(vec!["receipt".into(), "delivery".into()]),
                &o
            )
            .await
            .passed
        );
        assert!(
            !s(Scorer::NotContains(vec!["returns".into()]), &o)
                .await
                .passed
        );
        assert!(s(Scorer::Regex(r"\d+ Days".into()), &o).await.passed);
        assert!(s(Scorer::Equals(" 42 ".into()), &obs("42")).await.passed);

        let j = obs("Here you go:\n```json\n{\"total\": 12, \"items\": [{\"sku\": \"a\"}]}\n```");
        let mut fields = serde_json::Map::new();
        fields.insert("total".into(), 12.into());
        fields.insert("items.0.sku".into(), "a".into());
        assert!(
            s(
                Scorer::Json(JsonCheck {
                    fields: fields.clone()
                }),
                &j
            )
            .await
            .passed
        );
        fields.insert("total".into(), 13.into());
        let bad = s(Scorer::Json(JsonCheck { fields }), &j).await;
        assert!(!bad.passed && bad.reason.unwrap().contains("total is 12"));

        let o = Observed {
            steps: 3,
            cost: 0.02,
            tools: vec!["rag_search".into()],
            ..obs("x")
        };
        assert!(s(Scorer::ToolCalled("rag_search".into()), &o).await.passed);
        assert!(
            !s(Scorer::ToolNotCalled("rag_search".into()), &o)
                .await
                .passed
        );
        assert!(!s(Scorer::MaxSteps(2), &o).await.passed);
        assert!(s(Scorer::MaxCostUsd(0.05), &o).await.passed);
    }

    #[tokio::test]
    async fn citations() {
        let mut o = obs("You get 25 days of leave (see handbook/leave.md).");
        assert!(!s(Scorer::Cites(true), &o).await.passed);
        o.evidence.push(Evidence {
            source: "docs/handbook/leave.md".into(),
            title: None,
            text: "25 days".into(),
        });
        assert!(s(Scorer::Cites(true), &o).await.passed);
        o.output = "You get 25 days of leave.".into();
        assert!(!s(Scorer::Cites(true), &o).await.passed);
    }

    #[tokio::test]
    async fn grounding_with_the_mock_judge() {
        let registry = gai_llm::ModelRegistry::builder()
            .config(gai_llm::LlmConfig::default())
            .build()
            .unwrap();
        let judge = registry.chat("mock/judge").unwrap();
        let mut o = obs("Everyone gets twenty five days of annual leave.");
        let g = Scorer::Grounded(Grounded::On(true));
        assert!(!g.score(&o, Some(&judge)).await.passed, "no sources");
        o.evidence.push(Evidence {
            source: "leave.md".into(),
            title: Some("Leave policy".into()),
            text: "Everyone gets twenty five days of annual leave.".into(),
        });
        let ok = g.score(&o, Some(&judge)).await;
        assert!(ok.passed, "{ok:?}");
        o.output =
            "Everyone gets twenty five days of annual leave. Managers approve sabbaticals monthly."
                .into();
        let half = g.score(&o, Some(&judge)).await;
        assert_eq!(half.score, 0.5, "{half:?}");
        assert!(!half.passed);
        assert!(half.reason.unwrap().contains("sabbaticals"));

        let j = Scorer::Judge(Judge::Rubric("Mentions annual leave days".into()));
        assert!(j.score(&o, Some(&judge)).await.passed);
        assert!(!j.score(&obs("No idea."), Some(&judge)).await.passed);
    }

    #[test]
    fn yaml_shapes() {
        let v: Vec<Scorer> = crate::from_yaml(
            "- contains: [a]\n- judge: Is polite\n- judge: {rubric: Short, pass: 0.5}\n- grounded: true\n- max_steps: 3\n- cites: true\n",
        )
        .unwrap();
        assert_eq!(v[1], Scorer::Judge(Judge::Rubric("Is polite".into())));
        assert!(matches!(&v[2], Scorer::Judge(Judge::Full { pass: Some(p), .. }) if *p == 0.5));
        assert_eq!(v[3], Scorer::Grounded(Grounded::On(true)));
        assert!(Scorer::Regex("(".into()).check().is_err());
    }
}
