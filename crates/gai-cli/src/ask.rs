//! `gai ask`: one prompt straight to a model.

use std::io::Write;

use futures::StreamExt;
use gai::Gai;
use gai::llm::{Accumulator, ChatEvent, ChatRequest, Effort, Reasoning};

use crate::run::read_prompt;
use crate::style::dim;

#[derive(clap::Args)]
pub struct AskArgs {
    /// Prompt text. Reads stdin when omitted or `-`.
    prompt: Vec<String>,
    /// Model address (`provider/model` or an alias)
    #[arg(long, short)]
    model: Option<String>,
    /// System prompt
    #[arg(long, short)]
    system: Option<String>,
    /// Reasoning effort: off, low, medium, high, xhigh, max
    #[arg(long, short)]
    effort: Option<Effort>,
    /// Print the model's reasoning (or its summary) to stderr
    #[arg(long)]
    show_reasoning: bool,
    #[arg(long)]
    max_tokens: Option<u32>,
    #[arg(long)]
    temperature: Option<f32>,
    /// Print the full response as JSON instead of streaming text
    #[arg(long)]
    json: bool,
}

pub async fn ask(gai: &Gai, args: AskArgs) -> anyhow::Result<()> {
    let prompt = read_prompt(&args.prompt)?;
    let model = gai.chat_model(args.model.as_deref())?;

    let mut req = ChatRequest::user(prompt);
    req.system = args.system;
    req.max_tokens = args.max_tokens;
    req.temperature = args.temperature;
    if args.effort.is_some() || args.show_reasoning {
        req.reasoning = Some(Reasoning {
            effort: args.effort.unwrap_or_default(),
            budget_tokens: None,
            expose: args.show_reasoning,
        });
    }

    let mut stream = model.stream(req).await?;
    let mut acc = Accumulator::new(&model.info().id);
    let mut out = std::io::stdout().lock();
    let mut last_was_reasoning = false;
    while let Some(event) = stream.next().await {
        let event = event?;
        if !args.json {
            match &event {
                ChatEvent::TextDelta { text, .. } => {
                    if last_was_reasoning {
                        eprintln!();
                        last_was_reasoning = false;
                    }
                    write!(out, "{text}")?;
                    out.flush()?;
                }
                ChatEvent::ReasoningDelta { text, .. } if args.show_reasoning => {
                    eprint!("{}", dim(text));
                    last_was_reasoning = true;
                }
                _ => {}
            }
        }
        acc.push(&event);
    }
    let resp = acc.finish();
    if args.json {
        serde_json::to_writer_pretty(&mut out, &resp)?;
        writeln!(out)?;
        return Ok(());
    }
    if !resp.text().ends_with('\n') {
        writeln!(out)?;
    }
    let u = resp.usage;
    let cost = model
        .info()
        .cost(&u)
        .map(|c| format!(" · ${c:.4}"))
        .unwrap_or_default();
    let cached = if u.cache_read_tokens > 0 {
        format!(" ({} cached)", u.cache_read_tokens)
    } else {
        String::new()
    };
    eprintln!(
        "{}",
        dim(&format!(
            "{} · {} in{cached} · {} out · {:?}{cost}",
            model.info().id,
            u.total_input(),
            u.output_tokens,
            resp.stop_reason
        ))
    );
    Ok(())
}
