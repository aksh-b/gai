//! Listing and managing agents, sessions, runs and tools.

use std::path::PathBuf;

use clap::Subcommand;
use gai::Gai;
use gai::agent::AgentSpec;
use gai::store::{RunFilter, RunStatus};

use crate::style::{age, cyan, dim, green, one_line, red, yellow};

#[derive(Subcommand)]
pub enum AgentsCmd {
    /// List stored agents and agent files
    List,
    /// Print an agent's spec as YAML
    Show {
        reference: String,
    },
    /// Store an agent file as a new version
    Save {
        file: PathBuf,
        #[arg(long)]
        note: Option<String>,
    },
    /// List an agent's versions
    Versions {
        name: String,
    },
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
    Delete {
        name: String,
    },
}

pub async fn agents(gai: &Gai, cmd: AgentsCmd) -> anyhow::Result<()> {
    let rt = gai.runtime().await?;
    let store = rt.store();
    match cmd {
        AgentsCmd::List => {
            let stored = store.list_agents().await?;
            for a in &stored {
                let labels: Vec<String> =
                    a.labels.iter().map(|(l, v)| format!("{l}=v{v}")).collect();
                println!(
                    "{:<24} v{:<4} {} {}",
                    a.name,
                    a.latest_version,
                    dim(&age(a.updated_at)),
                    if labels.is_empty() {
                        String::new()
                    } else {
                        cyan(&labels.join(" "))
                    }
                );
            }
            for dir in gai.config().agents_dirs() {
                let Ok(rd) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for e in rd.flatten() {
                    let p = e.path();
                    if matches!(
                        p.extension().and_then(|x| x.to_str()),
                        Some("yaml" | "yml" | "toml" | "json")
                    ) {
                        println!(
                            "{:<24} {}",
                            p.file_stem().unwrap_or_default().to_string_lossy(),
                            dim(&format!("file {}", p.display()))
                        );
                    }
                }
            }
            if stored.is_empty() {
                eprintln!(
                    "{}",
                    dim("No stored agents. Save one with `gai agents save agents/my-agent.yaml`.")
                );
            }
        }
        AgentsCmd::Show { reference } => {
            let (spec, version) = gai.agent(&reference).await?;
            if let Some(v) = version {
                eprintln!("{}", dim(&format!("# {} v{v}", spec.name)));
            }
            print!("{}", serde_yaml::to_string(&spec)?);
        }
        AgentsCmd::Save { file, note } => {
            let spec = AgentSpec::load(&file).map_err(anyhow::Error::msg)?;
            let v = rt.save_agent(&spec, note.as_deref()).await?;
            println!("{} {} v{v}", green("saved"), spec.name);
        }
        AgentsCmd::Versions { name } => {
            for v in store.list_agent_versions(&name).await? {
                println!(
                    "v{:<4} {} {}",
                    v.version,
                    dim(&age(v.created_at)),
                    v.note.unwrap_or_default()
                );
            }
        }
        AgentsCmd::Label {
            name,
            label,
            version,
            remove,
        } => {
            if !remove && version.is_none() {
                anyhow::bail!("give a version to label, or --remove");
            }
            store
                .set_agent_label(&name, &label, if remove { None } else { version })
                .await?;
            println!("{}", green("ok"));
        }
        AgentsCmd::Diff { name, from, to } => {
            let latest = store
                .get_agent(&name, None)
                .await?
                .ok_or_else(|| anyhow::anyhow!("no stored agent {name}"))?
                .version;
            let to = to.unwrap_or(latest);
            let from = from.unwrap_or(to.saturating_sub(1).max(1));
            let get = |v: u32| {
                let name = name.clone();
                async move {
                    store
                        .get_agent(&name, Some(v))
                        .await?
                        .ok_or_else(|| anyhow::anyhow!("agent {name} has no v{v}"))
                }
            };
            let (a, b) = (get(from).await?, get(to).await?);
            print_diff(&gai_server::unified_diff(
                &gai_server::spec_yaml(&a.spec),
                &gai_server::spec_yaml(&b.spec),
                &format!("{name}@v{from}"),
                &format!("{name}@v{to}"),
            ));
        }
        AgentsCmd::Rollback {
            name,
            version,
            note,
        } => {
            let old = store
                .get_agent(&name, Some(version))
                .await?
                .ok_or_else(|| anyhow::anyhow!("agent {name} has no v{version}"))?;
            let note = note.unwrap_or_else(|| format!("rollback to v{version}"));
            let v = store.put_agent(&name, &old.spec, Some(&note)).await?;
            println!(
                "{} {name} v{v} {}",
                green("saved"),
                dim(&format!("(= v{version})"))
            );
        }
        AgentsCmd::Delete { name } => {
            if store.delete_agent(&name).await? {
                println!("{} {name}", green("deleted"));
            } else {
                anyhow::bail!("no stored agent {name}");
            }
        }
    }
    Ok(())
}

