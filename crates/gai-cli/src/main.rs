mod ask;
mod bundle;
mod chat;
mod data;
mod eval;
mod governance;
mod knowledge;
mod manage;
mod media;
mod orgs;
mod replay;
mod run;
mod sources;
mod style;
mod triggers;
mod workflow;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use gai::Gai;
use gai::llm::KeyState;

use crate::style::{dim, green, red, yellow};

#[derive(Parser)]
#[command(name = "gai", version, about = "An agentic AI platform")]
struct Cli {
    /// Config file (default: $GAI_CONFIG, ./gai.toml, then ~/.config/gai/gai.toml)
    #[arg(long, global = true, env = "GAI_CONFIG")]
    config: Option<PathBuf>,

    /// Log level filter, e.g. `debug` or `gai_agent=trace`
    #[arg(long, global = true, env = "GAI_LOG", default_value = "warn")]
    log: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the HTTP API and the dashboard
    Serve {
        /// Address to listen on
        #[arg(long, default_value = "127.0.0.1:8484", env = "GAI_BIND")]
        bind: std::net::SocketAddr,
        /// Serve the dashboard from this build folder (e.g. web/dist)
        /// instead of the one built into the binary
        #[arg(long, env = "GAI_UI_DIR", conflicts_with = "no_ui")]
        ui: Option<PathBuf>,
        /// Serve only the API, without the dashboard
        #[arg(long)]
        no_ui: bool,
        /// auto (open until the first API key exists, local only), required, or off
        #[arg(long, env = "GAI_AUTH")]
        auth: Option<gai::AuthMode>,
        /// The port is only reachable from this machine (e.g. a container
        /// port published on 127.0.0.1), even though the bind address is not
        /// loopback
        #[arg(long)]
        local: bool,
    },
    /// Manage API keys for the server
    #[command(subcommand)]
    Keys(manage::KeysCmd),
    /// Organizations sharing this server
    #[command(subcommand)]
    Orgs(orgs::OrgsCmd),
    /// People in an organization and their roles
    #[command(subcommand)]
    Members(orgs::MembersCmd),
    /// Teams in an organization
    #[command(subcommand)]
    Teams(orgs::TeamsCmd),
    /// Manage MCP servers whose tools agents can use
    #[command(subcommand)]
    Mcp(manage::McpCmd),
    /// Give an agent a task and stream its work
    Run(run::RunArgs),
    /// Fork a run: replay its first steps from the recording, run the rest live
    Replay(replay::ReplayArgs),
    /// Turn text into speech and save it
    Speak(media::SpeakArgs),
    /// Turn speech in an audio file into text
    Transcribe(media::TranscribeArgs),
    /// Notes on a meeting or call recording, or a summary of a video:
    /// speakers, decisions, action items, chapters
    Notes(media::NotesArgs),
    /// Generate images from a prompt and save them
    Image(media::ImageArgs),
    /// Chat with an agent in a full-screen terminal UI
    Chat(chat::ChatArgs),
    /// Send one prompt straight to a model (no agent, no tools)
    Ask(ask::AskArgs),
    /// Manage stored agents
    #[command(subcommand)]
    Agents(manage::AgentsCmd),
    /// List and inspect sessions
    #[command(subcommand)]
    Sessions(manage::SessionsCmd),
    /// List and inspect runs
    #[command(subcommand)]
    Runs(manage::RunsCmd),
    /// List the tools agents can use
    Tools,
    /// Add files or folders (text, Markdown, HTML, PDF) to a knowledge collection
    Ingest(knowledge::IngestArgs),
    /// Manage and search knowledge collections
    #[command(subcommand)]
    Collections(knowledge::CollectionsCmd),
    /// Folders, websites, GitHub, Confluence and Notion synced into collections
    #[command(subcommand)]
    Sources(sources::SourcesCmd),
    /// Databases agents query: connections, schemas and queries
    #[command(subcommand)]
    Data(data::DataCmd),
    /// Inspect and edit agents' memory blocks
    #[command(subcommand)]
    Memory(knowledge::MemoryCmd),
    /// List, show and upload Agent Skills
    #[command(subcommand)]
    Skills(knowledge::SkillsCmd),
    /// Schedules, webhooks and file watchers that start runs
    #[command(subcommand)]
    Triggers(triggers::TriggersCmd),
    /// Run and manage multi-agent workflows
    #[command(subcommand)]
    Workflow(workflow::WorkflowCmd),
    /// Evaluate agents on eval sets, compare models and gate CI
    #[command(subcommand)]
    Eval(eval::EvalCmd),
    /// Changes made through the API: who changed what, and when
    Audit(governance::AuditArgs),
    /// Apply data retention (agents' safety.retention_days)
    #[command(subcommand)]
    Retention(governance::RetentionCmd),
    /// Delete everything kept about a user: conversations, runs, files,
    /// memory and ratings
    ForgetUser { user: String },
    /// Export and import .gaibundle files (agents and what they use)
    #[command(subcommand)]
    Bundle(bundle::BundleCmd),
    /// List models from provider catalogs, or ask a provider what it serves
    Models {
        /// Only this provider
        #[arg(long, short)]
        provider: Option<String>,
        /// Query the provider's live model list instead of the catalog
        #[arg(long, requires = "provider")]
        remote: bool,
        #[arg(long)]
        json: bool,
    },
    /// List configured providers and whether their keys are set
    Providers {
        #[arg(long)]
        json: bool,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_new(&cli.log).unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    if let Err(e) = dispatch(cli).await {
        eprintln!("{} {e:#}", red("error:"));
        std::process::exit(1);
    }
}

async fn dispatch(cli: Cli) -> anyhow::Result<()> {
    let gai = Gai::load(cli.config.as_deref())?;
    match cli.command {
        Command::Serve {
            bind,
            ui,
            no_ui,
            auth,
            local,
        } => {
            let dashboard = match ui {
                _ if no_ui => gai_server::Dashboard::Off,
                Some(dir) => gai_server::Dashboard::Dir(dir),
                None => gai_server::Dashboard::Embedded,
            };
            eprintln!(
                "gai {} listening on http://{bind}{}",
                env!("CARGO_PKG_VERSION"),
                match &dashboard {
                    gai_server::Dashboard::Off => "",
                    d if d.available() => "  (dashboard at /)",
                    gai_server::Dashboard::Dir(_) => {
                        "  (no dashboard build in that folder; run `npm run build` in web/)"
                    }
                    _ => "  (built without the dashboard; see / for how to add it)",
                }
            );
            gai_server::serve(
                std::sync::Arc::new(gai),
                gai_server::ServeOptions {
                    bind,
                    dashboard,
                    auth,
                    local,
                },
            )
            .await
            .map_err(|e| anyhow::anyhow!(e))
        }
        Command::Keys(cmd) => manage::keys(&gai, cmd).await,
        Command::Mcp(manage::McpCmd::Serve) => {
            gai_server::mcp_server::serve_stdio(std::sync::Arc::new(gai))
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))
        }
        Command::Mcp(cmd) => manage::mcp(&gai, cmd).await,
        Command::Run(args) => run::run(&gai, args).await,
        Command::Replay(args) => replay::replay(&gai, args).await,
        Command::Speak(args) => media::speak(&gai, args).await,
        Command::Transcribe(args) => media::transcribe(&gai, args).await,
        Command::Notes(args) => media::notes(&gai, args).await,
        Command::Image(args) => media::image(&gai, args).await,
        Command::Chat(args) => chat::chat(&gai, args).await,
        Command::Ask(args) => ask::ask(&gai, args).await,
        Command::Agents(cmd) => manage::agents(&gai, cmd).await,
        Command::Sessions(cmd) => manage::sessions(&gai, cmd).await,
        Command::Runs(cmd) => manage::runs(&gai, cmd).await,
        Command::Tools => manage::tools(&gai).await,
        Command::Ingest(args) => knowledge::ingest(&gai, args).await,
        Command::Collections(cmd) => knowledge::collections(&gai, cmd).await,
        Command::Memory(cmd) => knowledge::memory(&gai, cmd).await,
        Command::Skills(cmd) => knowledge::skills(&gai, cmd).await,
        Command::Workflow(cmd) => workflow::workflow(&gai, cmd).await,
        Command::Triggers(cmd) => triggers::triggers(&gai, cmd).await,
        Command::Bundle(cmd) => bundle::bundle(&gai, cmd).await,
        Command::Eval(cmd) => eval::eval(&gai, cmd).await,
        Command::Data(cmd) => data::run(&gai, cmd).await,
        Command::Orgs(cmd) => orgs::orgs(&gai, cmd).await,
        Command::Members(cmd) => orgs::members(&gai, cmd).await,
        Command::Teams(cmd) => orgs::teams(&gai, cmd).await,
        Command::Sources(cmd) => sources::run(&gai, cmd).await,
        Command::Audit(args) => governance::audit(&gai, args).await,
        Command::Retention(cmd) => governance::retention(&gai, cmd).await,
        Command::ForgetUser { user } => governance::forget_user(&gai, &user).await,
        Command::Models {
            provider,
            remote,
            json,
        } => models(&gai, provider, remote, json).await,
        Command::Providers { json } => providers(&gai, json),
    }
}

