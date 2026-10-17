//! The tools agents use for memory and knowledge. They are built per run,
//! bound to that run's blocks and collections.

use std::sync::Arc;

use gai_tools::{SharedTool, ToolError, ToolOutput, typed_tool};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::MemoryError;
use crate::blocks::RunMemory;
use crate::knowledge::{DocumentInput, Knowledge, SearchMode, format_hits};

impl From<MemoryError> for ToolError {
    fn from(e: MemoryError) -> Self {
        match e {
            MemoryError::Invalid(m) => ToolError::InvalidInput(m),
            other => ToolError::Failed(other.to_string()),
        }
    }
}

#[derive(Deserialize, JsonSchema)]
struct AppendArgs {
    /// The block to add to.
    label: String,
    /// Text to add on a new line at the end of the block.
    content: String,
}

#[derive(Deserialize, JsonSchema)]
struct ReplaceArgs {
    label: String,
    /// Exact text currently in the block. It must appear exactly once.
    old: String,
    /// What to put in its place; empty to delete it.
    #[serde(default)]
    new: String,
}

#[derive(Deserialize, JsonSchema)]
struct RethinkArgs {
    label: String,
    /// The block's complete new contents.
    new_value: String,
}

fn saved(b: &gai_store::MemoryBlock) -> ToolOutput {
    ToolOutput::text(format!(
        "Saved {} ({}/{} characters).",
        b.label,
        b.value.chars().count(),
        b.limit
    ))
}

/// `memory_append`, `memory_replace` and `memory_rethink` over the run's
/// blocks.
pub fn block_tools(memory: Arc<RunMemory>) -> Vec<SharedTool> {
    let labels = memory.labels().collect::<Vec<_>>().join(", ");
    let m = memory.clone();
    let append = typed_tool(
        "memory_append",
        &format!(
            "Add a line to one of your memory blocks ({labels}). Use it to remember durable facts, preferences and decisions as you learn them."
        ),
        move |ctx, a: AppendArgs| {
            let m = m.clone();
            async move {
                let content = a.content.trim().to_owned();
                if content.is_empty() {
                    return Err(ToolError::InvalidInput("content is empty".into()));
                }
                let b = m
                    .edit(&a.label, "append", &ctx.run_id, |v| {
                        Ok(if v.trim().is_empty() {
                            content.clone()
                        } else {
                            format!("{}\n{content}", v.trim_end())
                        })
                    })
                    .await?;
                Ok(saved(&b))
            }
        },
    );
    let m = memory.clone();
    let replace = typed_tool(
        "memory_replace",
        "Replace exact text in a memory block, e.g. to correct an outdated fact. `old` must appear exactly once; an empty `new` deletes it.",
        move |ctx, a: ReplaceArgs| {
            let m = m.clone();
            async move {
                if a.old.is_empty() {
                    return Err(ToolError::InvalidInput("old is empty".into()));
                }
                let b = m
                    .edit(&a.label, "replace", &ctx.run_id, |v| {
                        match v.matches(&a.old).count() {
                            1 => Ok(v.replacen(&a.old, &a.new, 1)),
                            0 => Err(MemoryError::Invalid(format!(
                                "{:?} is not in {}; copy the text exactly as the block shows it",
                                a.old, a.label
                            ))),
                            n => Err(MemoryError::Invalid(format!(
                                "{:?} appears {n} times in {}; include more surrounding text",
                                a.old, a.label
                            ))),
                        }
                    })
                    .await?;
                Ok(saved(&b))
            }
        },
    );
    let m = memory;
    let rethink = typed_tool(
        "memory_rethink",
        "Rewrite a memory block completely: reorganize it, merge duplicates, drop what no longer matters. Pass the full new contents.",
        move |ctx, a: RethinkArgs| {
            let m = m.clone();
            async move {
                let value = a.new_value.trim().to_owned();
                let b = m
                    .edit(&a.label, "rethink", &ctx.run_id, |_| Ok(value.clone()))
                    .await?;
                Ok(saved(&b))
            }
        },
    );
    vec![Arc::new(append), Arc::new(replace), Arc::new(rethink)]
}

#[derive(Deserialize, JsonSchema)]
struct RememberArgs {
    /// A self-contained fact or note, written so it makes sense on its own later.
    text: String,
}

