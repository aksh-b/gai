use std::collections::BTreeMap;
use std::path::PathBuf;

use gai_llm::Reasoning;
use gai_memory::MemoryConfig;
use gai_tools::ToolPolicy;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Everything that defines an agent. Written as YAML/TOML/JSON or built in
/// code; stored as versioned JSON.
///
/// ```yaml
/// name: researcher
/// model: anthropic/claude-opus-5-5
/// system: |
///   You research questions thoroughly. Today is {{ date }}.
/// tools: [http_fetch, calculator, fs_*]
/// tool_policy: { fs_write: approve }
/// reasoning: { effort: high }
/// limits: { max_steps: 40, max_cost_usd: 2.0 }
/// ```
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AgentSpec {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Model address or alias. Falls back to the runtime's default model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// System prompt, a minijinja template. Variables: `agent`, `model`,
    /// `date`, `datetime`, `workspace`, `persona`, and anything passed as
    /// run `vars`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    /// A stored prompt template used as the instructions, before `system`:
    /// `name`, `name@v3` or `name@<label>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Persona the agent speaks as unless a run picks another: `name`,
    /// `name@v3` or `name@<label>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persona: Option<String>,
    /// Where the persona goes when `system` doesn't use `{{ persona }}`.
    /// Defaults to the global setting, else `prepend`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persona_placement: Option<crate::persona::Placement>,
    /// The language it answers in: a code (`fr`, `pt-BR`) or a name. A run
    /// can pick another; the server's default applies otherwise. The
    /// prompt can place it with `{{ language }}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Tool names, or prefixes ending in `*`. Ending in `?` makes an entry
    /// optional: `mcp__github__*?` uses those tools when the server is
    /// there and runs without them when it isn't.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    /// Overrides of each tool's default policy, by name or by a prefix
    /// ending in `*` (`mcp__gmail__send*: approve`); the longest match wins.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub tool_policy: BTreeMap<String, ToolPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// JSON Schema the final answer must follow.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    /// Directory file and shell tools work in. Defaults to a per-session
    /// directory under the data dir.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<PathBuf>,
    pub limits: Limits,
    pub context: ContextConfig,
    /// Memory blocks the agent always sees and edits, and archival memory.
    #[serde(skip_serializing_if = "MemoryConfig::is_empty")]
    pub memory: MemoryConfig,
    /// Knowledge collections the agent searches with `rag_search`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub knowledge: Vec<String>,
    /// Skills the agent can load: names, or prefixes ending in `*`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    /// Agents this agent can hand tasks to, each as an `ask_<name>` tool.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub delegates: Vec<crate::multi::DelegateRef>,
    /// Models for the `image_gen`, `speak` and `transcribe` tools, over
    /// the runtime's `[media]` defaults.
    #[serde(skip_serializing_if = "crate::media::MediaSettings::is_empty")]
    pub media: crate::media::MediaSettings,
    /// Personal data, prompt injection, guardrails and retention, over the
    /// runtime's `[safety]` settings field by field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safety: Option<crate::safety::Safety>,
    /// Free-form data for UIs and integrations.
    #[serde(skip_serializing_if = "Value::is_null")]
    pub metadata: Value,
}

impl AgentSpec {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    pub fn tools<S: Into<String>>(mut self, tools: impl IntoIterator<Item = S>) -> Self {
        self.tools = tools.into_iter().map(Into::into).collect();
        self
    }

    pub fn policy(mut self, tool: &str, policy: ToolPolicy) -> Self {
        self.tool_policy.insert(tool.to_owned(), policy);
        self
    }

    /// The policy `tool_policy` sets for a tool: its exact name, else the
    /// longest `prefix*` that matches.
    pub fn policy_for(&self, tool: &str) -> Option<ToolPolicy> {
        if let Some(p) = self.tool_policy.get(tool) {
            return Some(*p);
        }
        self.tool_policy
            .iter()
            .filter_map(|(k, p)| Some((k.strip_suffix('*')?, p)))
            .filter(|(prefix, _)| tool.starts_with(prefix))
            .max_by_key(|(prefix, _)| prefix.len())
            .map(|(_, p)| *p)
    }

    pub fn limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Parse an agent file. `format` is a file extension: `yaml`/`yml`,
    /// `toml` or `json`.
    pub fn parse(text: &str, format: &str) -> Result<Self, String> {
        let spec: Self = match format.to_ascii_lowercase().as_str() {
            "yaml" | "yml" => serde_yaml::from_str(text).map_err(|e| e.to_string())?,
            "toml" => toml::from_str(text).map_err(|e| e.to_string())?,
            "json" => serde_json::from_str(text).map_err(|e| e.to_string())?,
            other => {
                return Err(format!(
                    "unknown agent file format {other:?}; use .yaml, .toml or .json"
                ));
            }
        };
        spec.validate()?;
        Ok(spec)
    }

