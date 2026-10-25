//! Knowledge collections, memory blocks and skills.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use gai::Gai;
use gai::memory::{NewCollection, SearchMode};

use crate::style::{age, cyan, dim, green, one_line, red, yellow};

#[derive(Args)]
pub struct IngestArgs {
    /// Files or directories (directories are read recursively)
    #[arg(required = true)]
    paths: Vec<PathBuf>,
    /// Collection to add to; created if it does not exist
    #[arg(long, short)]
    collection: String,
    /// Embedding model for a new collection (default: knowledge.embedding_model)
    #[arg(long)]
    model: Option<String>,
}

pub async fn ingest(gai: &Gai, a: IngestArgs) -> anyhow::Result<()> {
    let rt = gai.runtime().await?;
    let k = rt.knowledge();
    if rt.store().get_collection(&a.collection).await?.is_none() {
        let c = k
            .create_collection(NewCollection {
                name: a.collection.clone(),
                embedding_model: a.model,
                ..Default::default()
            })
            .await?;
        eprintln!(
            "{} collection {} {}",
            green("created"),
            c.name,
            dim(&format!("(embeddings: {})", c.embedding_model))
        );
    }
    let (mut added, mut same, mut failed) = (0, 0, 0);
    for p in &a.paths {
        for (file, r) in k.ingest_path(&a.collection, p).await? {
            match r {
                Ok(i) if i.unchanged => {
                    same += 1;
                    println!("{} {}", dim("unchanged"), file.display());
                }
                Ok(i) => {
                    added += 1;
                    println!(
                        "{} {} {}",
                        green("added"),
                        file.display(),
                        dim(&format!("({} chunks)", i.document.chunks))
                    );
                }
                Err(e) => {
                    failed += 1;
                    println!("{} {}: {e}", red("failed"), file.display());
                }
            }
        }
    }
    eprintln!(
        "{}",
        dim(&format!("{added} added, {same} unchanged, {failed} failed"))
    );
    if failed > 0 && added + same == 0 {
        anyhow::bail!("nothing was ingested");
    }
    Ok(())
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Mode {
    Hybrid,
    Vector,
    Text,
}

#[derive(Subcommand)]
pub enum CollectionsCmd {
    /// List collections
    List {
        /// Include archival memory collections
        #[arg(long)]
        all: bool,
    },
    /// Create an empty collection
    Create {
        name: String,
        #[arg(long)]
        description: Option<String>,
        /// Embedding model (default: knowledge.embedding_model)
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        chunk_tokens: Option<u32>,
    },
    /// List a collection's documents
    Docs { name: String },
    /// Search a collection
    Search {
        name: String,
        query: String,
        #[arg(long, short, default_value_t = 5)]
        k: usize,
        #[arg(long, value_enum, default_value = "hybrid")]
        mode: Mode,
    },
    /// Delete a collection and everything in it
    Delete { name: String },
    /// Copy a collection's vectors into the configured vector index
    /// (Qdrant or pgvector), e.g. after switching to it
    Reindex { name: String },
}

pub async fn collections(gai: &Gai, cmd: CollectionsCmd) -> anyhow::Result<()> {
    let rt = gai.runtime().await?;
    let store = rt.store();
    match cmd {
        CollectionsCmd::List { all } => {
            let list: Vec<_> = store
                .list_collections()
                .await?
                .into_iter()
                .filter(|c| all || !c.name.contains(':'))
                .collect();
            for c in &list {
                println!(
                    "{:<28} {:>5} docs {:>6} chunks  {} {}",
                    c.name,
                    c.documents,
                    c.chunks,
                    cyan(&c.embedding_model),
                    dim(&age(c.updated_at))
                );
            }
            if list.is_empty() {
                eprintln!(
                    "{}",
                    dim("No collections. Add documents with `gai ingest ./docs -c handbook`.")
                );
            }
        }
        CollectionsCmd::Create {
            name,
            description,
            model,
            chunk_tokens,
        } => {
            let c = rt
                .knowledge()
                .create_collection(NewCollection {
                    name,
                    description,
                    embedding_model: model,
                    chunk_tokens,
                    ..Default::default()
                })
                .await?;
            println!("{} {} ({})", green("created"), c.name, c.embedding_model);
        }
        CollectionsCmd::Docs { name } => {
            rt.knowledge().collection(&name).await?;
            for d in store.list_documents(&name, 1000, 0).await? {
                println!(
                    "{:<32} {:>4} chunks  {} {}",
                    d.source,
                    d.chunks,
                    d.title.unwrap_or_default(),
                    dim(&age(d.created_at))
                );
            }
        }
        CollectionsCmd::Search {
            name,
            query,
            k,
            mode,
        } => {
            let mode = match mode {
                Mode::Hybrid => SearchMode::Hybrid,
                Mode::Vector => SearchMode::Vector,
                Mode::Text => SearchMode::Text,
            };
            let hits = rt.knowledge().search(&[name], &query, k, mode).await?;
            for (i, h) in hits.iter().enumerate() {
                let label = match &h.heading {
                    Some(head) => format!("{} › {head}", h.source),
                    None => h.source.clone(),
                };
                println!(
                    "{} {} {}",
                    cyan(&format!("[{}]", i + 1)),
                    label,
                    dim(&format!("{:.4}", h.score))
                );
                println!("    {}", one_line(&h.text, 200));
            }
            if hits.is_empty() {
                eprintln!("{}", dim("No results."));
            }
        }
        CollectionsCmd::Reindex { name } => {
            let k = gai.runtime().await?.knowledge().clone();
            if k.index_backend() == "builtin" {
                anyhow::bail!("no vector index is configured ([knowledge.vector_index] backend)");
            }
            let n = k.reindex(&name).await?;
            println!(
                "{} {n} vectors into {}",
                green("indexed"),
                k.index_backend()
            );
        }
        CollectionsCmd::Delete { name } => {
            if gai
                .runtime()
                .await?
                .knowledge()
                .delete_collection(&name)
                .await?
            {
                println!("{} {name}", green("deleted"));
            } else {
                anyhow::bail!("no collection {name}");
            }
        }
    }
    Ok(())
}

#[derive(Subcommand)]
pub enum MemoryCmd {
    /// List memory blocks, optionally in one scope (agent:<name>, user:<id>, shared:<name>)
    List {
        scope: Option<String>,
    },
    /// Print a block's value
    Show {
        scope: String,
        label: String,
    },
    /// Replace a block's value
    Set {
        scope: String,
        label: String,
        value: String,
    },
    /// A block's past values
    History {
        scope: String,
        label: String,
    },
    Delete {
        scope: String,
        label: String,
    },
}

pub async fn memory(gai: &Gai, cmd: MemoryCmd) -> anyhow::Result<()> {
    let rt = gai.runtime().await?;
    let store = rt.store();
    let find = |scope: String, label: String| async move {
        store
            .find_block(&scope, &label)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no block {label} in {scope}"))
    };
    match cmd {
        MemoryCmd::List { scope } => {
            let blocks = store.list_blocks(scope.as_deref()).await?;
            for b in &blocks {
                println!(
                    "{:<24} {:<16} {:>5}/{:<5} v{:<3} {}{}",
                    b.scope,
                    b.label,
                    b.value.chars().count(),
                    b.limit,
                    b.version,
                    dim(&one_line(&b.value, 60)),
                    if b.read_only {
                        yellow(" read-only")
                    } else {
                        String::new()
                    }
                );
            }
            if blocks.is_empty() {
                eprintln!(
                    "{}",
                    dim(
                        "No memory blocks. Agents get them from `memory: { blocks: [...] }` in their spec."
                    )
                );
            }
        }
        MemoryCmd::Show { scope, label } => {
            println!("{}", find(scope, label).await?.value);
        }
        MemoryCmd::Set {
            scope,
            label,
            value,
        } => {
            let b = find(scope, label).await?;
            if value.chars().count() > b.limit as usize {
                anyhow::bail!(
                    "{} characters is over the limit of {}",
                    value.chars().count(),
                    b.limit
                );
            }
            let b = store
                .set_block_value(&b.id, &value, None, "edit", Some("cli"))
                .await?;
            println!("{} {} v{}", green("saved"), b.label, b.version);
        }
        MemoryCmd::History { scope, label } => {
            let b = find(scope, label).await?;
            for r in store.block_history(&b.id).await? {
                println!(
                    "v{:<3} {:<8} {:<20} {}",
                    r.version,
                    r.op,
                    r.actor.unwrap_or_default(),
                    dim(&age(r.created_at))
                );
                println!("    {}", one_line(&r.value, 120));
            }
        }
        MemoryCmd::Delete { scope, label } => {
            let b = find(scope, label).await?;
            store.delete_block(&b.id).await?;
            println!("{} {}", green("deleted"), b.label);
        }
    }
    Ok(())
}

#[derive(Subcommand)]
pub enum SkillsCmd {
    /// List skills from skill folders and the store
    List,
    /// Print a skill's SKILL.md
    Show { name: String },
    /// Upload a skill folder (SKILL.md plus its text files) to the store
    Add { dir: PathBuf },
    /// Delete an uploaded skill
    Delete { name: String },
}

fn text_files(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            text_files(root, &p, out);
        } else if let (Ok(rel), Ok(text)) = (p.strip_prefix(root), std::fs::read_to_string(&p)) {
            let rel = rel.to_string_lossy().replace('\\', "/");
            if rel != "SKILL.md" {
                out.insert(rel, text);
            }
        }
    }
}

