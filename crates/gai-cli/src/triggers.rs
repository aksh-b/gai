//! `gai triggers`: schedules, webhooks and file watchers.

use std::path::PathBuf;

use clap::Subcommand;
use gai::Gai;
use gai::triggers::{TriggerSpec, Triggers};

use crate::style::{age, cyan, dim, green, red, yellow};

#[derive(Subcommand)]
pub enum TriggersCmd {
    /// List triggers with their next and last firing
    List,
    /// Print a trigger as YAML
    Show {
        trigger: String,
    },
    /// Create a trigger from a YAML file, or replace one with the same name
    Save {
        file: PathBuf,
    },
    /// Delete a trigger
    Delete {
        trigger: String,
    },
    Enable {
        trigger: String,
    },
    Disable {
        trigger: String,
    },
    /// Fire a trigger now and follow its run here
    Fire {
        trigger: String,
        /// Payload for the templates, as JSON
        #[arg(long)]
        payload: Option<String>,
    },
    /// What a trigger did, newest first
    Events {
        trigger: String,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// A webhook trigger's signing secret
    Secret {
        trigger: String,
    },
}

pub async fn triggers(gai: &Gai, cmd: TriggersCmd) -> anyhow::Result<()> {
    let t = gai.triggers().await?;
    let store = gai.runtime().await?.store().clone();
    match cmd {
        TriggersCmd::List => {
            let all = store.list_triggers().await?;
            if all.is_empty() {
                eprintln!(
                    "{}",
                    dim(
                        "No triggers. Write one in YAML and run `gai triggers save my-trigger.yaml`."
                    )
                );
            }
            for r in all {
                let spec = Triggers::spec_of(&r).ok();
                let when = match (&r.next_fire_at, r.kind.as_str()) {
                    (Some(n), "cron") => format!("next {}", n.format("%Y-%m-%d %H:%M UTC")),
                    (_, "webhook") => format!("POST /v1/webhooks/{}", r.id),
                    _ => spec
                        .as_ref()
                        .and_then(|s| match &s.source {
                            gai::triggers::Source::File { path, .. } => {
                                Some(format!("watching {path}"))
                            }
                            _ => None,
                        })
                        .unwrap_or_default(),
                };
                let target = spec
                    .as_ref()
                    .map(|s| match &s.target {
                        gai::triggers::Target::Agent(a) => format!("agent {a}"),
                        gai::triggers::Target::Workflow(w) => format!("workflow {w}"),
                    })
                    .unwrap_or_default();
                println!(
                    "{:<20} {:<8} {} {:<24} {} {}",
                    r.name,
                    r.kind,
                    if r.enabled {
                        green("on ")
                    } else {
                        yellow("off")
                    },
                    target,
                    cyan(&when),
                    dim(&r
                        .last_fired_at
                        .map(|l| format!("last {}", age(l)))
                        .unwrap_or_default())
                );
            }
        }
        TriggersCmd::Show { trigger } => {
            let r = t.get(&trigger).await?;
            let spec = Triggers::spec_of(&r)?;
            if let Some(d) = spec.describe() {
                eprintln!("{}", dim(&format!("# {d}")));
            }
            print!("{}", serde_yaml::to_string(&spec)?);
        }
        TriggersCmd::Save { file } => {
            let spec =
                TriggerSpec::parse(&std::fs::read_to_string(&file)?).map_err(anyhow::Error::msg)?;
            let rec = match store.get_trigger(&spec.name).await? {
                Some(old) => t.update(&old.id, spec).await?,
                None => {
                    let rec = t.create(spec).await?;
                    if let Some(s) = &rec.secret {
                        println!(
                            "{} POST /v1/webhooks/{} with X-Gai-Token: {s}\n{}",
                            cyan("webhook:"),
                            rec.id,
                            dim("(or sign the body: X-Gai-Signature: sha256=<hex HMAC-SHA256>)")
                        );
                    }
                    rec
                }
            };
            println!(
                "{} {} {}",
                green("saved"),
                rec.name,
                dim(&rec
                    .next_fire_at
                    .map(|n| format!("next {}", n.format("%Y-%m-%d %H:%M UTC")))
                    .unwrap_or_default())
            );
            eprintln!("{}", dim("Schedules and watchers run inside `gai serve`."));
        }
        TriggersCmd::Delete { trigger } => {
            t.delete(&trigger).await?;
            println!("{} {trigger}", green("deleted"));
        }
        TriggersCmd::Enable { trigger } => {
            t.set_enabled(&trigger, true).await?;
            println!("{}", green("on"));
        }
        TriggersCmd::Disable { trigger } => {
            t.set_enabled(&trigger, false).await?;
            println!("{}", yellow("off"));
        }
        TriggersCmd::Fire { trigger, payload } => {
            let rec = t.get(&trigger).await?;
            let payload = payload
                .map(|p| serde_json::from_str(&p))
                .transpose()
                .map_err(|e| anyhow::anyhow!("--payload is not JSON: {e}"))?;
            let fired = t.fire(&rec, "manual", payload, true).await?;
            match (&fired.run_id, &fired.error) {
                (Some(id), _) => {
                    let run = store.get_run(id).await?;
                    let out = run
                        .as_ref()
                        .and_then(|r| r.output.clone())
                        .map(|o| match o {
                            serde_json::Value::String(s) => s,
                            v => serde_json::to_string_pretty(&v).unwrap_or_default(),
                        })
                        .unwrap_or_default();
                    println!("{out}");
                    eprintln!(
                        "{}",
                        dim(&format!(
                            "run {id} {}",
                            run.map(|r| r.status.as_str().to_owned())
                                .unwrap_or_default()
                        ))
                    );
                }
                (None, Some(e)) => anyhow::bail!("{} {e}", fired.status),
                (None, None) => anyhow::bail!("{}", fired.status),
            }
        }
        TriggersCmd::Events { trigger, limit } => {
            let rec = t.get(&trigger).await?;
            for e in store.list_trigger_events(&rec.id, limit).await? {
                let status = match e.status.as_str() {
                    "started" => green(&e.status),
                    "skipped" => yellow(&e.status),
                    _ => red(&e.status),
                };
                println!(
                    "{} {:<9} {:<8} {} {}",
                    dim(&e.fired_at.format("%Y-%m-%d %H:%M:%S").to_string()),
                    e.reason,
                    status,
                    e.run_id.unwrap_or_default(),
                    e.error.map(|x| red(&x)).unwrap_or_default()
                );
            }
        }
        TriggersCmd::Secret { trigger } => {
            let rec = t.get(&trigger).await?;
            match rec.secret {
                Some(s) => println!("{s}"),
                None => anyhow::bail!("{} is not a webhook trigger", rec.name),
            }
        }
    }
    Ok(())
}