    /// Read and parse an agent file, choosing the format by extension.
    /// The agent name defaults to the file stem.
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("yaml");
        let mut value: Value = match ext.to_ascii_lowercase().as_str() {
            "yaml" | "yml" => {
                serde_yaml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?
            }
            "toml" => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?,
            _ => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?,
        };
        if let (Value::Object(o), Some(stem)) =
            (&mut value, path.file_stem().and_then(|s| s.to_str()))
        {
            o.entry("name")
                .or_insert_with(|| Value::String(stem.to_owned()));
        }
        let spec: Self =
            serde_json::from_value(value).map_err(|e| format!("{}: {e}", path.display()))?;
        spec.validate()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(spec)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("agent name is empty".into());
        }
        if let Some(s) = &self.safety {
            s.validate()?;
        }
        if !self
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(format!(
                "agent name {:?} may only use letters, digits, '-', '_' and '.'",
                self.name
            ));
        }
        if self.limits.max_steps == 0 {
            return Err("limits.max_steps must be at least 1".into());
        }
        let c = &self.context;
        if !(0.1..=1.0).contains(&c.compact_at) || !(0.1..=1.0).contains(&c.clear_tool_results_at) {
            return Err("context thresholds must be between 0.1 and 1.0".into());
        }
        self.memory.validate()?;
        if self.knowledge.iter().any(|k| k.trim().is_empty()) {
            return Err("knowledge lists an empty collection name".into());
        }
        Ok(())
    }
}

/// Hard ceilings for one run. Hitting any ends the run with
/// `budget_exceeded`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    /// Model calls per run.
    pub max_steps: u32,
    /// Input plus output tokens across the run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_cost_usd: Option<f64>,
    /// Wall-clock seconds, including time waiting for approvals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// Tool calls run at once when the model asks for several.
    pub max_parallel_tools: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_steps: 50,
            max_tokens: None,
            max_cost_usd: None,
            timeout_secs: None,
            max_parallel_tools: 8,
        }
    }
}

/// How the runtime keeps the conversation inside the context window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ContextConfig {
    /// Fraction of the window at which old tool results are cleared.
    pub clear_tool_results_at: f64,
    /// Most recent tool results never cleared.
    pub keep_tool_results: usize,
    /// Fraction of the window at which older turns are summarized.
    pub compact_at: f64,
    /// Messages kept verbatim when compacting.
    pub keep_recent_messages: usize,
    /// Model that writes summaries; defaults to the agent's model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_model: Option<String>,
    /// Override the model's context window, e.g. to cap spend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_context_tokens: Option<u32>,
    /// Use provider prompt caching.
    pub cache: bool,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            clear_tool_results_at: 0.6,
            keep_tool_results: 6,
            compact_at: 0.8,
            keep_recent_messages: 8,
            summary_model: None,
            max_context_tokens: None,
            cache: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_round_trips_with_defaults() {
        let s: AgentSpec = serde_json::from_value(serde_json::json!({
            "name": "r",
            "tools": ["fs_*"],
            "tool_policy": {"fs_write": "approve"},
            "limits": {"max_steps": 3}
        }))
        .unwrap();
        assert_eq!(s.limits.max_steps, 3);
        assert_eq!(s.limits.max_parallel_tools, 8);
        assert_eq!(s.tool_policy["fs_write"], ToolPolicy::Approve);
        let wild = AgentSpec::new("x")
            .policy("mcp__gmail__*", ToolPolicy::Auto)
            .policy("mcp__gmail__send*", ToolPolicy::Approve);
        assert_eq!(
            wild.policy_for("mcp__gmail__send_email"),
            Some(ToolPolicy::Approve)
        );
        assert_eq!(
            wild.policy_for("mcp__gmail__search"),
            Some(ToolPolicy::Auto)
        );
        assert_eq!(wild.policy_for("fs_read"), None);
        assert!(s.context.cache);
        s.validate().unwrap();
        assert!(AgentSpec::new("bad name").validate().is_err());
    }

    #[test]
    fn parses_yaml_and_toml() {
        let y = AgentSpec::parse(
            "name: helper\nmodel: mock/echo\ntools: [calculator]\nreasoning: { effort: high }\nlimits: { max_cost_usd: 0.5 }\n",
            "yaml",
        )
        .unwrap();
        assert_eq!(y.reasoning.unwrap().effort, gai_llm::Effort::High);
        assert_eq!(y.limits.max_cost_usd, Some(0.5));
        let t = AgentSpec::parse(
            "name = \"helper\"\ntools = [\"fs_*\"]\n[tool_policy]\nfs_write = \"deny\"\n",
            "toml",
        )
        .unwrap();
        assert_eq!(t.tool_policy["fs_write"], ToolPolicy::Deny);
        let typo = AgentSpec::parse("name: x\ntool: [calculator]\n", "yaml").unwrap_err();
        assert!(typo.contains("unknown field"), "{typo}");
    }
}
