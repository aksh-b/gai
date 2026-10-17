use std::collections::BTreeMap;
use std::time::Duration;

use gai_core::Pricing;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{Capabilities, LlmError, ModelKind};

/// The model layer's slice of `gai.toml`.
///
/// ```toml
/// [providers.myvllm]
/// dialect = "openai"
/// base_url = "http://10.0.0.5:8000/v1"
/// api_key_env = "MYVLLM_KEY"
///
/// [[providers.myvllm.models]]
/// id = "qwen3-32b"
/// capabilities = { tools = true, context_window = 131072 }
///
/// [aliases]
/// fast = "groq/llama-3.3-70b-versatile"
/// smart = { model = "anthropic/claude-opus-5-5", fallback = ["openai/gpt-5"] }
/// ```
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    /// Providers by name. Entries named like a built-in preset are merged
    /// over it, so `[providers.anthropic] api_key_env = "MY_KEY"` is enough.
    pub providers: BTreeMap<String, ProviderConfig>,
    pub aliases: BTreeMap<String, AliasConfig>,
    /// Virtual models that pick a real model per request: `router/<name>`.
    pub routers: BTreeMap<String, crate::router::RouterConfig>,
    /// Skip the built-in presets (anthropic, openai, ollama, ...).
    pub no_presets: bool,
    pub retry: RetryConfig,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// Wire format: `anthropic`, `openai`, `mock`, or one registered by an
    /// SDK user.
    pub dialect: String,
    pub base_url: Option<String>,
    /// Name of the environment variable holding the API key.
    pub api_key_env: Option<String>,
    /// A literal key. Prefer `api_key_env` so keys stay out of config files.
    #[serde(skip_serializing)]
    pub api_key: Option<SecretString>,
    /// Header that carries the key. Defaults to the dialect's usual one.
    pub auth_header: Option<String>,
    /// Prefix before the key in that header, e.g. `Bearer`. Empty for none.
    pub auth_scheme: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub query: BTreeMap<String, String>,
    /// Fields merged into every request body.
    pub extra_body: Map<String, Value>,
    pub timeout_secs: Option<u64>,
    /// Capabilities assumed for models of this provider not listed below.
    pub default_capabilities: Option<Capabilities>,
    pub models: Vec<ModelConfig>,
    /// Dialect-specific switches; see each dialect's docs.
    pub options: Map<String, Value>,
    pub enabled: Option<bool>,
}

impl ProviderConfig {
    pub fn new(dialect: &str, base_url: &str) -> Self {
        Self {
            dialect: dialect.to_owned(),
            base_url: Some(base_url.to_owned()),
            ..Default::default()
        }
    }

    pub fn key_env(mut self, var: &str) -> Self {
        self.api_key_env = Some(var.to_owned());
        self
    }

    pub fn option(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.options.insert(key.to_owned(), value.into());
        self
    }

    /// The API key, from the literal or the environment.
    pub fn resolve_key(&self) -> Option<SecretString> {
        if let Some(k) = &self.api_key {
            return Some(k.clone());
        }
        let var = self.api_key_env.as_deref()?;
        std::env::var(var)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(SecretString::from)
    }

    /// A provider needs a key when it names a variable for one.
    pub fn key_status(&self) -> KeyStatus {
        match (&self.api_key, &self.api_key_env) {
            (Some(_), _) => KeyStatus::Present,
            (None, None) => KeyStatus::NotNeeded,
            (None, Some(_)) if self.resolve_key().is_some() => KeyStatus::Present,
            (None, Some(var)) => KeyStatus::Missing(var.clone()),
        }
    }

    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs.unwrap_or(600))
    }

    pub fn option_bool(&self, key: &str) -> Option<bool> {
        self.options.get(key).and_then(Value::as_bool)
    }

    pub fn option_str(&self, key: &str) -> Option<&str> {
        self.options.get(key).and_then(Value::as_str)
    }

    pub fn model(&self, id: &str) -> Option<&ModelConfig> {
        self.models.iter().find(|m| m.id == id)
    }

    /// Overlay `other` (user config) on `self` (a preset).
    pub fn merge(mut self, other: ProviderConfig) -> Self {
        if !other.dialect.is_empty() {
            self.dialect = other.dialect;
        }
        macro_rules! take {
            ($($f:ident),*) => { $( if other.$f.is_some() { self.$f = other.$f; } )* };
        }
        take!(
            base_url,
            api_key_env,
            api_key,
            auth_header,
            auth_scheme,
            timeout_secs,
            default_capabilities,
            enabled
        );
        self.headers.extend(other.headers);
        self.query.extend(other.query);
        self.extra_body.extend(other.extra_body);
        self.options.extend(other.options);
        for m in other.models {
            match self.models.iter_mut().find(|x| x.id == m.id) {
                Some(existing) => *existing = m,
                None => self.models.push(m),
            }
        }
        self
    }

    pub(crate) fn require_key(&self, provider: &str) -> Result<Option<String>, LlmError> {
        match self.key_status() {
            KeyStatus::Missing(var) => Err(LlmError::Auth(format!(
                "provider {provider:?} needs an API key: set the {var} environment variable"
            ))),
            _ => Ok(self.resolve_key().map(|k| k.expose_secret().to_owned())),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyStatus {
    Present,
    NotNeeded,
    Missing(String),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ModelConfig {
    /// The provider's own model name.
    pub id: String,
    #[serde(default)]
    pub kind: ModelKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Capabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing: Option<Pricing>,
    /// Prices for speech and image models (per minute, per million
    /// characters, per image).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_pricing: Option<crate::MediaPricing>,
}

impl ModelConfig {
    pub fn chat(id: &str, capabilities: Capabilities, pricing: Option<Pricing>) -> Self {
        Self {
            id: id.to_owned(),
            kind: ModelKind::Chat,
            capabilities: Some(capabilities),
            pricing,
            media_pricing: None,
        }
    }

    /// A speech or image model.
    pub fn media(id: &str, kind: ModelKind, pricing: crate::MediaPricing) -> Self {
        Self {
            id: id.to_owned(),
            kind,
            capabilities: None,
            pricing: None,
            media_pricing: Some(pricing),
        }
    }
}

/// An alias for a model, optionally with models to fall back to.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AliasConfig {
    Model(String),
    Chain {
        model: String,
        #[serde(default)]
        fallback: Vec<String>,
    },
}

impl AliasConfig {
    pub fn chain(&self) -> Vec<&str> {
        match self {
            Self::Model(m) => vec![m],
            Self::Chain { model, fallback } => std::iter::once(model.as_str())
                .chain(fallback.iter().map(String::as_str))
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff_ms: 500,
            max_backoff_ms: 30_000,
        }
    }
}
