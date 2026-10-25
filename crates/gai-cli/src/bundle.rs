//! `gai bundle`: export and import `.gaibundle` files.

use std::path::PathBuf;

use clap::Subcommand;
use gai::Gai;
use gai::bundle::{ExportOptions, ImportOptions, ImportReport};

use crate::style::{bold, dim, green, yellow};

#[derive(Subcommand)]
pub enum BundleCmd {
    /// Write agents and what they use (personas, skills, delegates,
    /// knowledge, MCP servers) to a .gaibundle file
    Export {
        /// The file to write
        #[arg(long, short, default_value = "gai.gaibundle")]
        output: PathBuf,
        /// Stored agents to include (repeat the flag)
        #[arg(long = "agent", short)]
        agents: Vec<String>,
        #[arg(long = "workflow", short)]
        workflows: Vec<String>,
        #[arg(long = "persona")]
        personas: Vec<String>,
        #[arg(long = "skill")]
        skills: Vec<String>,
        /// MCP servers added through the API
        #[arg(long = "mcp")]
        mcp_servers: Vec<String>,
        /// Knowledge collections, with their documents
        #[arg(long = "knowledge", short)]
        knowledge: Vec<String>,
        /// Everything stored (add --all-knowledge for every collection)
        #[arg(long)]
        all: bool,
        #[arg(long, requires = "all")]
        all_knowledge: bool,
        /// Only the items named, not what they use
        #[arg(long)]
        no_deps: bool,
        /// Shown to whoever imports it
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        description: Option<String>,
    },
    /// Show what a bundle holds and what it needs, without importing it
    Inspect { file: PathBuf },
    /// Install a bundle's items. Existing ones get a new version.
    Import {
        file: PathBuf,
        /// Leave items that already exist alone
        #[arg(long)]
        skip_existing: bool,
    },
}

pub async fn bundle(gai: &Gai, cmd: BundleCmd) -> anyhow::Result<()> {
    match cmd {
        BundleCmd::Export {
            output,
            agents,
            workflows,
            personas,
            skills,
            mcp_servers,
            knowledge,
            all,
            all_knowledge,
            no_deps,
            name,
            description,
        } => {
            let opts = ExportOptions {
                name,
                description,
                agents,
                workflows,
                personas,
                skills,
                mcp_servers,
                knowledge,
                all,
                knowledge_all: all_knowledge,
                no_dependencies: no_deps,
            };
            let bytes = gai::bundle::export(gai, &opts).await?;
            std::fs::write(&output, &bytes)?;
            let report = gai::bundle::inspect(gai, &bytes).await?;
            eprintln!(
                "{} {} ({} KB)",
                green("wrote"),
                output.display(),
                bytes.len().div_ceil(1024)
            );
            print_contents(&report);
            if !report.manifest.requires.env.is_empty() {
                eprintln!(
                    "\n{}",
                    dim("Whoever imports it needs these set (no secrets are in the file):")
                );
                for v in &report.manifest.requires.env {
                    eprintln!("  {}  {}", v.name, dim(&v.used_by));
                }
            }
        }
        BundleCmd::Inspect { file } => {
            let report = gai::bundle::inspect(gai, &std::fs::read(&file)?).await?;
            print_contents(&report);
            print_needs(&report);
        }
        BundleCmd::Import {
            file,
            skip_existing,
        } => {
            let report = gai::bundle::import(
                gai,
                &std::fs::read(&file)?,
                &ImportOptions { skip_existing },
            )
            .await?;
            eprintln!("{} {}", green("imported"), bold(&report.manifest.name));
            for i in &report.installed {
                eprintln!("  + {i}");
            }
            for i in &report.skipped {
                eprintln!("  {}", dim(&format!("= {i} (already here)")));
            }
            for w in &report.warnings {
                eprintln!("  {} {w}", yellow("!"));
            }
            print_needs(&report);
        }
    }
    Ok(())
}

fn print_contents(r: &ImportReport) {
    let m = &r.manifest;
    eprintln!(
        "{}{}",
        bold(&m.name),
        m.description
            .as_deref()
            .map(|d| format!(" - {d}"))
            .unwrap_or_default()
    );
    eprintln!(
        "{}",
        dim(&format!(
            "written by gai {} on {}",
            m.gai_version, m.created_at
        ))
    );
    let c = &m.contents;
    for (what, items) in [
        ("agents", &c.agents),
        ("workflows", &c.workflows),
        ("personas", &c.personas),
        ("skills", &c.skills),
        ("MCP servers", &c.mcp_servers),
        ("knowledge", &c.knowledge),
    ] {
        if !items.is_empty() {
            eprintln!("  {what}: {}", items.join(", "));
        }
    }
    if c.memory_blocks > 0 {
        eprintln!("  memory blocks: {}", c.memory_blocks);
    }
}

fn print_needs(r: &ImportReport) {
    if !r.missing_env.is_empty() {
        eprintln!("\n{}", yellow("Set these before using it:"));
        for v in &r.missing_env {
            eprintln!("  export {}=...  {}", v.name, dim(&v.used_by));
        }
    }
    if !r.missing_models.is_empty() {
        eprintln!(
            "\n{} {}",
            yellow("Models this server can't reach:"),
            r.missing_models.join(", ")
        );
    }
}