pub async fn skills(gai: &Gai, cmd: SkillsCmd) -> anyhow::Result<()> {
    let rt = gai.runtime().await?;
    let skills = rt.skills();
    match cmd {
        SkillsCmd::List => {
            let list = skills.list().await?;
            for s in &list {
                let source = match &s.source {
                    gai::skills::SkillSource::Dir { path } => format!("{}", path.display()),
                    gai::skills::SkillSource::Stored { version } => format!("stored v{version}"),
                };
                println!(
                    "{:<24} {} {}",
                    s.name,
                    one_line(&s.description, 70),
                    dim(&source)
                );
            }
            if list.is_empty() {
                eprintln!(
                    "{}",
                    dim(&format!(
                        "No skills. Put skill folders (each with a SKILL.md) in {}, or upload one with `gai skills add <dir>`.",
                        skills
                            .dirs()
                            .iter()
                            .map(|d| d.display().to_string())
                            .collect::<Vec<_>>()
                            .join(" or ")
                    ))
                );
            }
        }
        SkillsCmd::Show { name } => {
            let s = skills
                .get(&name)
                .await?
                .ok_or_else(|| anyhow::anyhow!("no skill {name}"))?;
            print!("{}", s.raw);
            if !s.files.is_empty() {
                eprintln!("{}", dim(&format!("files: {}", s.files.join(", "))));
            }
        }
        SkillsCmd::Add { dir } => {
            let raw = std::fs::read_to_string(dir.join("SKILL.md"))
                .map_err(|e| anyhow::anyhow!("{}: {e}", dir.join("SKILL.md").display()))?;
            let mut files = BTreeMap::new();
            text_files(&dir, &dir, &mut files);
            let (name, v) = skills.save(&raw, files).await?;
            println!("{} {name} v{v}", green("saved"));
        }
        SkillsCmd::Delete { name } => {
            if skills.delete(&name).await? {
                println!("{} {name}", green("deleted"));
            } else {
                anyhow::bail!("no uploaded skill {name}");
            }
        }
    }
    Ok(())
}