#[derive(Deserialize, JsonSchema)]
struct SearchArgs {
    /// What to look for, in plain words.
    query: String,
    /// How many results (default 5, at most 20).
    #[serde(default)]
    k: Option<usize>,
}

/// `remember` and `memory_search` over an archival collection.
pub fn archival_tools(knowledge: Arc<Knowledge>, collection: String) -> Vec<SharedTool> {
    let (k, c) = (knowledge.clone(), collection.clone());
    let remember = typed_tool(
        "remember",
        "Save a fact or note to your long-term archival memory, to find later with memory_search. Use it for details too large or too specific for your memory blocks.",
        move |ctx, a: RememberArgs| {
            let (k, c) = (k.clone(), c.clone());
            async move {
                let text = a.text.trim().to_owned();
                if text.is_empty() {
                    return Err(ToolError::InvalidInput("text is empty".into()));
                }
                k.ensure_collection(&c, "Archival memory").await?;
                let title: String = text.chars().take(80).collect();
                let doc = k
                    .ingest(
                        &c,
                        DocumentInput {
                            source: gai_core::new_id("mem"),
                            title: Some(title),
                            text,
                            metadata: json!({"run_id": ctx.run_id, "session_id": ctx.session_id}),
                        },
                    )
                    .await?;
                Ok(ToolOutput::text(format!(
                    "Remembered ({}).",
                    doc.document.source
                )))
            }
        },
    );
    let search = typed_tool(
        "memory_search",
        "Search your long-term archival memory for things you saved with remember.",
        move |_ctx, a: SearchArgs| {
            let (k, c) = (knowledge.clone(), collection.clone());
            async move {
                if k.store()
                    .get_collection(&c)
                    .await
                    .map_err(MemoryError::from)?
                    .is_none()
                {
                    return Ok(ToolOutput::text("Archival memory is empty."));
                }
                let hits = k
                    .search(&[c], &a.query, a.k.unwrap_or(5).min(20), SearchMode::Hybrid)
                    .await?;
                Ok(ToolOutput::text(format_hits(&hits)))
            }
        },
    )
    .idempotent();
    vec![Arc::new(remember), Arc::new(search)]
}

#[derive(Deserialize, JsonSchema)]
struct RagArgs {
    /// What to look for. Plain words work best; include key terms.
    query: String,
    /// Search only this collection.
    #[serde(default)]
    collection: Option<String>,
    /// How many passages (default 5, at most 20).
    #[serde(default)]
    k: Option<usize>,
}

/// `rag_search` over `collections`, or over every knowledge collection
/// when the list is empty.
pub fn rag_tool(knowledge: Arc<Knowledge>, collections: Vec<String>) -> SharedTool {
    let scope = if collections.is_empty() {
        "the knowledge base".to_owned()
    } else {
        format!("the knowledge base ({})", collections.join(", "))
    };
    Arc::new(
        typed_tool(
            "rag_search",
            &format!("Search {scope} for passages relevant to a question. Results are numbered with their sources; cite them as [n]. Search again with different words if the first results miss."),
            move |_ctx, a: RagArgs| {
                let (k, allowed) = (knowledge.clone(), collections.clone());
                async move {
                    let all = if allowed.is_empty() {
                        k.store()
                            .list_collections()
                            .await
                            .map_err(MemoryError::from)?
                            .into_iter()
                            .map(|c| c.name)
                            .filter(|n| !n.contains(':'))
                            .collect()
                    } else {
                        allowed
                    };
                    let targets = match a.collection {
                        Some(c) if all.contains(&c) => vec![c],
                        Some(c) => {
                            return Err(ToolError::InvalidInput(format!(
                                "unknown collection {c:?}; available: {}",
                                all.join(", ")
                            )));
                        }
                        None => all,
                    };
                    if targets.is_empty() {
                        return Ok(ToolOutput::text("There are no knowledge collections yet."));
                    }
                    let hits = k
                        .search(&targets, &a.query, a.k.unwrap_or(5).min(20), SearchMode::Hybrid)
                        .await?;
                    Ok(ToolOutput::text(format_hits(&hits)))
                }
            },
        )
        .idempotent(),
    )
}
