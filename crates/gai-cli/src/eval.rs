//! `gai eval`: eval sets, evaluations, comparisons and the CI gate.

use std::path::PathBuf;
use std::sync::Arc;

use clap::Subcommand;
use gai::Gai;
use gai::eval::{EvalOptions, EvalProgress, EvalReport, EvalSet, Target, compare, gate};

use crate::style::{bold, dim, green, red, yellow};

#[derive(Subcommand)]
pub enum EvalCmd {
    /// List eval sets
    List,
    /// Print an eval set as YAML (name or name@v2)
    Show { set: String },
    /// Save an eval set from a YAML file as a new version
    Save {
        file: PathBuf,
        #[arg(long)]
        note: Option<String>,
    },
    /// Delete an eval set
    Delete { set: String },
    /// Evaluate an agent on a set (a stored set or a YAML file). Exits with
    /// status 1 when the evaluation fails its threshold or regresses
    /// against --baseline, so it can gate CI.
    Run {
        /// A stored set (name, name@v2) or a YAML file
        set: String,
        /// The agent to evaluate (defaults to the set's): name, name@v3, name@prod
        #[arg(long, short)]
        agent: Option<String>,
        /// Run on these models instead of the agent's; several compare them
        #[arg(long = "model", short)]
        models: Vec<String>,
        /// Model for judge and grounded scorers
        #[arg(long)]
        judge: Option<String>,
        /// Only these cases
        #[arg(long = "case")]
        cases: Vec<String>,
        /// Only cases with these tags
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Cases run at once
        #[arg(long, short = 'j', default_value_t = 4)]
        concurrency: usize,
        /// Compare against an earlier evaluation (its id) or another agent
        /// version (e.g. support@prod), evaluated now
        #[arg(long)]
        baseline: Option<String>,
        /// Largest score drop against the baseline that still passes
        #[arg(long, default_value_t = 0.05)]
        max_drop: f64,
        /// Print the report as JSON
        #[arg(long)]
        json: bool,
    },
    /// Evaluations, newest first
    Runs {
        set: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// An evaluation's per-case results
    Report {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Two evaluations side by side
    Compare {
        baseline: String,
        candidate: String,
        #[arg(long, default_value_t = 0.05)]
        max_drop: f64,
    },
}

pub async fn eval(gai: &Gai, cmd: EvalCmd) -> anyhow::Result<()> {
    let ev = gai.evals().await?;
    let store = gai.runtime().await?.store().clone();
    match cmd {
        EvalCmd::List => {
            let sets = store.list_eval_sets().await?;
            if sets.is_empty() {
                eprintln!(
                    "{}",
                    dim("No eval sets yet. Save one with `gai eval save <file>`.")
                );
            }
            for s in sets {
                println!(
                    "{}  v{}  {} cases  {}",
                    bold(&s.name),
                    s.latest_version,
                    s.cases,
                    dim(s.description.as_deref().unwrap_or(""))
                );
            }
        }
        EvalCmd::Show { set } => {
            let (s, v) = ev.load(&set).await?;
            eprintln!("{}", dim(&format!("{} v{v}", s.name)));
            print!("{}", serde_yaml::to_string(&s)?);
        }
        EvalCmd::Save { file, note } => {
            let set = EvalSet::from_yaml(&std::fs::read_to_string(&file)?)?;
            let set = if set.name.is_empty() {
                EvalSet {
                    name: file
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    ..set
                }
            } else {
                set
            };
            let v = ev.save(&set, note.as_deref()).await?;
            eprintln!(
                "{} {} v{v} ({} cases)",
                green("saved"),
                set.name,
                set.cases.len()
            );
        }
        EvalCmd::Delete { set } => {
            if store.delete_eval_set(&set).await? {
                eprintln!("{} {set}", green("deleted"));
            } else {
                anyhow::bail!("no eval set {set}");
            }
        }
        EvalCmd::Run {
            set,
            agent,
            models,
            judge,
            cases,
            tags,
            concurrency,
            baseline,
            max_drop,
            json,
        } => {
            let opts = EvalOptions {
                concurrency,
                judge_model: judge,
                cases,
                tags,
            };
            let quiet = json;
            let progress: gai::eval::Progress = Arc::new(move |p| {
                if quiet {
                    return;
                }
                if let EvalProgress::Case(r) = p {
                    let mark = if r.passed { green("pass") } else { red("FAIL") };
                    let why = r.error.clone().or_else(|| {
                        r.scores.as_array().and_then(|a| {
                            a.iter()
                                .filter(|s| s["passed"] == false)
                                .filter_map(|s| {
                                    Some(format!(
                                        "{}: {}",
                                        s["scorer"].as_str()?,
                                        s["reason"].as_str().unwrap_or("failed")
                                    ))
                                })
                                .next()
                        })
                    });
                    eprintln!(
                        "  {mark} {}  {:.2}  {}",
                        r.case_id,
                        r.score,
                        dim(&why.unwrap_or_default())
                    );
                }
            });
            let targets: Vec<Option<String>> = if models.is_empty() {
                vec![None]
            } else {
                models.into_iter().map(Some).collect()
            };
            let base = match &baseline {
                Some(b) if b.starts_with("eval_") => Some(ev.report(b).await?),
                Some(b) => {
                    eprintln!("{} {set} on {b} (baseline)", bold("evaluating"));
                    let t = Target {
                        agent: Some(b.clone()),
                        model: None,
                    };
                    Some(ev.run(&set, &t, &opts, Some(progress.clone())).await?)
                }
                None => None,
            };
            let mut reports = Vec::new();
            for model in targets {
                let t = Target {
                    agent: agent.clone(),
                    model: model.clone(),
                };
                if !quiet {
                    eprintln!(
                        "{} {set}{}{}",
                        bold("evaluating"),
                        agent
                            .as_deref()
                            .map(|a| format!(" on {a}"))
                            .unwrap_or_default(),
                        model
                            .as_deref()
                            .map(|m| format!(" with {m}"))
                            .unwrap_or_default()
                    );
                }
                let r = ev.run(&set, &t, &opts, Some(progress.clone())).await?;
                if !quiet {
                    print_summary(&r);
                }
                reports.push(r);
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&reports)?);
            }
            if reports.len() > 1 && !quiet {
                benchmark(&reports);
            }
            let mut ok = true;
            for r in &reports {
                if let Some(b) = &base
                    && !quiet
                {
                    print_comparison(b, r);
                }
                let v = gate(r, base.as_ref(), max_drop);
                if !v.ok {
                    ok = false;
                    for reason in &v.reasons {
                        eprintln!("{} {reason}", red("failed:"));
                    }
                }
            }
            if !ok {
                std::process::exit(1);
            }
        }
        EvalCmd::Runs { set, limit } => {
            for r in store.list_eval_runs(set.as_deref(), limit).await? {
                let s = &r.summary;
                println!(
                    "{}  {} v{}  {}  {}  {}",
                    r.id,
                    r.eval_set,
                    r.eval_version,
                    r.target["agent_ref"]
                        .as_str()
                        .or(r.target["agent"].as_str())
                        .unwrap_or("?"),
                    r.target["model"].as_str().unwrap_or(""),
                    match r.status.as_str() {
                        "completed" => format!(
                            "{}/{} passed, score {:.2}",
                            s["passed"],
                            s["total"],
                            s["score"].as_f64().unwrap_or(0.0)
                        ),
                        other => other.to_owned(),
                    }
                );
            }
        }
        EvalCmd::Report { id, json } => {
            let r = ev.report(&id).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&r)?);
                return Ok(());
            }
            for x in &r.results {
                let mark = if x.passed { green("pass") } else { red("FAIL") };
                println!(
                    "  {mark} {}  {:.2}  {}",
                    x.case_id,
                    x.score,
                    dim(x.error.as_deref().unwrap_or(""))
                );
            }
            print_summary(&r);
        }
        EvalCmd::Compare {
            baseline,
            candidate,
            max_drop,
        } => {
            let (b, c) = (ev.report(&baseline).await?, ev.report(&candidate).await?);
            print_comparison(&b, &c);
            let v = gate(&c, Some(&b), max_drop);
            if v.ok {
                eprintln!("{}", green("ok"));
            } else {
                for reason in &v.reasons {
                    eprintln!("{} {reason}", red("failed:"));
                }
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

fn print_summary(r: &EvalReport) {
    let s = &r.summary;
    let verdict = if s.ok {
        green("passed")
    } else {
        red("below threshold")
    };
    eprintln!(
        "{} {}/{} cases, score {:.2}, ${:.4}, {} ms mean ({} p95)  {}  {}",
        verdict,
        s.passed,
        s.total,
        s.score,
        s.cost,
        s.mean_latency_ms,
        s.p95_latency_ms,
        dim(&format!("need {:.0}%", s.threshold * 100.0)),
        dim(&r.run.id)
    );
}

fn print_comparison(b: &EvalReport, c: &EvalReport) {
    let cmp = compare(b, c);
    let arrow = |d: f64| {
        if d > 1e-9 {
            green(&format!("+{d:.2}"))
        } else if d < -1e-9 {
            red(&format!("{d:.2}"))
        } else {
            dim("±0")
        }
    };
    eprintln!(
        "{} score {:.2} → {:.2} ({}), passed {} → {}, cost ${:.4} → ${:.4}",
        bold("vs baseline:"),
        cmp.baseline.score,
        cmp.candidate.score,
        arrow(cmp.score_delta),
        cmp.baseline.passed,
        cmp.candidate.passed,
        cmp.baseline.cost,
        cmp.candidate.cost
    );
    if !cmp.regressions.is_empty() {
        eprintln!("  {} {}", red("newly failing:"), cmp.regressions.join(", "));
    }
    if !cmp.fixes.is_empty() {
        eprintln!("  {} {}", green("newly passing:"), cmp.fixes.join(", "));
    }
}

/// Models side by side: quality, cost and speed.
fn benchmark(reports: &[EvalReport]) {
    eprintln!(
        "\n{}",
        bold("model                         score  passed     cost   mean ms")
    );
    let best = reports
        .iter()
        .map(|r| r.summary.score)
        .fold(f64::MIN, f64::max);
    for r in reports {
        let s = &r.summary;
        let model = r.run.target["model"].as_str().unwrap_or("?");
        let line = format!(
            "{model:<29} {:>5.2}  {:>3}/{:<3}  ${:>7.4}  {:>7}",
            s.score, s.passed, s.total, s.cost, s.mean_latency_ms
        );
        eprintln!(
            "{}",
            if (s.score - best).abs() < 1e-9 {
                yellow(&line)
            } else {
                line
            }
        );
    }
}
