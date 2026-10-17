//! `gai workflow`: list, check, save and run workflows.

use std::collections::HashSet;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::time::Duration;

use clap::Subcommand;
use gai::Gai;
use gai::agent::{Decision, RunEvent};
use gai::orchestra::{WorkflowRequest, WorkflowSpec};
use gai::store::RunStatus;
use serde_json::{Map, Value, json};

use crate::run::ask;
use crate::style::{age, cyan, dim, green, one_line, red, yellow};

#[derive(Subcommand)]
pub enum WorkflowCmd {
    /// List stored workflows and workflow files
    List,
    /// Print a workflow and its dependency graph
    Show { reference: String },
    /// Check a workflow file
    Validate { file: PathBuf },
    /// Store a workflow file as a new version
    Save {
        file: PathBuf,
        #[arg(long)]
        note: Option<String>,
    },
    /// Run a workflow and follow its nodes
    Run {
        /// A workflow file, a stored workflow (`name`, `name@v2`, `name@prod`), or a name in a workflows folder
        reference: String,
        /// Inputs as key=value (values that parse as JSON are used as JSON)
        #[arg(long = "input", short = 'i', value_parser = parse_input)]
        inputs: Vec<(String, Value)>,
        /// Run approve-policy tools without asking
        #[arg(long)]
        yes: bool,
        /// Print the outcome as JSON
        #[arg(long)]
        json: bool,
    },
    /// List a stored workflow's versions
    Versions { name: String },
    /// Show what changed between two versions (default: the latest two)
    Diff {
        name: String,
        /// Older version
        from: Option<u32>,
        /// Newer version (default: latest)
        to: Option<u32>,
    },
    /// Save an old version again as the newest one
    Rollback {
        name: String,
        version: u32,
        #[arg(long)]
        note: Option<String>,
    },
    /// Point a label (e.g. prod) at a version, or remove it with --remove
    Label {
        name: String,
        label: String,
        version: Option<u32>,
        #[arg(long)]
        remove: bool,
    },
    /// Delete a stored workflow
    Delete { name: String },
}

fn parse_input(s: &str) -> Result<(String, Value), String> {
    let (k, v) = s
        .split_once('=')
        .ok_or_else(|| format!("expected key=value, got {s:?}"))?;
    let value = serde_json::from_str(v).unwrap_or_else(|_| Value::String(v.to_owned()));
    Ok((k.to_owned(), value))
}

