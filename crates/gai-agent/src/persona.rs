//! Personas: reusable voices an agent or chat can take on.
//!
//! An agent says what it can do (model, tools, memory); a persona says who
//! it is and how it talks. The runtime renders the persona into the system
//! prompt, before or after the agent's own prompt, or wherever the agent's
//! template puts `{{ persona }}`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::runtime::{AgentError, Runtime};

/// A persona: identity for the UI, style for the prompt.
///
/// ```yaml
/// name: friendly-tutor
/// display_name: Friendly tutor
/// avatar: 🦉
/// greeting: Hi! What would you like to learn today?
/// tone: warm and encouraging
/// formality: casual
/// verbosity: balanced
/// dos: [Check understanding with a short question]
/// donts: [Hand over full solutions to exercises]
/// ```
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PersonaSpec {
    pub name: String,
    /// Name shown in chat and used in the prompt; defaults to `name`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// An emoji or an image URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    /// Shown when a chat with this persona starts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub greeting: Option<String>,
    /// Suggested first messages.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub starters: Vec<String>,
    /// Free text, e.g. "warm and encouraging".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formality: Option<Formality>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verbosity: Option<Verbosity>,
    /// Language to answer in, e.g. "British English" or "fr".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Anything else about the voice, in plain words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dos: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub donts: Vec<String>,
    /// Sample exchanges showing the voice.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<PersonaExample>,
    /// Free-form data for UIs and integrations.
    #[serde(skip_serializing_if = "Value::is_null")]
    pub metadata: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Formality {
    Casual,
    Neutral,
    Formal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Verbosity {
    Terse,
    Balanced,
    Detailed,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PersonaExample {
    pub user: String,
    pub assistant: String,
}

/// Where the persona goes when the agent's prompt doesn't place it with
/// `{{ persona }}`.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Placement {
    /// Before the agent's prompt. Keeps a stable prefix for prompt caching.
    #[default]
    Prepend,
    /// After the agent's prompt, so the voice has the last word.
    Append,
}

impl std::str::FromStr for Placement {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "prepend" => Ok(Self::Prepend),
            "append" => Ok(Self::Append),
            other => Err(format!(
                "unknown persona placement {other:?}; use prepend or append"
            )),
        }
    }
}

impl PersonaSpec {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Parse a persona file. `format` is `yaml`/`yml`, `toml` or `json`.
    pub fn parse(text: &str, format: &str) -> Result<Self, String> {
        let spec: Self = match format.to_ascii_lowercase().as_str() {
            "yaml" | "yml" => serde_yaml::from_str(text).map_err(|e| e.to_string())?,
            "toml" => toml::from_str(text).map_err(|e| e.to_string())?,
            "json" => serde_json::from_str(text).map_err(|e| e.to_string())?,
            other => {
                return Err(format!(
                    "unknown persona file format {other:?}; use .yaml, .toml or .json"
                ));
            }
        };
        spec.validate()?;
        Ok(spec)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("persona name is empty".into());
        }
        if !self
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(format!(
                "persona name {:?} may only use letters, digits, '-', '_' and '.'",
                self.name
            ));
        }
        if self.name == NO_PERSONA {
            return Err(format!(
                "{NO_PERSONA:?} is reserved; pick another persona name"
            ));
        }
        if self
            .examples
            .iter()
            .any(|e| e.user.trim().is_empty() || e.assistant.trim().is_empty())
        {
            return Err("every example needs both a user and an assistant message".into());
        }
        Ok(())
    }

    /// The name to speak as.
    pub fn speaker(&self) -> &str {
        self.display_name
            .as_deref()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or(&self.name)
    }

    /// The persona as system prompt text. Deterministic, so prompt caching
    /// holds across runs. Avatar, greeting and starters are for the UI and
    /// are left out.
    pub fn render(&self) -> String {
        let mut out = format!("<persona name=\"{}\">\n", self.speaker().replace('"', "'"));
        out.push_str(&format!("You are {}.", self.speaker()));
        if let Some(d) = nonempty(&self.description) {
            out.push(' ');
            out.push_str(d);
        }
        out.push('\n');
        let mut style = Vec::new();
        if let Some(t) = nonempty(&self.tone) {
            style.push(format!("Tone: {t}."));
        }
        if let Some(f) = self.formality {
            style.push(
                match f {
                    Formality::Casual => "Formality: casual and relaxed.",
                    Formality::Neutral => "Formality: neutral and professional.",
                    Formality::Formal => "Formality: formal and polished.",
                }
                .to_owned(),
            );
        }
        if let Some(v) = self.verbosity {
            style.push(
                match v {
                    Verbosity::Terse => "Length: as short as possible; no filler.",
                    Verbosity::Balanced => "Length: as long as the question needs, no longer.",
                    Verbosity::Detailed => "Length: thorough, with explanation and examples.",
                }
                .to_owned(),
            );
        }
        if let Some(l) = nonempty(&self.language) {
            style.push(format!("Language: answer in {l}."));
        }
        if !style.is_empty() {
            out.push('\n');
            out.push_str(&style.join("\n"));
            out.push('\n');
        }
        if let Some(i) = nonempty(&self.instructions) {
            out.push('\n');
            out.push_str(i.trim());
            out.push('\n');
        }
        for (title, items) in [("Do:", &self.dos), ("Don't:", &self.donts)] {
            let items: Vec<&String> = items.iter().filter(|s| !s.trim().is_empty()).collect();
            if !items.is_empty() {
                out.push('\n');
                out.push_str(title);
                out.push('\n');
                for i in items {
                    out.push_str(&format!("- {}\n", i.trim()));
                }
            }
        }
        if !self.examples.is_empty() {
            out.push_str("\nExamples of your voice:\n");
            for e in &self.examples {
                out.push_str(&format!(
                    "User: {}\nYou: {}\n",
                    e.user.trim(),
                    e.assistant.trim()
                ));
            }
        }
        out.push_str("</persona>");
        out
    }
}

