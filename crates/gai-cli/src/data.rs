//! `gai data`: database connections, their schemas, and queries.

use std::path::PathBuf;

use clap::Subcommand;
use gai::Gai;
use gai::data::Data;

use crate::style::{dim, green};

#[derive(Subcommand)]
pub enum DataCmd {
    /// List database connections
    Connections,
    /// Save connections from a YAML or JSON file (one, or a list); reads
    /// each one's schema to check it
    Save { file: PathBuf },
    /// Delete a connection
    Delete { name: String },
    /// Show a connection's tables and columns
    Schema {
        name: String,
        /// Read it from the database again
        #[arg(long)]
        refresh: bool,
    },
    /// Run one read-only query
    Query {
        name: String,
        sql: String,
        #[arg(long)]
        limit: Option<usize>,
        /// Print CSV instead of a table
        #[arg(long)]
        csv: bool,
        /// Only check it and print the plan
        #[arg(long)]
        dry_run: bool,
    },
}

pub async fn run(gai: &Gai, cmd: DataCmd) -> anyhow::Result<()> {
    let data = gai.data().await?;
    match cmd {
        DataCmd::Connections => {
            let list = data.list().await?;
            if list.is_empty() {
                eprintln!(
                    "{}",
                    dim("No connections yet. Save one with gai data save <file.yaml>.")
                );
            }
            for c in list {
                let tables = c
                    .schema
                    .as_ref()
                    .and_then(|s| s["tables"].as_array().map(Vec::len));
                println!(
                    "{:<20} {:<10} {}{}",
                    c.name,
                    c.kind,
                    c.spec["description"].as_str().unwrap_or(""),
                    dim(&tables
                        .map(|n| format!("  ({n} tables)"))
                        .unwrap_or_default())
                );
            }
        }
        DataCmd::Save { file } => {
            let v: serde_json::Value = serde_yaml::from_str(&std::fs::read_to_string(&file)?)?;
            let specs = match v {
                serde_json::Value::Array(a) => a,
                v => vec![v],
            };
            for v in specs {
                let spec = Data::parse(v)?;
                data.save(&spec).await?;
                let s = data.schema(&spec.name, true).await?;
                println!(
                    "{} {} ({} tables)",
                    green("saved"),
                    spec.name,
                    s.tables.len()
                );
            }
        }
        DataCmd::Delete { name } => {
            if !data.delete(&name).await? {
                anyhow::bail!("no connection {name}");
            }
            println!("deleted {name}");
        }
        DataCmd::Schema { name, refresh } => {
            print!("{}", data.schema(&name, refresh).await?.describe(&[]));
        }
        DataCmd::Query {
            name,
            sql,
            limit,
            csv,
            dry_run,
        } => {
            if dry_run {
                println!("{}", data.dry_run(&name, &sql).await?);
                return Ok(());
            }
            let r = data.query(&name, &sql, limit).await?;
            if csv {
                print!("{}", r.csv());
            } else {
                print!("{}", r.markdown(usize::MAX));
                eprintln!(
                    "{}",
                    dim(&format!(
                        "{} rows in {} ms{}",
                        r.rows.len(),
                        r.elapsed_ms,
                        if r.truncated {
                            " (more were left out: raise --limit or max_rows)"
                        } else {
                            ""
                        }
                    ))
                );
            }
        }
    }
    Ok(())
}
