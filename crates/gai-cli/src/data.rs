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
    /// List dashboards
    Dashboards,
    /// Save a dashboard from a YAML or JSON file, and refresh it
    SaveDashboard { file: PathBuf },
    /// Run a dashboard's queries now (and send alerts that start firing)
    Refresh { name: String },
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
        DataCmd::Dashboards => {
            for d in gai.dashboards().await?.list().await? {
                let at = d
                    .results
                    .as_ref()
                    .and_then(|r| r.refreshed_at)
                    .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "never".into());
                let alerting = d
                    .results
                    .as_ref()
                    .map(|r| r.panels.iter().filter(|p| p.alerting).count())
                    .unwrap_or(0);
                println!(
                    "{:<20} {:<28} {} panels  {}{}",
                    d.spec.name,
                    d.spec.title,
                    d.spec.panels.len(),
                    dim(&format!("refreshed {at}")),
                    if alerting > 0 {
                        format!("  {alerting} alerting")
                    } else {
                        String::new()
                    }
                );
            }
        }
        DataCmd::SaveDashboard { file } => {
            let v: serde_json::Value = serde_yaml::from_str(&std::fs::read_to_string(&file)?)?;
            let d = gai.dashboards().await?;
            let spec = gai::data::dashboards::Dashboards::parse(v)?;
            d.save(&spec).await?;
            let r = d.refresh(&spec.name).await?;
            let failed = r.panels.iter().filter(|p| p.error.is_some()).count();
            println!(
                "{} {} ({} panels, {failed} failed)",
                green("saved"),
                spec.name,
                r.panels.len()
            );
        }
        DataCmd::Refresh { name } => {
            let r = gai.dashboards().await?.refresh(&name).await?;
            for (i, p) in r.panels.iter().enumerate() {
                match &p.error {
                    Some(e) => println!("{i}: failed: {e}"),
                    None => println!(
                        "{i}: {} rows{}",
                        p.rows.len(),
                        p.value
                            .map(|v| format!(
                                ", value {v}{}",
                                if p.alerting { " (alerting)" } else { "" }
                            ))
                            .unwrap_or_default()
                    ),
                }
            }
            for a in &r.alerts {
                println!("alert sent: {a}");
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