fn nonempty(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|s| !s.trim().is_empty())
}

/// Turns the persona off for a run, over the agent's and global defaults.
pub const NO_PERSONA: &str = "none";

/// Which persona a run asks for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum PersonaRef {
    /// A stored persona: `name`, `name@v3` or `name@<label>`; `"none"` for
    /// no persona.
    Name(String),
    /// A persona given in full, e.g. an unsaved draft being previewed.
    Inline(Box<PersonaSpec>),
}

/// The persona a run settled on, recorded as `metadata.persona` so a
/// resumed run speaks the same way.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ResolvedPersona {
    pub name: String,
    /// Stored version; `None` for inline personas.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    pub placement: Placement,
    pub spec: PersonaSpec,
}

/// Put a rendered persona into a rendered system prompt. `placed` says the
/// agent's template already put it somewhere with `{{ persona }}`.
pub(crate) fn compose(system: String, persona: &str, placement: Placement, placed: bool) -> String {
    if placed || persona.is_empty() {
        return system;
    }
    if system.is_empty() {
        return persona.to_owned();
    }
    match placement {
        Placement::Prepend => format!("{persona}\n\n{system}"),
        Placement::Append => format!("{system}\n\n{persona}"),
    }
}

/// Whether a system prompt template uses the `persona` variable.
pub(crate) fn template_places_persona(template: &str) -> bool {
    let env = minijinja::Environment::new();
    match env.template_from_str(template) {
        Ok(t) => t.undeclared_variables(false).contains("persona"),
        Err(_) => false,
    }
}

impl Runtime {
    /// Validate and store a persona as its next version.
    pub async fn save_persona(
        &self,
        spec: &PersonaSpec,
        note: Option<&str>,
    ) -> Result<u32, AgentError> {
        spec.validate().map_err(AgentError::Invalid)?;
        let value = serde_json::to_value(spec).expect("serializable persona");
        Ok(self.store().put_persona(&spec.name, &value, note).await?)
    }

    /// Stored agents whose latest version speaks as persona `name` (any
    /// version or label of it) by default.
    pub async fn persona_users(&self, name: &str) -> Result<Vec<String>, AgentError> {
        let mut out = Vec::new();
        for a in self.store().list_agents().await? {
            if let Ok((spec, _)) = self.load_agent(&a.name, None).await
                && spec
                    .persona
                    .as_deref()
                    .is_some_and(|p| p.split('@').next() == Some(name))
            {
                out.push(a.name);
            }
        }
        Ok(out)
    }

    /// Load a stored persona by `name`, `name@v3` or `name@<label>`.
    pub async fn resolve_persona(&self, reference: &str) -> Result<(PersonaSpec, u32), AgentError> {
        let (name, selector) = match reference.split_once('@') {
            Some((n, s)) => (n, Some(s)),
            None => (reference, None),
        };
        let store = self.store();
        let version = match selector {
            None => None,
            Some(s) => match s.strip_prefix('v').and_then(|v| v.parse::<u32>().ok()) {
                Some(v) => Some(v),
                None => Some(
                    store
                        .list_personas()
                        .await?
                        .iter()
                        .find(|p| p.name == name)
                        .and_then(|p| p.labels.get(s).copied())
                        .ok_or_else(|| {
                            AgentError::NotFound(format!("persona {name} has no label {s:?}"))
                        })?,
                ),
            },
        };
        let stored = store
            .get_persona(name, version)
            .await?
            .ok_or_else(|| AgentError::NotFound(format!("persona {reference}")))?;
        let spec: PersonaSpec = serde_json::from_value(stored.spec)
            .map_err(|e| AgentError::Invalid(format!("stored persona {name}: {e}")))?;
        Ok((spec, stored.version))
    }

