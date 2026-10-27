//! Prompt templates: instructions kept and versioned on their own, so many
//! agents can share them and they can be improved (by hand, or by the
//! prompt optimizer) without touching the agents.
//!
//! An agent names one with `prompt: support-tone@prod`; its text becomes
//! the agent's instructions, before the agent's own `system`. Templates
//! are minijinja, with the same variables as `system`, and the prompt's
//! `variables` give defaults for run `vars`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::runtime::{AgentError, Runtime};

/// The library kind prompt templates are stored under.
pub const PROMPT_KIND: &str = "prompt";

/// A prompt template.
///
/// ```yaml
/// name: support-tone
/// title: Support tone
/// text: |
///   You answer customers of {{ company }}. Be brief and kind; say what
///   happens next.
/// variables:
///   company: { description: Who we are, default: Acme }
/// ```
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PromptSpec {
    /// Letters, digits, `-`, `_` and `.`.
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The instructions: a minijinja template.
    pub text: String,
    /// Variables the text uses, with what they are and their defaults.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub variables: BTreeMap<String, PromptVariable>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PromptVariable {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Used when a run doesn't pass the variable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

impl PromptSpec {
    pub fn parse(text: &str, format: &str) -> Result<Self, String> {
        let p: Self = match format {
            "json" => serde_json::from_str(text).map_err(|e| e.to_string())?,
            _ => serde_yaml::from_str(text).map_err(|e| e.to_string())?,
        };
        p.validate()?;
        Ok(p)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty()
            || !self
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(format!(
                "prompt name {:?} may only use letters, digits, '-', '_' and '.'",
                self.name
            ));
        }
        if self.text.trim().is_empty() {
            return Err(format!("prompt {} has no text", self.name));
        }
        minijinja::Environment::new()
            .template_from_str(&self.text)
            .map_err(|e| format!("prompt {}: {e}", self.name))?;
        Ok(())
    }

    /// Run vars with this prompt's defaults filled in.
    pub fn vars(&self, given: &Map<String, Value>) -> Map<String, Value> {
        let mut out = given.clone();
        for (k, v) in &self.variables {
            if let Some(d) = &v.default {
                out.entry(k.clone()).or_insert_with(|| d.clone());
            }
        }
        out
    }

    /// The text rendered with these variables (and the prompt's defaults).
    pub fn render(&self, vars: &Map<String, Value>) -> Result<String, String> {
        minijinja::Environment::new()
            .render_str(&self.text, self.vars(vars))
            .map_err(|e| format!("prompt {}: {e}", self.name))
    }
}

/// `name`, `name@v3` or `name@<label>`.
fn split(reference: &str) -> (&str, Option<&str>) {
    match reference.split_once('@') {
        Some((n, s)) => (n, Some(s)),
        None => (reference, None),
    }
}

impl Runtime {
    /// A stored prompt template and its version: `name` (latest),
    /// `name@v3`, or `name@<label>`.
    pub async fn resolve_prompt(&self, reference: &str) -> Result<(PromptSpec, u32), AgentError> {
        let (name, selector) = split(reference);
        let store = self.store();
        let version = match selector {
            None => None,
            Some(s) => match s.strip_prefix('v').and_then(|v| v.parse::<u32>().ok()) {
                Some(v) => Some(v),
                None => Some(
                    store
                        .item(PROMPT_KIND, name)
                        .await?
                        .and_then(|p| p.labels.get(s).copied())
                        .ok_or_else(|| {
                            AgentError::NotFound(format!("prompt {name} has no label {s:?}"))
                        })?,
                ),
            },
        };
        let stored = store
            .get_item(PROMPT_KIND, name, version)
            .await?
            .ok_or_else(|| AgentError::NotFound(format!("prompt {reference}")))?;
        let spec: PromptSpec = serde_json::from_value(stored.spec)
            .map_err(|e| AgentError::Invalid(format!("prompt {reference}: {e}")))?;
        Ok((spec, stored.version))
    }

    /// Stored agents whose latest version uses this prompt template.
    pub async fn prompt_users(&self, name: &str) -> Result<Vec<String>, AgentError> {
        let mut out = Vec::new();
        for a in self.store().list_agents().await? {
            if let Ok((spec, _)) = self.load_agent(&a.name, None).await
                && spec.prompt.as_deref().is_some_and(|p| split(p).0 == name)
            {
                out.push(a.name);
            }
        }
        Ok(out)
    }

    /// Save a prompt template as its next version.
    pub async fn save_prompt(
        &self,
        spec: &PromptSpec,
        note: Option<&str>,
    ) -> Result<u32, AgentError> {
        spec.validate().map_err(AgentError::Invalid)?;
        let v = serde_json::to_value(spec).expect("serializable prompt");
        Ok(self
            .store()
            .put_item(PROMPT_KIND, &spec.name, &v, note)
            .await?)
    }

    /// An agent with its `prompt` resolved: the template's text goes before
    /// the agent's own `system`, and its defaults into `vars`. Returns the
    /// prompt's name and version when there was one.
    pub(crate) async fn apply_prompt(
        &self,
        spec: &mut crate::AgentSpec,
        vars: &mut Map<String, Value>,
    ) -> Result<Option<(String, u32)>, AgentError> {
        let Some(reference) = spec.prompt.take() else {
            return Ok(None);
        };
        let (p, version) = self.resolve_prompt(&reference).await?;
        *vars = p.vars(vars);
        spec.system = Some(match spec.system.take() {
            Some(own) if !own.trim().is_empty() => format!("{}\n\n{own}", p.text.trim_end()),
            _ => p.text.clone(),
        });
        Ok(Some((p.name, version)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn checks_and_renders_with_defaults() {
        let p = PromptSpec::parse(
            "name: tone\ntext: 'Answer for {{ company }}, {{ who }}.'\nvariables:\n  company: { default: Acme }\n",
            "yaml",
        )
        .unwrap();
        let given = json!({"who": "Ada"}).as_object().unwrap().clone();
        assert_eq!(p.render(&given).unwrap(), "Answer for Acme, Ada.");
        let given = json!({"who": "Ada", "company": "Initech"})
            .as_object()
            .unwrap()
            .clone();
        assert_eq!(p.render(&given).unwrap(), "Answer for Initech, Ada.");
        assert!(PromptSpec::parse("name: x\ntext: '{{ oops'\n", "yaml").is_err());
        assert!(PromptSpec::parse("name: 'a b'\ntext: hi\n", "yaml").is_err());
        assert!(PromptSpec::parse("name: x\ntext: ' '\n", "yaml").is_err());
    }
}
