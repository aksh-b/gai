//! Hooks: code that runs around every model call and tool call, for
//! guardrails, redaction, auditing or rewriting.
//!
//! A hook can change what passes through it, or block it:
//! - blocking `before_model`/`after_model` fails the run;
//! - blocking `before_tool` returns the reason to the model instead of
//!   running the tool;
//! - blocking `after_tool` replaces the tool's output with the reason.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gai_llm::{ChatRequest, ChatResponse};
use gai_tools::ToolOutput;
use regex::Regex;
use serde_json::Value;

/// Where a hook is being called from.
#[derive(Clone, Debug)]
pub struct HookCtx {
    pub run_id: String,
    pub session_id: String,
    pub agent: String,
    pub step: u32,
    /// The run's safety settings: `[safety]` with the agent's overrides.
    pub safety: Arc<crate::safety::Safety>,
    /// Warnings for the run's events; see [`HookCtx::warn`].
    pub warnings: Arc<Mutex<Vec<String>>>,
    /// Calls of each tool so far in this run.
    pub tool_calls: Arc<Mutex<HashMap<String, u32>>>,
}

impl HookCtx {
    /// A context for tests and calls outside a run.
    pub fn new(run_id: &str, agent: &str) -> Self {
        Self {
            run_id: run_id.into(),
            session_id: String::new(),
            agent: agent.into(),
            step: 0,
            safety: Arc::default(),
            warnings: Arc::default(),
            tool_calls: Arc::default(),
        }
    }

    /// Report something in the run's events (as a `warning`) without
    /// stopping anything.
    pub fn warn(&self, message: impl Into<String>) {
        self.warnings.lock().unwrap().push(message.into());
    }
}

/// A tool call as hooks see it; `input` may be rewritten.
#[derive(Clone, Debug)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// Why a hook stopped something. Shown to the model or recorded on the run.
#[derive(Clone, Debug)]
pub struct Blocked(pub String);

pub type HookResult = Result<(), Blocked>;

#[async_trait]
pub trait RunHook: Send + Sync {
    fn name(&self) -> &str;

    async fn before_model(&self, _ctx: &HookCtx, _req: &mut ChatRequest) -> HookResult {
        Ok(())
    }

    /// Sees the complete response. Text has already streamed to clients;
    /// changes here affect what is stored and what the model sees next.
    async fn after_model(&self, _ctx: &HookCtx, _resp: &mut ChatResponse) -> HookResult {
        Ok(())
    }

    async fn before_tool(&self, _ctx: &HookCtx, _call: &mut ToolCall) -> HookResult {
        Ok(())
    }

    async fn after_tool(
        &self,
        _ctx: &HookCtx,
        _call: &ToolCall,
        _output: &mut ToolOutput,
    ) -> HookResult {
        Ok(())
    }
}

/// Masks credentials in tool output before the model (and the transcript)
/// see them: API keys, tokens, private keys, passwords in URLs.
pub struct RedactSecrets {
    patterns: Vec<Regex>,
}

impl RedactSecrets {
    const BUILTIN: &'static [&'static str] = &[
        r"sk-ant-[A-Za-z0-9_\-]{20,}",
        r"sk-(?:proj-)?[A-Za-z0-9_\-]{20,}",
        r"gai_[0-9a-f]{40}",
        r"gh[pousr]_[A-Za-z0-9]{30,}",
        r"github_pat_[A-Za-z0-9_]{40,}",
        r"AKIA[0-9A-Z]{16}",
        r"xox[abprs]-[A-Za-z0-9\-]{10,}",
        r"AIza[0-9A-Za-z_\-]{35}",
        r"(?i)bearer\s+[A-Za-z0-9._\-]{20,}",
        r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
        r"(?i)(?:postgres|postgresql|mysql|mongodb(?:\+srv)?|redis|amqp)://[^:\s/]+:[^@\s]+@",
    ];

    /// The built-in patterns plus `extra` regexes.
    pub fn new(extra: &[String]) -> Result<Self, regex::Error> {
        let patterns = Self::BUILTIN
            .iter()
            .map(|p| p.to_string())
            .chain(extra.iter().cloned())
            .map(|p| Regex::new(&p))
            .collect::<Result<_, _>>()?;
        Ok(Self { patterns })
    }

    pub fn redact(&self, text: &str) -> Option<String> {
        let mut out = std::borrow::Cow::Borrowed(text);
        for re in &self.patterns {
            if re.is_match(&out) {
                out = std::borrow::Cow::Owned(re.replace_all(&out, "[REDACTED]").into_owned());
            }
        }
        match out {
            std::borrow::Cow::Owned(s) => Some(s),
            std::borrow::Cow::Borrowed(_) => None,
        }
    }
}

