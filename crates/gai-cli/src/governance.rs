//! `gai audit`, `gai retention` and `gai forget-user`.

use clap::{Args, Subcommand};
use gai::Gai;
use gai::store::AuditFilter;

use crate::style::{dim, green, red};

#[derive(Args)]
pub struct AuditArgs {
    /// Only changes by this API key name
    #[arg(long)]
    actor: Option<String>,
    /// Only targets starting with this, e.g. agents/ or keys/
    #[arg(long)]
    target: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: u32,
    /// Print CSV
    #[arg(long)]
    csv: bool,
}

#[derive(Subcommand)]
pub enum RetentionCmd {
    /// Delete conversations older than each agent's retention_days now
    Sweep,
}

pub async fn audit(gai: &Gai, a: AuditArgs) -> anyhow::Result<()> {
    let rows = gai
        .runtime()
        .await?
        .store()
        .list_audit(AuditFilter {
            actor: a.actor,
            target: a.target,
            since: None,
            until: None,
            limit: a.limit,
        })
        .await?;
    if a.csv {
        println!("id,at,actor,action,target,status,detail");
        for r in rows {
            println!(
                "{},{},{:?},{:?},{:?},{},{:?}",
                r.id,
                r.at.to_rfc3339(),
                r.actor,
                r.action,
                r.target,
                r.status,
                r.detail.to_string()
            );
        }
        return Ok(());
    }
    if rows.is_empty() {
        eprintln!(
            "{}",
            dim("Nothing recorded yet. Changes made through the API and dashboard show up here.")
        );
    }
    for r in rows {
        let status = if r.status < 400 {
            green(&r.status.to_string())
        } else {
            red(&r.status.to_string())
        };
        let detail = if r.detail.as_object().is_some_and(|o| o.is_empty()) {
            String::new()
        } else {
            r.detail.to_string()
        };
        println!(
            "{}  {:<10} {} {}  {}",
            dim(&r.at.format("%Y-%m-%d %H:%M:%S").to_string()),
            r.actor,
            status,
            r.action,
            dim(&detail)
        );
    }
    Ok(())
}

pub async fn retention(gai: &Gai, cmd: RetentionCmd) -> anyhow::Result<()> {
    match cmd {
        RetentionCmd::Sweep => {
            let done = gai.sweep_retention().await?;
            if done.is_empty() {
                eprintln!(
                    "{}",
                    dim(
                        "Nothing to delete (no agent has expired conversations, or none sets retention_days)."
                    )
                );
            }
            for (agent, d) in done {
                eprintln!(
                    "{} {agent}: {} conversations, {} runs, {} files",
                    green("deleted"),
                    d.sessions,
                    d.runs,
                    d.blobs
                );
            }
        }
    }
    Ok(())
}

pub async fn forget_user(gai: &Gai, user: &str) -> anyhow::Result<()> {
    let d = gai.forget_user(user).await?;
    eprintln!(
        "{} {user}: {} conversations, {} runs, {} files, {} memory blocks, {} ratings",
        green("forgot"),
        d.sessions,
        d.runs,
        d.blobs,
        d.memory_blocks,
        d.feedback
    );
    Ok(())
}
