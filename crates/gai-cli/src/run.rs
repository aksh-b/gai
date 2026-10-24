//! `gai run`: one agent task, streamed to the terminal.

use std::io::{IsTerminal, Write};

use anyhow::bail;
use gai::Gai;
use gai::agent::{AgentSpec, Decision, RunEvent, RunOutcome, RunRequest};
use gai::store::RunStatus;
use serde_json::Value;

use crate::style::{cyan, dim, green, one_line, red, yellow};

#[derive(clap::Args)]
pub struct RunArgs {
    /// The task. Reads stdin when omitted or `-`.
    pub task: Vec<String>,
    #[command(flatten)]
    pub agent: AgentArgs,
    /// Continue an existing session
    #[arg(long)]
    pub session: Option<String>,
    /// Approve every tool call without asking
    #[arg(long, short = 'y')]
    pub yes: bool,
    /// Print the reasoning (or its summary) as it streams
    #[arg(long)]
    pub show_reasoning: bool,
    /// Template variables for the system prompt, as key=value
    #[arg(long = "var", value_parser = parse_var)]
    pub vars: Vec<(String, String)>,
    /// Print the outcome as JSON instead of streaming
    #[arg(long)]
    pub json: bool,
}

/// Choosing an agent, shared by `run` and `chat`.
#[derive(clap::Args, Clone)]
pub struct AgentArgs {
    /// Agent: a file, a stored name (`name`, `name@v3`, `name@prod`), or a
    /// name found in an agents directory. Without it, a general assistant.
    #[arg(long, short)]
    pub agent: Option<String>,
    /// Override the agent's model
    #[arg(long, short)]
    pub model: Option<String>,
    /// Override the agent's tools (comma-separated; `none` for no tools)
    #[arg(long, short, value_delimiter = ',')]
    pub tools: Option<Vec<String>>,
}

fn parse_var(s: &str) -> Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .ok_or_else(|| format!("expected key=value, got {s:?}"))
}

pub async fn resolve_agent(
    gai: &Gai,
    args: &AgentArgs,
) -> anyhow::Result<(AgentSpec, Option<u32>)> {
    let (mut spec, version) = match &args.agent {
        Some(r) => gai.find_agent(r).await?,
        None => (gai::default_agent(), None),
    };
    if let Some(m) = &args.model {
        spec.model = Some(m.clone());
    }
    if let Some(t) = &args.tools {
        spec.tools = if t.iter().any(|x| x == "none") {
            vec![]
        } else {
            t.clone()
        };
    }
    Ok((spec, version))
}

pub fn read_prompt(words: &[String]) -> anyhow::Result<String> {
    if words.is_empty() || words == ["-"] {
        if std::io::stdin().is_terminal() {
            bail!("give a task, or pipe one on stdin");
        }
        let mut s = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
        return Ok(s.trim().to_owned());
    }
    Ok(words.join(" "))
}

pub async fn run(gai: &Gai, args: RunArgs) -> anyhow::Result<()> {
    let task = read_prompt(&args.task)?;
    let (spec, version) = resolve_agent(gai, &args.agent).await?;
    let rt = gai.runtime().await?;
    let mut req = RunRequest::new(spec, task);
    req.agent_version = version;
    req.session_id = args.session.clone();
    req.auto_approve = args.yes;
    req.vars = args
        .vars
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect();

    let handle = rt.start(req).await?;
    follow(rt, handle, args.show_reasoning, args.json).await
}

/// Stream a run to the terminal, answering approvals interactively, and
/// exit non-zero unless it completes.
pub async fn follow(
    rt: &gai::agent::Runtime,
    mut handle: gai::agent::RunHandle,
    show_reasoning: bool,
    json: bool,
) -> anyhow::Result<()> {
    let mut printer = Printer {
        show_reasoning,
        quiet: json,
        ..Default::default()
    };
    while let Some(env) = handle.next_event().await {
        if let RunEvent::ApprovalRequired { id, name, input } = &env.event {
            printer.break_line();
            let decision = ask(name, input).await;
            if let Err(e) = rt.resolve_approval(&handle.run_id, id, decision) {
                eprintln!("{}", red(&e));
            }
            continue;
        }
        printer.event(&env.event);
    }
    let outcome = handle.wait().await;
    if json {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
    } else {
        printer.break_line();
        summary(&outcome);
    }
    if outcome.status != RunStatus::Completed {
        std::process::exit(2);
    }
    Ok(())
}