#[async_trait]
impl RunHook for RedactSecrets {
    fn name(&self) -> &str {
        "redact_secrets"
    }

    async fn after_tool(
        &self,
        _ctx: &HookCtx,
        _call: &ToolCall,
        output: &mut ToolOutput,
    ) -> HookResult {
        for block in &mut output.content {
            if let gai_core::ContentBlock::Text { text } = block
                && let Some(clean) = self.redact(text)
            {
                *text = clean;
            }
        }
        Ok(())
    }
}

/// Refuses shell commands that match any pattern (regexes), before an
/// approval is even asked for.
pub struct DenyShell {
    patterns: Vec<Regex>,
}

impl DenyShell {
    pub fn new(patterns: &[String]) -> Result<Self, regex::Error> {
        Ok(Self {
            patterns: patterns
                .iter()
                .map(|p| Regex::new(p))
                .collect::<Result<_, _>>()?,
        })
    }
}

#[async_trait]
impl RunHook for DenyShell {
    fn name(&self) -> &str {
        "deny_shell"
    }

    async fn before_tool(&self, _ctx: &HookCtx, call: &mut ToolCall) -> HookResult {
        if call.name != "shell" {
            return Ok(());
        }
        let cmd = call.input["command"].as_str().unwrap_or_default();
        match self.patterns.iter().find(|p| p.is_match(cmd)) {
            Some(p) => Err(Blocked(format!(
                "this command matches a blocked pattern ({p})"
            ))),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> HookCtx {
        HookCtx::new("r", "a")
    }

    #[tokio::test]
    async fn redacts_common_secrets() {
        let r = RedactSecrets::new(&["internal-[0-9]{4}".into()]).unwrap();
        let mut out = ToolOutput::text(
            "key=sk-ant-api03-abcdefghijklmnopqrstuvwxyz token ghp_abcdefghijklmnopqrstuvwxyz0123456789 \
             db=postgres://admin:hunter2@db:5432/x id internal-1234 fine text",
        );
        let call = ToolCall {
            id: "1".into(),
            name: "fs_read".into(),
            input: json!({}),
        };
        r.after_tool(&ctx(), &call, &mut out).await.unwrap();
        let t = out.as_text();
        assert!(
            !t.contains("sk-ant")
                && !t.contains("ghp_")
                && !t.contains("hunter2")
                && !t.contains("internal-1234"),
            "{t}"
        );
        assert!(t.contains("fine text"));
        assert!(r.redact("nothing secret here").is_none());
    }

    #[tokio::test]
    async fn blocks_matching_shell_commands() {
        let d = DenyShell::new(&[r"rm\s+-rf\s+/".into()]).unwrap();
        let mut bad = ToolCall {
            id: "1".into(),
            name: "shell".into(),
            input: json!({"command": "sudo rm -rf / --no-preserve-root"}),
        };
        assert!(d.before_tool(&ctx(), &mut bad).await.is_err());
        let mut ok = ToolCall {
            id: "2".into(),
            name: "shell".into(),
            input: json!({"command": "ls"}),
        };
        assert!(d.before_tool(&ctx(), &mut ok).await.is_ok());
    }
}
