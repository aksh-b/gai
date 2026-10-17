//! Self-edited memory: blocks of text an agent always sees in its prompt
//! and rewrites with tools as it learns, plus an archival store it can
//! write facts to and search later.

use gai_store::{MemoryBlock, NewBlock, SharedStore, StoreError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{MemoryError, Result};

/// An agent's memory settings.
///
/// ```yaml
/// memory:
///   blocks:
///     - label: persona
///       value: I am a patient tutor.
///     - label: human
///       scope: user            # one per user, shared by every agent
///       description: What you know about the person you talk to.
///     - label: team
///       scope: shared:support  # shared by every agent naming it
///       read_only: true
///   archival: true             # remember / memory_search tools
/// ```
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct MemoryConfig {
    pub blocks: Vec<BlockSpec>,
    /// Give the agent an archival memory to `remember` facts in and
    /// `memory_search` later.
    pub archival: bool,
    /// Whose archival memory: the agent's own, or the user's.
    pub archival_scope: ArchivalScope,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArchivalScope {
    #[default]
    Agent,
    User,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BlockSpec {
    /// Lowercase letters, digits, `_` and `-`.
    pub label: String,
    /// `agent` (this agent's own), `user` (the user's profile, shared by
    /// all agents) or `shared:<name>` (any agent naming it).
    #[serde(default = "agent_scope")]
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Starting value, used when the block does not exist yet.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub value: String,
    /// Most characters the block may hold.
    #[serde(default = "default_limit")]
    pub limit: u32,
    /// The agent sees the block but cannot change it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub read_only: bool,
}

fn agent_scope() -> String {
    "agent".into()
}

fn default_limit() -> u32 {
    2000
}

impl MemoryConfig {
    pub fn validate(&self) -> std::result::Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        for b in &self.blocks {
            if !valid_label(&b.label) {
                return Err(format!(
                    "memory block label {:?}: use lowercase letters, digits, '_' and '-' (at most 40)",
                    b.label
                ));
            }
            if !seen.insert(&b.label) {
                return Err(format!("memory block {:?} is listed twice", b.label));
            }
            parse_scope(&b.scope)?;
            if b.limit == 0 || b.limit > 100_000 {
                return Err(format!(
                    "memory block {:?}: limit must be between 1 and 100000",
                    b.label
                ));
            }
            if b.value.chars().count() > b.limit as usize {
                return Err(format!(
                    "memory block {:?}: the starting value is longer than its limit",
                    b.label
                ));
            }
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty() && !self.archival
    }
}

pub fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 40
        && label
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-'))
}

enum Scope<'a> {
    Agent,
    User,
    Shared(&'a str),
}

fn parse_scope(s: &str) -> std::result::Result<Scope<'_>, String> {
    match s {
        "agent" => Ok(Scope::Agent),
        "user" => Ok(Scope::User),
        _ => match s.strip_prefix("shared:") {
            Some(name) if valid_label(name) => Ok(Scope::Shared(name)),
            _ => Err(format!(
                "memory scope {s:?}: use agent, user or shared:<name>"
            )),
        },
    }
}

/// The stored scope of a block: `agent:<agent>`, `user:<user>` or
/// `shared:<name>`.
pub fn resolve_scope(scope: &str, agent: &str, user: &str) -> Result<String> {
    Ok(match parse_scope(scope).map_err(MemoryError::Invalid)? {
        Scope::Agent => format!("agent:{agent}"),
        Scope::User => format!("user:{user}"),
        Scope::Shared(name) => format!("shared:{name}"),
    })
}

/// The collection archival memories live in.
pub fn archival_collection(cfg: &MemoryConfig, agent: &str, user: &str) -> String {
    match cfg.archival_scope {
        ArchivalScope::Agent => format!("memory:agent:{agent}"),
        ArchivalScope::User => format!("memory:user:{user}"),
    }
}

/// The memory blocks of one run, by label.
pub struct RunMemory {
    store: SharedStore,
    blocks: Vec<(String, String)>,
}

impl RunMemory {
    /// Make sure every block the agent declares exists (seeding new ones
    /// with their starting value) and bind them by label.
    pub async fn prepare(
        store: SharedStore,
        cfg: &MemoryConfig,
        agent: &str,
        user: &str,
    ) -> Result<Self> {
        cfg.validate().map_err(MemoryError::Invalid)?;
        let mut blocks = Vec::with_capacity(cfg.blocks.len());
        for b in &cfg.blocks {
            let scope = resolve_scope(&b.scope, agent, user)?;
            let block = store
                .ensure_block(NewBlock {
                    scope,
                    label: b.label.clone(),
                    description: b.description.clone(),
                    value: b.value.clone(),
                    limit: b.limit,
                    read_only: b.read_only,
                })
                .await?;
            blocks.push((b.label.clone(), block.id));
        }
        Ok(Self { store, blocks })
    }

    pub fn labels(&self) -> impl Iterator<Item = &str> {
        self.blocks.iter().map(|(l, _)| l.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Whether any block can be edited, so edit tools are worth offering.
    pub async fn any_writable(&self) -> bool {
        for (_, id) in &self.blocks {
            if let Ok(Some(b)) = self.store.get_block(id).await
                && !b.read_only
            {
                return true;
            }
        }
        false
    }

    pub async fn current(&self) -> Vec<MemoryBlock> {
        let mut out = Vec::with_capacity(self.blocks.len());
        for (label, id) in &self.blocks {
            match self.store.get_block(id).await {
                Ok(Some(b)) => out.push(b),
                Ok(None) => tracing::warn!(label, "memory block was deleted during the run"),
                Err(e) => tracing::warn!(label, error = %e, "could not read memory block"),
            }
        }
        out
    }

    /// The blocks as they go in the system prompt.
    pub async fn render(&self) -> String {
        let blocks = self.current().await;
        if blocks.is_empty() {
            return String::new();
        }
        let mut s = String::from(
            "<memory_blocks>\nThese blocks are your memory. They persist across conversations and you always see their latest contents here. Keep them accurate and useful: add what you learn with memory_append, fix details with memory_replace, and rewrite a block with memory_rethink when it gets messy. Never store secrets.\n",
        );
        for b in blocks {
            let used = b.value.chars().count();
            s.push_str(&format!("<{} chars=\"{used}/{}\"", b.label, b.limit));
            if b.read_only {
                s.push_str(" read_only=\"true\"");
            }
            if let Some(d) = &b.description {
                s.push_str(&format!(" description=\"{}\"", d.replace('"', "'")));
            }
            s.push_str(">\n");
            s.push_str(&b.value);
            if !b.value.is_empty() && !b.value.ends_with('\n') {
                s.push('\n');
            }
            s.push_str(&format!("</{}>\n", b.label));
        }
        s.push_str("</memory_blocks>");
        s
    }

    fn id(&self, label: &str) -> Result<&str> {
        self.blocks
            .iter()
            .find(|(l, _)| l == label)
            .map(|(_, id)| id.as_str())
            .ok_or_else(|| {
                MemoryError::Invalid(format!(
                    "no memory block {label:?}; blocks: {}",
                    self.labels().collect::<Vec<_>>().join(", ")
                ))
            })
    }

    /// Apply `edit` to the current value of a block and save it, retrying
    /// if another writer got in first.
    pub async fn edit(
        &self,
        label: &str,
        op: &str,
        actor: &str,
        edit: impl Fn(&str) -> Result<String>,
    ) -> Result<MemoryBlock> {
        let id = self.id(label)?;
        for _ in 0..3 {
            let block = self
                .store
                .get_block(id)
                .await?
                .ok_or_else(|| MemoryError::NotFound(format!("memory block {label}")))?;
            if block.read_only {
                return Err(MemoryError::Invalid(format!(
                    "memory block {label} is read-only"
                )));
            }
            let value = edit(&block.value)?;
            let n = value.chars().count();
            if n > block.limit as usize {
                return Err(MemoryError::Invalid(format!(
                    "that would make {label} {n} characters, over its limit of {}; {} are in use. Shorten it first (memory_rethink) or store details in archival memory",
                    block.limit,
                    block.value.chars().count()
                )));
            }
            match self
                .store
                .set_block_value(id, &value, Some(block.version), op, Some(actor))
                .await
            {
                Ok(b) => return Ok(b),
                Err(StoreError::Conflict(_)) => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Err(MemoryError::Invalid(format!(
            "memory block {label} kept changing under this edit; try again"
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn config_validation() {
        let ok: MemoryConfig = serde_json::from_value(serde_json::json!({
            "blocks": [{"label": "persona", "value": "hi"}, {"label": "human", "scope": "user"},
                       {"label": "team", "scope": "shared:support", "read_only": true}],
            "archival": true
        }))
        .unwrap();
        ok.validate().unwrap();
        assert_eq!(ok.blocks[0].limit, 2000);
        let bad = |v: serde_json::Value| {
            serde_json::from_value::<MemoryConfig>(v)
                .map_err(|e| e.to_string())
                .and_then(|c| c.validate())
                .unwrap_err()
        };
        assert!(bad(serde_json::json!({"blocks": [{"label": "Persona"}]})).contains("label"));
        assert!(
            bad(serde_json::json!({"blocks": [{"label": "a", "scope": "team"}]})).contains("scope")
        );
        assert!(
            bad(serde_json::json!({"blocks": [{"label": "a"}, {"label": "a"}]})).contains("twice")
        );
        assert!(
            bad(serde_json::json!({"blocks": [{"label": "a", "limit": 2, "value": "abc"}]}))
                .contains("limit")
        );
        assert!(bad(serde_json::json!({"block": []})).contains("unknown field"));
        assert_eq!(resolve_scope("user", "a", "u1").unwrap(), "user:u1");
        assert_eq!(resolve_scope("shared:x", "a", "u1").unwrap(), "shared:x");
    }

    #[tokio::test]
    async fn blocks_are_seeded_once_rendered_and_edited_within_limits() {
        let store: SharedStore = Arc::new(gai_store::SqliteStore::temporary().await.unwrap());
        let cfg: MemoryConfig = serde_json::from_value(serde_json::json!({
            "blocks": [{"label": "persona", "value": "I tutor.", "limit": 30},
                       {"label": "human", "scope": "user"},
                       {"label": "rules", "value": "Be kind.", "read_only": true}]
        }))
        .unwrap();
        let m = RunMemory::prepare(store.clone(), &cfg, "tutor", "ann")
            .await
            .unwrap();
        let r = m.render().await;
        assert!(
            r.contains("<persona chars=\"8/30\">\nI tutor.\n</persona>"),
            "{r}"
        );
        assert!(
            r.contains("<rules chars=\"8/2000\" read_only=\"true\">"),
            "{r}"
        );
        assert!(m.any_writable().await);

        let b = m
            .edit("persona", "append", "run_1", |v| {
                Ok(format!("{v}\nLikes maths."))
            })
            .await
            .unwrap();
        assert_eq!(b.value, "I tutor.\nLikes maths.");
        let too_long = m
            .edit("persona", "append", "run_1", |v| {
                Ok(format!("{v} and much much more"))
            })
            .await
            .unwrap_err();
        assert!(
            too_long.to_string().contains("over its limit"),
            "{too_long}"
        );
        assert!(
            m.edit("rules", "rethink", "r", |_| Ok("x".into()))
                .await
                .is_err()
        );
        assert!(
            m.edit("nope", "rethink", "r", |_| Ok("x".into()))
                .await
                .is_err()
        );

        // Preparing again keeps the edited value, and the user's block is
        // the same one another agent sees.
        let again = RunMemory::prepare(store.clone(), &cfg, "tutor", "ann")
            .await
            .unwrap();
        assert!(again.render().await.contains("Likes maths."));
        let other_cfg: MemoryConfig = serde_json::from_value(
            serde_json::json!({"blocks": [{"label": "human", "scope": "user"}]}),
        )
        .unwrap();
        let other = RunMemory::prepare(store.clone(), &other_cfg, "coach", "ann")
            .await
            .unwrap();
        other
            .edit("human", "append", "run_2", |_| {
                Ok("Ann prefers short answers.".into())
            })
            .await
            .unwrap();
        assert!(m.render().await.contains("Ann prefers short answers."));
        assert_eq!(store.list_blocks(Some("user:ann")).await.unwrap().len(), 1);
    }
}