/// Ask on the terminal whether a tool call may run.
pub async fn ask(name: &str, input: &Value) -> Decision {
    if !std::io::stdin().is_terminal() {
        eprintln!(
            "{}",
            yellow(&format!(
                "{name} needs approval but there is no terminal to ask; declining (use --yes to allow)"
            ))
        );
        return Decision::reject("no one was available to approve it");
    }
    let shown = serde_json::to_string_pretty(input).unwrap_or_default();
    eprintln!("{} {}\n{}", yellow("approve?"), cyan(name), dim(&shown));
    eprint!("{} ", yellow("[y]es / [n]o / or type a reason to decline:"));
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
        "" | "n" | "N" | "no" => Decision::reject("the user declined"),
        reason => Decision::reject(reason),
    }
}

fn summary(o: &RunOutcome) {
    let status = match o.status {
        RunStatus::Completed => green("completed"),
        s => red(s.as_str()),
    };
    let cost = if o.cost > 0.0 {
        format!(" · ${:.4}", o.cost)
    } else {
        String::new()
    };
    eprintln!(
        "{}",
        dim(&format!(
            "{} · {} steps · {} in / {} out{} · run {}",
            "",
            o.steps,
            o.usage.total_input(),
            o.usage.output_tokens,
            cost,
            o.run_id
        ))
        .replacen(" · ", &format!("{status} · "), 1)
    );
    if let Some(e) = &o.error {
        eprintln!("{} {e}", red("error:"));
    }
}

/// Renders run events as terminal text: the answer on stdout, everything
/// else on stderr.
#[derive(Default)]
pub struct Printer {
    pub show_reasoning: bool,
    pub quiet: bool,
    mid_line: bool,
    in_reasoning: bool,
}

impl Printer {
    pub fn break_line(&mut self) {
        if self.mid_line {
            println!();
            self.mid_line = false;
        }
        if self.in_reasoning {
            eprintln!();
            self.in_reasoning = false;
        }
    }

    pub fn event(&mut self, e: &RunEvent) {
        if self.quiet {
            return;
        }
        match e {
            RunEvent::TextDelta { text } => {
                if self.in_reasoning {
                    eprintln!();
                    self.in_reasoning = false;
                }
                print!("{text}");
                let _ = std::io::stdout().flush();
                self.mid_line = !text.ends_with('\n');
            }
            RunEvent::ReasoningDelta { text } if self.show_reasoning => {
                eprint!("{}", dim(text));
                self.in_reasoning = true;
            }
            RunEvent::ToolCall { name, input, .. } => {
                self.break_line();
                let args = serde_json::to_string(input).unwrap_or_default();
                eprintln!(
                    "{} {}{}",
                    cyan("⚙"),
                    cyan(name),
                    dim(&format!(" {}", one_line(&args, 110)))
                );
            }
            RunEvent::ToolResult {
                output,
                duration_ms,
                ..
            } => {
                let mark = if output.is_error {
                    red("✗")
                } else {
                    green("✓")
                };
                eprintln!(
                    "  {mark} {}",
                    dim(&format!(
                        "{} ({duration_ms} ms)",
                        one_line(&output.as_text(), 100)
                    ))
                );
            }
            RunEvent::ApprovalResolved {
                approved: false,
                reason,
                ..
            } => {
                eprintln!(
                    "  {}",
                    yellow(&format!(
                        "declined{}",
                        reason
                            .as_ref()
                            .map(|r| format!(": {r}"))
                            .unwrap_or_default()
                    ))
                );
            }
            RunEvent::StepReplayed { step, from_run_id } => {
                self.break_line();
                eprintln!(
                    "{}",
                    dim(&format!("↺ step {step} replayed from {from_run_id}"))
                );
            }
            RunEvent::Warning { message } => {
                self.break_line();
                eprintln!("{} {message}", yellow("warning:"));
            }
            RunEvent::Compacted {
                summarized_messages,
                ..
            } => {
                self.break_line();
                eprintln!(
                    "{}",
                    dim(&format!(
                        "(summarized {summarized_messages} earlier messages to free context)"
                    ))
                );
            }
            RunEvent::ToolResultsCleared { cleared } => {
                eprintln!(
                    "{}",
                    dim(&format!(
                        "(cleared {cleared} old tool results to free context)"
                    ))
                );
            }
            _ => {}
        }
    }
}