async fn models(
    gai: &Gai,
    provider: Option<String>,
    remote: bool,
    json: bool,
) -> anyhow::Result<()> {
    if remote {
        let p = provider.expect("clap requires provider");
        let list = gai.models().discover(&p).await?;
        if json {
            println!("{}", serde_json::to_string_pretty(&list)?);
        } else {
            for m in list {
                let ctx = m
                    .context_window
                    .map(|c| format!("  {}k ctx", c / 1000))
                    .unwrap_or_default();
                println!("{p}/{}{ctx}", m.id);
            }
        }
        return Ok(());
    }
    let entries: Vec<_> = gai
        .models()
        .catalog()
        .into_iter()
        .filter(|e| provider.as_deref().is_none_or(|p| e.info.provider == p))
        .collect();
    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }
    let width = entries.iter().map(|e| e.info.id.len()).max().unwrap_or(10);
    for e in &entries {
        let c = &e.info.capabilities;
        let ctx = c.context_window.map(|n| {
            if n >= 1_000_000 {
                format!("{}M", n / 1_000_000)
            } else {
                format!("{}k", n / 1000)
            }
        });
        let price = e
            .info
            .pricing
            .map(|p| format!("${}/${}", p.input, p.output))
            .unwrap_or_default();
        let ready = match &e.key {
            KeyState::Missing(var) => yellow(&format!("needs {var}")),
            _ => green("ready"),
        };
        println!(
            "{:<width$}  {:>5}  {:<12} {}",
            e.info.id,
            ctx.unwrap_or_default(),
            price,
            ready
        );
    }
    if provider.is_none() {
        eprintln!(
            "{}",
            dim(
                "Any model a provider serves works, listed or not: `gai ask -m ollama/qwen3:8b ...`. See live lists with `gai models -p <provider> --remote`."
            )
        );
    }
    Ok(())
}

fn providers(gai: &Gai, json: bool) -> anyhow::Result<()> {
    if json {
        let rows: Vec<_> = gai
            .models()
            .providers()
            .map(|(name, c)| {
                serde_json::json!({
                    "name": name,
                    "dialect": c.dialect,
                    "base_url": c.base_url,
                    "key": KeyState::from(c.key_status()),
                    "models": c.models.len(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    for (name, c) in gai.models().providers() {
        let key = match KeyState::from(c.key_status()) {
            KeyState::Missing(var) => yellow(&format!("needs {var}")),
            KeyState::Present => green("key set"),
            KeyState::NotNeeded => green("no key needed"),
        };
        println!(
            "{:<11} {:<10} {:<55} {}",
            name,
            c.dialect,
            c.base_url.as_deref().unwrap_or("-"),
            key
        );
    }
    if let Some(p) = gai.config_path() {
        eprintln!("{}", dim(&format!("config: {}", p.display())));
    }
    Ok(())
}