    /// The persona for a run: the request's choice, else the agent's, else
    /// the global default. `None` when there is none, or it is turned off.
    pub(crate) async fn pick_persona(
        &self,
        requested: Option<&PersonaRef>,
        agent: &crate::AgentSpec,
    ) -> Result<Option<ResolvedPersona>, AgentError> {
        let global = self.default_persona();
        let choice = match requested {
            Some(r) => Some(r.clone()),
            None => agent
                .persona
                .clone()
                .or(global.persona)
                .map(PersonaRef::Name),
        };
        let placement = agent.persona_placement.unwrap_or(global.placement);
        let (spec, version) = match choice {
            None => return Ok(None),
            Some(PersonaRef::Name(n)) if n.trim().is_empty() || n == NO_PERSONA => {
                return Ok(None);
            }
            Some(PersonaRef::Name(n)) => {
                let (spec, v) = self.resolve_persona(&n).await?;
                (spec, Some(v))
            }
            Some(PersonaRef::Inline(spec)) => {
                spec.validate().map_err(AgentError::Invalid)?;
                (*spec, None)
            }
        };
        Ok(Some(ResolvedPersona {
            name: spec.name.clone(),
            version,
            placement,
            spec,
        }))
    }
}

/// The persona used when neither the run nor the agent picks one.
///
/// ```toml
/// [persona]
/// default = "friendly-tutor"   # name, name@v3 or name@<label>
/// placement = "prepend"        # or "append"
/// ```
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct PersonaDefaults {
    /// A persona reference, as for an agent's `persona`.
    #[serde(alias = "default")]
    pub persona: Option<String>,
    pub placement: Placement,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tutor() -> PersonaSpec {
        PersonaSpec::parse(
            "name: tutor\ndisplay_name: Friendly tutor\navatar: \"🦉\"\ntone: warm\nformality: casual\nverbosity: terse\ndos: [Ask a check question]\ndonts: [Give full solutions]\nexamples:\n  - { user: hi, assistant: Hello there! }\n",
            "yaml",
        )
        .unwrap()
    }

    #[test]
    fn renders_style_but_not_ui_fields() {
        let p = tutor();
        let text = p.render();
        assert_eq!(text, p.render(), "deterministic");
        assert!(text.starts_with("<persona name=\"Friendly tutor\">\nYou are Friendly tutor."));
        assert!(text.contains("Tone: warm."));
        assert!(text.contains("Formality: casual"));
        assert!(text.contains("Do:\n- Ask a check question\n"));
        assert!(text.contains("Don't:\n- Give full solutions\n"));
        assert!(text.contains("User: hi\nYou: Hello there!\n"));
        assert!(!text.contains("🦉"));
        assert!(text.ends_with("</persona>"));
        assert_eq!(
            PersonaSpec::new("plain").render(),
            "<persona name=\"plain\">\nYou are plain.\n</persona>"
        );
    }

    #[test]
    fn validates() {
        tutor().validate().unwrap();
        assert!(PersonaSpec::new("bad name").validate().is_err());
        assert!(PersonaSpec::new(NO_PERSONA).validate().is_err());
        let typo = PersonaSpec::parse("name: x\ntones: warm\n", "yaml").unwrap_err();
        assert!(typo.contains("unknown field"), "{typo}");
        let mut p = PersonaSpec::new("x");
        p.examples.push(PersonaExample {
            user: "hi".into(),
            assistant: " ".into(),
        });
        assert!(p.validate().is_err());
    }

    #[test]
    fn composes_by_placement() {
        assert_eq!(
            compose("S".into(), "P", Placement::Prepend, false),
            "P\n\nS"
        );
        assert_eq!(compose("S".into(), "P", Placement::Append, false), "S\n\nP");
        assert_eq!(compose("S P".into(), "P", Placement::Append, true), "S P");
        assert_eq!(compose(String::new(), "P", Placement::Append, false), "P");
        assert_eq!(compose("S".into(), "", Placement::Prepend, false), "S");
        assert!(template_places_persona("Hi.\n{{ persona }}\nBye."));
        assert!(template_places_persona(
            "{% if persona %}{{ persona }}{% endif %}"
        ));
        assert!(!template_places_persona("You help with {{ topic }}."));
    }

    #[test]
    fn refs_parse_as_names_or_specs() {
        let n: PersonaRef = serde_json::from_value(serde_json::json!("tutor@v2")).unwrap();
        assert_eq!(n, PersonaRef::Name("tutor@v2".into()));
        let i: PersonaRef =
            serde_json::from_value(serde_json::json!({"name": "draft", "tone": "dry"})).unwrap();
        assert!(matches!(i, PersonaRef::Inline(s) if s.tone.as_deref() == Some("dry")));
    }
}