pub async fn workflow(gai: &Gai, cmd: WorkflowCmd) -> anyhow::Result<()> {
    let wf = gai.workflows().await?;
    match cmd {
        WorkflowCmd::List => {
            let stored = wf.runtime().store().list_workflows().await?;
            for w in &stored {
                let labels: Vec<String> =
                    w.labels.iter().map(|(l, v)| format!("{l}=v{v}")).collect();
                println!(
                    "{:<24} v{:<4} {} {} {}",
                    w.name,
                    w.latest_version,
                    dim(&age(w.updated_at)),
                    w.description
                        .as_deref()
                        .map(|d| one_line(d, 60))
                        .unwrap_or_default(),
                    cyan(&labels.join(" "))
                );
            }
            for (name, path) in wf.files() {
                if stored.iter().any(|w| w.name == name) {
                    continue;
                }
                println!("{:<24} {}", name, dim(&format!("file {}", path.display())));
            }
            if stored.is_empty() && wf.files().is_empty() {
                eprintln!(
                    "{}",
                    dim(
                        "No workflows. Write one in ./workflows/<name>.yaml (see examples/workflows)."
                    )
                );
            }
        }
        WorkflowCmd::Show { reference } => {
            let (spec, version) = wf.resolve(&reference).await?;
            if let Some(v) = version {
                eprintln!("{}", dim(&format!("# {} v{v}", spec.name)));
            }
            print!("{}", serde_yaml::to_string(&spec)?);
            eprintln!("{}", dim("# graph"));
            for n in spec.graph().map_err(anyhow::Error::msg)? {
                eprintln!(
                    "{}",
                    dim(&format!(
                        "#   {} ({}{}) <- {}",
                        n.id,
                        if n.map { "map:" } else { "" },
                        n.kind.as_str(),
                        if n.deps.is_empty() {
                            "start".into()
                        } else {
                            n.deps.join(", ")
                        }
                    ))
                );
            }
        }
        WorkflowCmd::Validate { file } => {
            let spec = WorkflowSpec::load(&file).map_err(anyhow::Error::msg)?;
            let g = spec.graph().map_err(anyhow::Error::msg)?;
            println!("{} {} ({} nodes)", green("ok"), spec.name, g.len());
        }
        WorkflowCmd::Save { file, note } => {
            let spec = WorkflowSpec::load(&file).map_err(anyhow::Error::msg)?;
            let v = wf.save(&spec, note.as_deref()).await?;
            println!("{} {} v{v}", green("saved"), spec.name);
        }
        WorkflowCmd::Versions { name } => {
            for v in wf.runtime().store().list_workflow_versions(&name).await? {
                println!(
                    "v{:<4} {} {}",
                    v.version,
                    dim(&age(v.created_at)),
                    v.note.unwrap_or_default()
                );
            }
        }
        WorkflowCmd::Diff { name, from, to } => {
            let store = wf.runtime().store();
            let latest = store
                .get_workflow(&name, None)
                .await?
                .ok_or_else(|| anyhow::anyhow!("no stored workflow {name}"))?
                .version;
            let to = to.unwrap_or(latest);
            let from = from.unwrap_or(to.saturating_sub(1).max(1));
            let get = |v: u32| {
                let name = name.clone();
                async move {
                    store
                        .get_workflow(&name, Some(v))
                        .await?
                        .ok_or_else(|| anyhow::anyhow!("workflow {name} has no v{v}"))
                }
            };
            let (a, b) = (get(from).await?, get(to).await?);
            crate::manage::print_diff(&gai_server::unified_diff(
                &gai_server::spec_yaml(&a.spec),
                &gai_server::spec_yaml(&b.spec),
                &format!("{name}@v{from}"),
                &format!("{name}@v{to}"),
            ));
        }
        WorkflowCmd::Rollback {
            name,
            version,
            note,
        } => {
            let store = wf.runtime().store();
            let old = store
                .get_workflow(&name, Some(version))
                .await?
                .ok_or_else(|| anyhow::anyhow!("workflow {name} has no v{version}"))?;
            let note = note.unwrap_or_else(|| format!("rollback to v{version}"));
            let v = store.put_workflow(&name, &old.spec, Some(&note)).await?;
            println!(
                "{} {name} v{v} {}",
                green("saved"),
                dim(&format!("(= v{version})"))
            );
        }
        WorkflowCmd::Label {
            name,
            label,
            version,
            remove,
        } => {
            if !remove && version.is_none() {
                anyhow::bail!("give a version to label, or --remove");
            }
            wf.runtime()
                .store()
                .set_workflow_label(&name, &label, if remove { None } else { version })
                .await?;
            println!("{}", green("ok"));
        }
        WorkflowCmd::Delete { name } => {
            if wf.runtime().store().delete_workflow(&name).await? {
                println!("{} {name}", green("deleted"));
            } else {
                anyhow::bail!("no stored workflow {name}");
            }
        }
        WorkflowCmd::Run {
            reference,
            inputs,
            yes,
            json: as_json,
        } => {
            let (spec, version) = wf.resolve(&reference).await?;
            let mut req = WorkflowRequest::new(spec, inputs.into_iter().collect::<Map<_, _>>());
            req.version = version;
            req.auto_approve = yes;
            let mut h = wf.start(req).await?;
            let rt = wf.runtime().clone();

            // Anything waiting on a person (human nodes, or tools in agent
            // runs the workflow started) is asked here.
            let asker = tokio::spawn(async move {
                let mut asked: HashSet<(String, String)> = HashSet::new();
                loop {
                    for p in rt.approvals().pending(None) {
                        if !asked.insert((p.run_id.clone(), p.call_id.clone())) {
                            continue;
                        }
                        let d = if p.tool == "human" {
                            human(p.input["prompt"].as_str().unwrap_or_default()).await
                        } else {
                            ask(&p.tool, &p.input).await
                        };
                        let _ = rt.resolve_approval(&p.run_id, &p.call_id, d);
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            });

            while let Some(env) = h.next_event().await {
                if as_json {
                    continue;
                }
                match env.event {
                    RunEvent::NodeStarted { node, kind } => {
                        eprintln!("{} {} {}", cyan("▶"), node, dim(&kind))
                    }
                    RunEvent::NodeFinished {
                        node,
                        status,
                        output,
                        error,
                        duration_ms,
                    } => {
                        let mark = match status.as_str() {
                            "completed" => green("✓"),
                            "skipped" => dim("–"),
                            _ => red("✗"),
                        };
                        let detail = match (&error, &output) {
                            (Some(e), _) => red(&one_line(e, 100)),
                            (None, Some(Value::String(s))) => dim(&one_line(s, 100)),
                            (None, Some(v)) => dim(&one_line(&v.to_string(), 100)),
                            _ => dim(&status),
                        };
                        eprintln!(
                            "{mark} {node} {detail} {}",
                            dim(&format!("({duration_ms} ms)"))
                        );
                    }
                    RunEvent::Warning { message } => eprintln!("{} {message}", yellow("warning:")),
                    _ => {}
                }
            }
            asker.abort();
            let o = h.wait().await;
            if as_json {
                println!("{}", serde_json::to_string_pretty(&o)?);
            } else {
                match &o.output {
                    Some(Value::String(s)) => println!("{s}"),
                    Some(v) => println!("{}", serde_json::to_string_pretty(v)?),
                    None => {}
                }
                let status = if o.status == RunStatus::Completed {
                    green("completed")
                } else {
                    red(o.status.as_str())
                };
                eprintln!(
                    "{}",
                    dim(&format!(
                        "{status} · {} nodes · ${:.4} · run {}",
                        o.steps, o.cost, o.run_id
                    ))
                );
                if let Some(e) = &o.error {
                    eprintln!("{} {e}", red("error:"));
                }
            }
            if o.status != RunStatus::Completed {
                std::process::exit(2);
            }
        }
    }
    Ok(())
}

/// Ask a person on the terminal for a human node.
async fn human(prompt: &str) -> Decision {
    if !std::io::stdin().is_terminal() {
        eprintln!(
            "{}",
            yellow("a human step is waiting but there is no terminal; declining")
        );
        return Decision::reject("no one was available");
    }
    eprintln!("{} {prompt}", yellow("?"));
    eprint!("{} ", yellow("[y]es / [n]o / or type a reply:"));
    let _ = std::io::stderr().flush();
    let line = tokio::task::spawn_blocking(|| {
        let mut s = String::new();
        std::io::stdin().read_line(&mut s).map(|_| s)
    })
    .await
    .ok()
    .and_then(Result::ok)
    .unwrap_or_default();
    match line.trim() {
        "y" | "Y" | "yes" => Decision::approve(),
        "" | "n" | "N" | "no" => Decision::reject("declined at the terminal"),
        reply => Decision {
            approved: true,
            reason: None,
            input: Some(json!({"reply": reply})),
        },
    }
}