/// Print a unified diff with colours, or say there is no difference.
pub fn print_diff(diff: &str) {
    if diff.is_empty() {
        println!("{}", dim("no differences"));
        return;
    }
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            println!("{}", dim(line));
        } else if line.starts_with('+') {
            println!("{}", green(line));
        } else if line.starts_with('-') {
            println!("{}", red(line));
        } else if line.starts_with("@@") {
            println!("{}", cyan(line));
        } else {
            println!("{line}");
        }
    }
}

#[derive(Subcommand)]
pub enum SessionsCmd {
    List {
        #[arg(long, short)]
        agent: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Print a session's transcript
    Show {
        id: String,
    },
    Delete {
        id: String,
    },
}

pub async fn sessions(gai: &Gai, cmd: SessionsCmd) -> anyhow::Result<()> {
    let store = gai.runtime().await?.store().clone();
    match cmd {
        SessionsCmd::List { agent, limit } => {
            for s in store.list_sessions(agent.as_deref(), limit).await? {
                println!(
                    "{}  {:<16} {} {}",
                    s.id,
                    s.agent.unwrap_or_default(),
                    dim(&format!("{:>8}", age(s.updated_at))),
                    one_line(&s.title.unwrap_or_default(), 60)
                );
            }
        }
        SessionsCmd::Show { id } => {
            for m in store.list_messages(&id).await? {
                let who = match m.message.role {
                    gai::core::Role::User => cyan("user"),
                    gai::core::Role::Assistant => green("assistant"),
                };
                for b in &m.message.content {
                    match b {
                        gai::core::ContentBlock::Text { text } => println!("{who}: {text}\n"),
                        gai::core::ContentBlock::ToolUse { name, input, .. } => {
                            println!(
                                "{who}: {} {}\n",
                                cyan(&format!("⚙ {name}")),
                                dim(&one_line(&input.to_string(), 120))
                            )
                        }
                        gai::core::ContentBlock::ToolResult {
                            content, is_error, ..
                        } => {
                            let t: String = content.iter().filter_map(|c| c.as_text()).collect();
                            let mark = if *is_error { red("✗") } else { green("✓") };
                            println!("  {mark} {}\n", dim(&one_line(&t, 140)));
                        }
                        _ => {}
                    }
                }
            }
        }
        SessionsCmd::Delete { id } => {
            if !store.delete_session(&id).await? {
                anyhow::bail!("no session {id}");
            }
            println!("{} {id}", green("deleted"));
        }
    }
    Ok(())
}

#[derive(Subcommand)]
pub enum RunsCmd {
    List {
        #[arg(long)]
        session: Option<String>,
        #[arg(long, short)]
        agent: Option<String>,
        #[arg(long)]
        status: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Continue a run that stopped without finishing (e.g. the process
    /// exited while it waited for approval)
    Resume {
        id: String,
        #[arg(long)]
        show_reasoning: bool,
    },
    /// Show a run's outcome and step journal
    Show {
        id: String,
        /// Also print the stored events
        #[arg(long)]
        events: bool,
    },
}

pub async fn runs(gai: &Gai, cmd: RunsCmd) -> anyhow::Result<()> {
    let store = gai.runtime().await?.store().clone();
    match cmd {
        RunsCmd::List {
            session,
            agent,
            status,
            limit,
        } => {
            let status = status
                .map(|s| RunStatus::parse(&s).ok_or_else(|| anyhow::anyhow!("unknown status {s}")))
                .transpose()?;
            let runs = store
                .list_runs(RunFilter {
                    session_id: session,
                    agent,
                    status,
                    limit: Some(limit),
                    ..Default::default()
                })
                .await?;
            for r in runs {
                let st = match r.status {
                    RunStatus::Completed => green(r.status.as_str()),
                    RunStatus::Running | RunStatus::Queued | RunStatus::AwaitingApproval => {
                        yellow(r.status.as_str())
                    }
                    _ => red(r.status.as_str()),
                };
                let input: String = serde_json::from_value::<Vec<gai::core::ContentBlock>>(r.input)
                    .map(|b| b.iter().filter_map(|x| x.as_text()).collect())
                    .unwrap_or_default();
                println!(
                    "{}  {:<14} {:<18} {} {}",
                    r.id,
                    r.agent,
                    st,
                    dim(&format!("{:>8} ${:.4}", age(r.created_at), r.cost)),
                    one_line(&input, 50)
                );
            }
        }
        RunsCmd::Resume { id, show_reasoning } => {
            let rt = gai.runtime().await?;
            let handle = rt.resume(&id).await?;
            return crate::run::follow(rt, handle, show_reasoning, false).await;
        }
        RunsCmd::Show { id, events } => {
            let run = store
                .get_run(&id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("no run {id}"))?;
            let mut summary = serde_json::to_value(&run)?;
            if let Some(m) = summary.get_mut("metadata").and_then(|m| m.as_object_mut()) {
                m.remove("spec");
            }
            println!("{}", serde_json::to_string_pretty(&summary)?);
            println!("{}", dim("steps:"));
            for s in store.list_steps(&id).await? {
                let took = s
                    .finished_at
                    .map(|f| format!("{} ms", (f - s.started_at).num_milliseconds()))
                    .unwrap_or_else(|| "…".into());
                println!(
                    "  {:>3} {:<10} {:<28} {:<9} {}",
                    s.seq,
                    format!("{:?}", s.kind).to_lowercase(),
                    s.name.unwrap_or_default(),
                    format!("{:?}", s.status).to_lowercase(),
                    dim(&took)
                );
            }
            if events {
                for e in store.list_events(&id, None, 10_000).await? {
                    println!("{}", serde_json::to_string(&e.event)?);
                }
            }
        }
    }
    Ok(())
}

pub async fn tools(gai: &Gai) -> anyhow::Result<()> {
    let rt = gai.runtime().await?;
    for (spec, policy) in rt.tool_specs() {
        let p = format!("{policy:?}").to_lowercase();
        println!(
            "{:<14} {:<8} {}",
            cyan(&spec.name),
            dim(&p),
            one_line(&spec.description, 90)
        );
    }
    Ok(())
}

#[derive(Subcommand)]
pub enum KeysCmd {
    /// Create a key. The secret is printed once.
    Create {
        #[arg(long)]
        name: String,
        /// admin, run or read (repeatable)
        #[arg(long = "scope", default_value = "run", value_parser = parse_scope)]
        scopes: Vec<gai::store::Scope>,
        /// Stop the key from starting new work after spending this much (USD)
        #[arg(long)]
        budget: Option<f64>,
    },
    List,
    Revoke {
        id: String,
    },
}

fn parse_scope(s: &str) -> Result<gai::store::Scope, String> {
    serde_json::from_value(serde_json::Value::String(s.to_lowercase()))
        .map_err(|_| format!("unknown scope {s:?}; use admin, run or read"))
}

pub async fn keys(gai: &Gai, cmd: KeysCmd) -> anyhow::Result<()> {
    let store = gai.runtime().await?.store().clone();
    match cmd {
        KeysCmd::Create {
            name,
            scopes,
            budget,
        } => {
            let k = gai::store::keys::generate();
            let key = store
                .create_key(&name, &k.prefix, &k.hash, &scopes, budget)
                .await?;
            println!("{}", k.secret);
            eprintln!(
                "{}",
                dim(&format!(
                    "created {} ({}) — this is the only time the key is shown",
                    key.id,
                    scopes
                        .iter()
                        .map(|s| format!("{s:?}").to_lowercase())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            );
        }
        KeysCmd::List => {
            for k in store.list_keys().await? {
                let state = if k.revoked_at.is_some() {
                    red("revoked")
                } else {
                    green("active")
                };
                let budget = k
                    .budget_usd
                    .map(|b| format!(" / ${b:.2}"))
                    .unwrap_or_default();
                println!(
                    "{}  {:<16} {:<10} {}…  {:<16} ${:.4}{budget}  {}",
                    k.id,
                    k.name,
                    state,
                    k.prefix,
                    k.scopes
                        .iter()
                        .map(|s| format!("{s:?}").to_lowercase())
                        .collect::<Vec<_>>()
                        .join(","),
                    k.spent_usd,
                    dim(&k
                        .last_used_at
                        .map(|t| format!("used {}", age(t)))
                        .unwrap_or_else(|| "never used".into()))
                );
            }
        }
        KeysCmd::Revoke { id } => {
            if !store.revoke_key(&id).await? {
                anyhow::bail!("no active key {id}");
            }
            println!("{} {id}", green("revoked"));
        }
    }
    Ok(())
}

#[derive(Subcommand)]
pub enum McpCmd {
    /// Connect every MCP server and list its status and tools
    List,
    /// Add an MCP server (stored; `gai serve` picks it up on restart).
    /// Stdio: `gai mcp add fs -- npx -y @modelcontextprotocol/server-filesystem /data`.
    /// HTTP: `gai mcp add search --url https://host/mcp --header "Authorization=Bearer ${TOKEN}"`.
    Add {
        name: String,
        #[arg(long, conflicts_with = "command")]
        url: Option<String>,
        /// HTTP header as Name=Value (repeatable; values may use ${VAR})
        #[arg(long = "header", value_parser = parse_kv)]
        headers: Vec<(String, String)>,
        /// Environment for a stdio server as NAME=value (repeatable)
        #[arg(long = "env", value_parser = parse_kv)]
        env: Vec<(String, String)>,
        /// Policy for its tools: auto, approve or deny
        #[arg(long)]
        policy: Option<String>,
        /// Program and arguments of a stdio server, after `--`
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Remove an MCP server added with `gai mcp add`
    Remove { name: String },
    /// Be an MCP server on stdin/stdout: agents and workflows as tools,
    /// knowledge collections as resources (see [server.mcp] in gai.toml)
    Serve,
}

fn parse_kv(s: &str) -> Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.trim().to_owned(), v.to_owned()))
        .ok_or_else(|| format!("expected NAME=value, got {s:?}"))
}

pub async fn mcp(gai: &Gai, cmd: McpCmd) -> anyhow::Result<()> {
    use gai::tools::mcp::{McpServer, McpServerConfig, McpStatus};
    let rt = gai.runtime().await?;
    match cmd {
        McpCmd::List => {
            rt.ensure_mcp();
            rt.mcp()
                .wait_settled(std::time::Duration::from_secs(60))
                .await;
            let servers = rt.mcp_servers();
            if servers.is_empty() {
                eprintln!(
                    "{}",
                    dim(
                        "No MCP servers. Add one with `gai mcp add` or [mcp_servers.<name>] in gai.toml."
                    )
                );
            }
            for s in servers {
                let target = s.config.url.clone().unwrap_or_else(|| {
                    std::iter::once(s.config.command.clone().unwrap_or_default())
                        .chain(s.config.args.iter().cloned())
                        .collect::<Vec<_>>()
                        .join(" ")
                });
                let status = match &s.status {
                    McpStatus::Ready => green("ready"),
                    McpStatus::Failed(e) => red(&format!("failed: {e}")),
                    McpStatus::NeedsAuth(_) => {
                        yellow("needs sign-in (use the dashboard's Tools page)")
                    }
                    McpStatus::Disabled => dim("disabled"),
                    McpStatus::Connecting => yellow("connecting"),
                };
                println!(
                    "{} {} {}  {}",
                    cyan(&s.name),
                    dim(&format!("({})", s.source)),
                    status,
                    dim(&target)
                );
                for t in &s.tools {
                    println!("    {t}");
                }
            }
        }
        McpCmd::Add {
            name,
            url,
            headers,
            env,
            policy,
            command,
        } => {
            let policy = policy
                .map(|p| {
                    serde_json::from_value(serde_json::Value::String(p.clone()))
                        .map_err(|_| anyhow::anyhow!("unknown policy {p}"))
                })
                .transpose()?;
            let config = McpServerConfig {
                command: command.first().cloned(),
                args: command.iter().skip(1).cloned().collect(),
                env: env.into_iter().collect(),
                url,
                headers: headers.into_iter().collect(),
                policy,
                ..Default::default()
            };
            config.validate().map_err(anyhow::Error::msg)?;
            if gai.config().mcp_servers.contains_key(&name) {
                anyhow::bail!("{name} is declared in gai.toml; change it there");
            }
            rt.store()
                .put_mcp_server(&name, &serde_json::to_value(&config)?)
                .await?;
            let server = McpServer::new(&name, config, "api");
            match rt.connect_mcp(server).await {
                Ok(n) => println!("{} {name}: {n} tools", green("added")),
                Err(e) => println!("{} {name}, but it did not connect: {e}", yellow("added")),
            }
        }
        McpCmd::Remove { name } => {
            if !rt.store().delete_mcp_server(&name).await? {
                anyhow::bail!("no stored MCP server {name}");
            }
            rt.remove_mcp(&name).await;
            println!("{} {name}", green("removed"));
        }
        McpCmd::Serve => unreachable!("`gai mcp serve` is handled before this"),
    }
    Ok(())
}
