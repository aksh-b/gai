//! `gai replay`: fork a run from one of its steps.

use gai::Gai;
use gai::agent::{ReplayOverrides, ReplayRequest, StepEdit};
use serde_json::Map;

use crate::run::follow;
use crate::style::dim;

#[derive(clap::Args)]
pub struct ReplayArgs {
    /// The run to fork
    pub run: String,
    /// First model step to run live; earlier steps come from the recording
    /// (1 re-runs everything)
    #[arg(long = "from", short = 'f', default_value_t = 1)]
    pub from_step: u32,
    /// Model for the live steps
    #[arg(long, short)]
    pub model: Option<String>,
    /// System prompt for the fork (`@file` reads it from a file)
    #[arg(long)]
    pub system: Option<String>,
    /// Tools for the fork (comma-separated; `none` for no tools)
    #[arg(long, short, value_delimiter = ',')]
    pub tools: Option<Vec<String>>,
    /// Run this stored version of the agent instead of the original's snapshot
    #[arg(long = "agent-version", short = 'V')]
    pub agent_version: Option<u32>,
    /// Replace what the model said at a replayed step, as STEP=TEXT
    #[arg(long = "edit", value_parser = parse_edit)]
    pub edits: Vec<(u32, String)>,
    /// Approve every tool call without asking
    #[arg(long, short = 'y')]
    pub yes: bool,
    /// Print the reasoning (or its summary) as it streams
    #[arg(long)]
    pub show_reasoning: bool,
    /// Print the outcome as JSON instead of streaming
    #[arg(long)]
    pub json: bool,
}

fn parse_edit(s: &str) -> Result<(u32, String), String> {
    let (step, text) = s
        .split_once('=')
        .ok_or_else(|| format!("expected STEP=TEXT, got {s:?}"))?;
    let step = step
        .trim()
        .parse()
        .map_err(|_| format!("bad step number in {s:?}"))?;
    Ok((step, text.to_owned()))
}

pub async fn replay(gai: &Gai, args: ReplayArgs) -> anyhow::Result<()> {
    let system = match args.system {
        Some(s) => Some(match s.strip_prefix('@') {
            Some(path) => std::fs::read_to_string(path)?,
            None => s,
        }),
        None => None,
    };
    let tools = args.tools.map(|t| {
        if t.iter().any(|x| x == "none") {
            Vec::new()
        } else {
            t
        }
    });
    let req = ReplayRequest {
        from_step: args.from_step,
        overrides: ReplayOverrides {
            model: args.model,
            system_prompt: system,
            tools,
            agent_version: args.agent_version,
            auto_approve: args.yes.then_some(true),
            step_edits: args
                .edits
                .into_iter()
                .map(|(step, text)| StepEdit {
                    step,
                    text: Some(text),
                    ..Default::default()
                })
                .collect(),
        },
    };
    let rt = gai.runtime().await?;
    let handle = rt.replay(&args.run, req, Map::new()).await?;
    if !args.json {
        eprintln!(
            "{}",
            dim(&format!(
                "fork {} of {} (steps before {} replayed)",
                handle.run_id, args.run, args.from_step
            ))
        );
    }
    follow(rt, handle, args.show_reasoning, args.json).await
}
