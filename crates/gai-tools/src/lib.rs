//! Tools agents can call.
//!
//! A [`Tool`] has a spec the model sees (name, description, JSON Schema) and
//! an async `call`. Write one by implementing the trait, or from a plain
//! async function with [`typed_tool`], which derives the schema from the
//! argument type.

pub mod a2a;
pub mod builtin;
pub mod mcp;
mod registry;
mod typed;

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use gai_core::{ContentBlock, ToolSpec};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub use registry::ToolRegistry;
pub use typed::{FnTool, IntoToolOutput, Json, typed_tool};

#[async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> ToolSpec;

    /// The policy applied when an agent does not set one for this tool.
    fn default_policy(&self) -> ToolPolicy {
        ToolPolicy::Auto
    }

    /// Safe to run twice with the same input. Durable runs re-execute
    /// idempotent tools that were in flight during a crash; others go back
    /// through approval.
    fn idempotent(&self) -> bool {
        false
    }

    async fn call(&self, ctx: &ToolCtx, input: Value) -> Result<ToolOutput, ToolError>;
}

pub type SharedTool = Arc<dyn Tool>;

/// Whether a tool call runs on its own, needs a human's approval, or is
/// refused.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ToolPolicy {
    #[default]
    Auto,
    #[serde(alias = "require_approval", alias = "ask")]
    Approve,
    Deny,
}

/// What a tool knows about the call it is serving.
#[derive(Clone, Debug)]
pub struct ToolCtx {
    pub run_id: String,
    /// The call being served (empty outside a run). Lets a tool recognise
    /// its own earlier attempt at the same call after a restart.
    pub call_id: String,
    pub session_id: String,
    pub agent: String,
    /// The directory file tools are confined to.
    pub workspace: PathBuf,
    pub cancel: CancellationToken,
    pub http: reqwest::Client,
    pub settings: Arc<ToolSettings>,
}

impl ToolCtx {
    /// A context for tests and one-off calls.
    pub fn for_workspace(workspace: impl Into<PathBuf>) -> Self {
        Self {
            run_id: gai_core::new_id("run"),
            call_id: String::new(),
            session_id: gai_core::new_id("ses"),
            agent: "test".into(),
            workspace: workspace.into(),
            cancel: CancellationToken::new(),
            http: reqwest::Client::new(),
            settings: Arc::default(),
        }
    }

    /// Resolve a path the model gave against the workspace, refusing
    /// anything that would land outside it (absolute paths, `..`, symlinks
    /// pointing out).
    pub fn resolve_path(&self, rel: &str) -> Result<PathBuf, ToolError> {
        let rel = rel.trim();
        let candidate = Path::new(rel);
        if candidate.is_absolute() || rel.starts_with('/') || rel.starts_with('\\') {
            return Err(ToolError::InvalidInput(format!(
                "{rel:?}: use a path relative to the workspace"
            )));
        }
        let mut clean = PathBuf::new();
        for c in candidate.components() {
            match c {
                Component::Normal(p) => clean.push(p),
                Component::CurDir => {}
                Component::ParentDir => {
                    if !clean.pop() {
                        return Err(ToolError::InvalidInput(format!(
                            "{rel:?} leaves the workspace"
                        )));
                    }
                }
                _ => {
                    return Err(ToolError::InvalidInput(format!(
                        "{rel:?} is not a workspace path"
                    )));
                }
            }
        }
        let root = self
            .workspace
            .canonicalize()
            .map_err(|e| ToolError::Failed(format!("workspace: {e}")))?;
        let full = root.join(&clean);
        // The deepest existing ancestor must still be inside the root once
        // symlinks are resolved.
        let mut probe = full.as_path();
        while !probe.exists() {
            probe = probe.parent().unwrap_or(&root);
        }
        let real = probe
            .canonicalize()
            .map_err(|e| ToolError::Failed(e.to_string()))?;
        if !real.starts_with(&root) {
            return Err(ToolError::InvalidInput(format!(
                "{rel:?} leaves the workspace"
            )));
        }
        Ok(full)
    }
}

/// Knobs for the built-in tools.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct ToolSettings {
    /// Let `http_fetch` reach localhost and private networks.
    pub allow_private_network: bool,
    /// Largest tool output passed back to the model, in characters.
    pub max_output_chars: usize,
    pub shell_timeout_secs: u64,
    pub http_timeout_secs: u64,
}

impl Default for ToolSettings {
    fn default() -> Self {
        Self {
            allow_private_network: false,
            max_output_chars: 50_000,
            shell_timeout_secs: 120,
            http_timeout_secs: 60,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ToolOutput {
    pub content: Vec<ContentBlock>,
    #[serde(default)]
    pub is_error: bool,
    /// What the call cost in USD, for tools that call paid services
    /// (image generation, speech). Added to the run's cost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
}

impl ToolOutput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::text(text)],
            is_error: false,
            cost: None,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::text(text)],
            is_error: true,
            cost: None,
        }
    }

    pub fn json(value: &impl Serialize) -> Self {
        Self::text(
            serde_json::to_string_pretty(value)
                .unwrap_or_else(|e| format!("<unserializable output: {e}>")),
        )
    }

    pub fn as_text(&self) -> String {
        self.content
            .iter()
            .filter_map(ContentBlock::as_text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Cut text blocks so the whole output stays under `max` characters.
    pub fn truncated(mut self, max: usize) -> Self {
        let mut budget = max;
        for block in &mut self.content {
            if let ContentBlock::Text { text } = block {
                let n = text.chars().count();
                if n > budget {
                    let kept: String = text.chars().take(budget).collect();
                    *text = format!("{kept}\n…[truncated {} of {n} characters]", n - budget);
                    budget = 0;
                } else {
                    budget -= n;
                }
            }
        }
        self
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("{0}")]
    Failed(String),
    #[error("timed out after {0}s")]
    Timeout(u64),
    #[error("cancelled")]
    Cancelled,
}

impl ToolError {
    /// What the model sees when a call fails.
    pub fn into_output(self) -> ToolOutput {
        ToolOutput::error(self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_inside_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let ctx = ToolCtx::for_workspace(dir.path());
        let root = dir.path().canonicalize().unwrap();
        assert_eq!(
            ctx.resolve_path("sub/../a.txt").unwrap(),
            root.join("a.txt")
        );
        assert_eq!(
            ctx.resolve_path("new/dir/f").unwrap(),
            root.join("new/dir/f")
        );
        assert!(ctx.resolve_path("../x").is_err());
        assert!(ctx.resolve_path("/etc/passwd").is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc", dir.path().join("out")).unwrap();
            assert!(ctx.resolve_path("out/passwd").is_err());
        }
    }

    #[test]
    fn truncation_counts_characters() {
        let out = ToolOutput::text("héllo world").truncated(5);
        assert!(out.as_text().starts_with("héllo\n…[truncated 6 of 11"));
    }
}
