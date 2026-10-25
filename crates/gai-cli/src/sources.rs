//! `gai sources`: outside content synced into knowledge collections.

use std::path::PathBuf;

use clap::Subcommand;
use gai::Gai;

use crate::style::{dim, green, red};

#[derive(Subcommand)]
pub enum SourcesCmd {
    /// List sources and their last sync
    List,
    /// Show a source: its spec, status and counts
    Show { name: String },
    /// Save sources from a YAML or JSON file (one, or a list)
    Save { file: PathBuf },
    /// Sync a source now: only new, changed and removed items
    Sync { name: String },
    /// Delete a source (its documents stay in the collection)
    Delete { name: String },
}

pub async fn run(gai: &Gai, cmd: SourcesCmd) -> anyhow::Result<()> {
    let sources = gai.sources().await?;
    let store = gai.runtime().await?.store().clone();
    match cmd {
        SourcesCmd::List => {
            let rows = store.list_sources().await?;
            if rows.is_empty() {
                eprintln!(
                    "{}",
                    dim("No sources yet. Save one with gai sources save <file.yaml>.")
                );
            }
            for r in rows {
                let status = match r.status.as_str() {
                    "ok" => green(&r.status),
                    "failed" => red(&r.status),
                    _ => r.status.clone(),
                };
                let when = r
                    .last_sync_at
                    .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "never".into());
                println!(
                    "{:<20} {:<10} → {:<16} {:<8} {}",
                    r.name,
                    r.kind,
                    r.collection,
                    status,
                    dim(&when)
                );
            }
        }
        SourcesCmd::Show { name } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&sources.get(&name).await?)?
            );
        }
        SourcesCmd::Save { file } => {
            let text = std::fs::read_to_string(&file)?;
            let value: serde_json::Value = serde_yaml::from_str(&text)?;
            let specs = match value {
                serde_json::Value::Array(a) => a,
                v => vec![v],
            };
            for v in specs {
                let spec = gai::memory::sources::Sources::parse(v)?;
                sources.save(&spec).await?;
                println!("{} {} → {}", green("saved"), spec.name, spec.collection);
            }
        }
        SourcesCmd::Sync { name } => {
            let s = sources.sync(&name).await?;
            println!(
                "{name}: {} added, {} updated, {} removed, {} unchanged, {} failed",
                s.added, s.updated, s.removed, s.unchanged, s.failed
            );
            for e in &s.errors {
                eprintln!("  {}", red(e));
            }
        }
        SourcesCmd::Delete { name } => {
            if !store.delete_source(&name).await? {
                anyhow::bail!("no source {name}");
            }
            println!("deleted {name}");
        }
    }
    Ok(())
}
