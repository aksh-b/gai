use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, RwLock};

use secrecy::{ExposeSecret, SecretString};
use serde::Serialize;

use crate::dialect::{self, Dialect, Provider, RemoteModel};
use crate::middleware::{Fallback, Metered, Retry, UsageSink};
use crate::router::{RouterConfig, RouterModel};
use crate::{
    AliasConfig, KeyStatus, LlmConfig, LlmError, ModelConfig, ModelInfo, ModelKind, ProviderConfig,
    RetryConfig, SharedChatModel, SharedEmbeddingModel, SharedImageModel, SharedSpeechToText,
    SharedTextToSpeech,
};

/// Resolves model addresses (`provider/model` or an alias) to ready-to-use
/// model handles.
///
/// Providers are built on first use, so a missing API key only matters for
/// the providers you actually call.
pub struct ModelRegistry {
    configs: BTreeMap<String, ProviderConfig>,
    aliases: BTreeMap<String, AliasConfig>,
    routers: BTreeMap<String, RouterConfig>,
    dialects: HashMap<String, Arc<dyn Dialect>>,
    built: Mutex<HashMap<String, Arc<dyn Provider>>>,
    /// Keys set at runtime (say, from the dashboard). They win over the
    /// config's literal key and environment variable.
    stored_keys: RwLock<HashMap<String, SecretString>>,
    /// Catalog entries added at runtime (say, a model just pulled into
    /// Ollama), by provider. Entries in config win over these.
    added_models: RwLock<BTreeMap<String, Vec<ModelConfig>>>,
    retry: RetryConfig,
    sink: Option<Arc<dyn UsageSink>>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
pub struct CatalogEntry {
    #[serde(flatten)]
    pub info: ModelInfo,
    pub key: KeyState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case", tag = "status", content = "env")]
pub enum KeyState {
    Present,
    NotNeeded,
    Missing(String),
}

impl From<KeyStatus> for KeyState {
    fn from(k: KeyStatus) -> Self {
        match k {
            KeyStatus::Present => Self::Present,
            KeyStatus::NotNeeded => Self::NotNeeded,
            KeyStatus::Missing(v) => Self::Missing(v),
        }
    }
}

#[derive(Default)]
pub struct RegistryBuilder {
    config: LlmConfig,
    dialects: Vec<Arc<dyn Dialect>>,
    providers: Vec<(String, Arc<dyn Provider>)>,
    sink: Option<Arc<dyn UsageSink>>,
}

impl RegistryBuilder {
    pub fn config(mut self, config: LlmConfig) -> Self {
        self.config = config;
        self
    }

    /// Add a wire format. Providers in config can then use `dialect = "<name>"`.
    pub fn dialect(mut self, dialect: Arc<dyn Dialect>) -> Self {
        self.dialects.push(dialect);
        self
    }

    /// Add an already-built provider under `name`.
    pub fn provider(mut self, name: &str, provider: Arc<dyn Provider>) -> Self {
        self.providers.push((name.to_owned(), provider));
        self
    }

    /// Report usage and cost of every model call to `sink`.
    pub fn usage_sink(mut self, sink: Arc<dyn UsageSink>) -> Self {
        self.sink = Some(sink);
        self
    }

    pub fn build(self) -> Result<ModelRegistry, LlmError> {
        let mut configs = if self.config.no_presets {
            BTreeMap::new()
        } else {
            crate::presets::presets()
        };
        for (name, cfg) in self.config.providers {
            let merged = match configs.remove(&name) {
                Some(preset) => preset.merge(cfg),
                None => cfg,
            };
            configs.insert(name, merged);
        }
        configs.retain(|_, c| c.enabled != Some(false));

        let mut dialects: HashMap<String, Arc<dyn Dialect>> = HashMap::new();
        for d in dialect::builtin().into_iter().chain(self.dialects) {
            dialects.insert(d.name().to_owned(), d);
        }
        for (name, cfg) in &configs {
            if !dialects.contains_key(&cfg.dialect) {
                return Err(LlmError::Config(format!(
                    "provider {name:?} uses unknown dialect {:?}; known: {}",
                    cfg.dialect,
                    dialects.keys().cloned().collect::<Vec<_>>().join(", ")
                )));
            }
        }
        let mut built = HashMap::new();
        for (name, p) in self.providers {
            configs
                .entry(name.clone())
                .or_insert_with(|| ProviderConfig {
                    dialect: "custom".into(),
                    ..Default::default()
                });
            built.insert(name, p);
        }
        for (alias, target) in &self.config.aliases {
            if alias.contains('/') {
                return Err(LlmError::Config(format!(
                    "alias {alias:?} must not contain '/'"
                )));
            }
            if target.chain().is_empty() {
                return Err(LlmError::Config(format!("alias {alias:?} has no model")));
            }
        }
        for (name, r) in &self.config.routers {
            if r.models.is_empty() {
                return Err(LlmError::Config(format!(
                    "router {name:?} has no models to choose from"
                )));
            }
        }
        if configs.contains_key(crate::router::PROVIDER) && !self.config.routers.is_empty() {
            return Err(LlmError::Config(format!(
                "a provider named {:?} would hide the model routers",
                crate::router::PROVIDER
            )));
        }
        Ok(ModelRegistry {
            configs,
            aliases: self.config.aliases,
            routers: self.config.routers,
            dialects,
            built: Mutex::new(built),
            stored_keys: RwLock::new(HashMap::new()),
            added_models: RwLock::new(BTreeMap::new()),
            retry: self.config.retry,
            sink: self.sink,
        })
    }
}

impl ModelRegistry {
    pub fn builder() -> RegistryBuilder {
        RegistryBuilder::default()
    }

    pub fn from_config(config: LlmConfig) -> Result<Self, LlmError> {
        Self::builder().config(config).build()
    }

    /// A chat model by address, with retries, the alias's fallbacks and usage
    /// metering applied. `router/<name>` gives a model that picks one of the
    /// router's models per request.
    pub fn chat(&self, spec: &str) -> Result<SharedChatModel, LlmError> {
        let model = match self.router_name(spec) {
            Some(name) => self.router(name)?,
            None => self.chat_unmetered(spec)?,
        };
        Ok(match &self.sink {
            Some(sink) => Arc::new(Metered::new(model, sink.clone())),
            None => model,
        })
    }

    /// The router a spec names (`router/<name>`, or an alias of one).
    fn router_name<'a>(&'a self, spec: &'a str) -> Option<&'a str> {
        let spec = match self.aliases.get(spec) {
            Some(a) => a.chain()[0],
            None => spec,
        };
        spec.strip_prefix(crate::router::PREFIX)
            .filter(|n| self.routers.contains_key(*n))
    }

    fn router(&self, name: &str) -> Result<SharedChatModel, LlmError> {
        let cfg = &self.routers[name];
        let mut candidates = Vec::new();
        let mut skipped = Vec::new();
        for c in &cfg.models {
            if self.router_name(&c.model).is_some() {
                return Err(LlmError::Config(format!(
                    "router {name:?} lists another router ({}); routers cannot nest",
                    c.model
                )));
            }
            match self.chat_unmetered(&c.model) {
                Ok(m) => candidates.push((c.clone(), m)),
                // A model without a key is just not a choice.
                Err(e) => skipped.push(format!("{}: {e}", c.model)),
            }
        }
        if candidates.is_empty() {
            return Err(LlmError::Config(format!(
                "router {name:?} has no usable models ({})",
                skipped.join("; ")
            )));
        }
        let classifier = match &cfg.classifier {
            Some(c) => Some(self.chat_unmetered(c)?),
            None => None,
        };
        Ok(Arc::new(RouterModel::new(
            self.router_info(name)?,
            cfg.clone(),
            candidates,
            classifier,
        )))
    }

    /// What a router can do: anything one of its models can.
    fn router_info(&self, name: &str) -> Result<ModelInfo, LlmError> {
        let cfg = self
            .routers
            .get(name)
            .ok_or_else(|| LlmError::Config(format!("unknown router {name:?}")))?;
        let mut info = ModelInfo::new(crate::router::PROVIDER, name);
        let c = &mut info.capabilities;
        *c = crate::Capabilities {
            tools: false,
            ..Default::default()
        };
        for m in &cfg.models {
            let Ok(i) = self.resolve(&m.model) else {
                continue;
            };
            let x = &i.capabilities;
            c.tools |= x.tools;
            c.vision |= x.vision;
            c.reasoning |= x.reasoning;
            c.json_schema |= x.json_schema;
            c.audio |= x.audio;
            c.context_window = c.context_window.max(x.context_window);
            c.max_output_tokens = c.max_output_tokens.max(x.max_output_tokens);
        }
        Ok(info)
    }

    fn chat_unmetered(&self, spec: &str) -> Result<SharedChatModel, LlmError> {
        let chain = self.chain(spec)?;
        let mut models = Vec::with_capacity(chain.len());
        for s in &chain {
            let info = self.resolve(s)?;
            let raw = match self
                .provider(&info.provider)
                .and_then(|p| p.chat(info.clone()))
            {
                Ok(m) => m,
                // In a chain, an unusable member (say, no API key) should
                // fall through at call time rather than break the alias.
                Err(e) if chain.len() > 1 => Arc::new(Unavailable {
                    info,
                    error: e.to_string(),
                }),
                Err(e) => return Err(e),
            };
            models.push(Arc::new(Retry::new(raw, self.retry.clone())) as SharedChatModel);
        }
        Ok(if models.len() == 1 {
            models.pop().expect("one model")
        } else {
            Arc::new(Fallback::new(models))
        })
    }

    pub fn embedding(&self, spec: &str) -> Result<SharedEmbeddingModel, LlmError> {
        let info = self.media_info(spec, ModelKind::Embedding)?;
        self.provider(&info.provider)?.embedding(info)
    }

    /// A speech-to-text model by address.
    pub fn transcription(&self, spec: &str) -> Result<SharedSpeechToText, LlmError> {
        let info = self.media_info(spec, ModelKind::Transcription)?;
        self.provider(&info.provider)?.transcription(info)
    }

    /// A text-to-speech model by address.
    pub fn speech(&self, spec: &str) -> Result<SharedTextToSpeech, LlmError> {
        let info = self.media_info(spec, ModelKind::Speech)?;
        self.provider(&info.provider)?.speech(info)
    }

    /// An image generation model by address.
    pub fn image(&self, spec: &str) -> Result<SharedImageModel, LlmError> {
        let info = self.media_info(spec, ModelKind::Image)?;
        self.provider(&info.provider)?.image(info)
    }

    fn media_info(&self, spec: &str, kind: ModelKind) -> Result<ModelInfo, LlmError> {
        let chain = self.chain(spec)?;
        let mut info = self.resolve(chain[0])?;
        info.kind = kind;
        if kind == ModelKind::Embedding {
            let cfg = self.configs[&info.provider].model(&info.model);
            let (q, d) = (
                cfg.and_then(|m| m.query_prefix.clone()),
                cfg.and_then(|m| m.document_prefix.clone()),
            );
            info.embed_prefixes = if q.is_some() || d.is_some() {
                Some(crate::EmbedPrefixes {
                    query: q.unwrap_or_default(),
                    document: d.unwrap_or_default(),
                })
            } else {
                crate::EmbedPrefixes::known(&info.model)
            };
        }
        Ok(info)
    }

    pub fn routers(&self) -> &BTreeMap<String, RouterConfig> {
        &self.routers
    }

    /// Resolve an address to model info without building anything.
    ///
    /// Accepts `provider/model`, an alias, or a bare model name listed in
    /// exactly one provider's catalog (`claude-opus-5-5`).
    pub fn resolve(&self, spec: &str) -> Result<ModelInfo, LlmError> {
        if let Some(name) = self.router_name(spec) {
            return self.router_info(name);
        }
        let spec = match self.aliases.get(spec) {
            Some(a) => a.chain()[0].to_owned(),
            None => spec.to_owned(),
        };
        let (provider, model) = match spec.split_once('/') {
            Some((p, m)) if self.configs.contains_key(p) => (p.to_owned(), m.to_owned()),
            Some((p, _)) => {
                return Err(LlmError::Config(format!(
                    "unknown provider {p:?} in {spec:?}; configured: {}",
                    self.configs.keys().cloned().collect::<Vec<_>>().join(", ")
                )));
            }
            None => (self.provider_listing(&spec)?, spec.clone()),
        };
        let cfg = &self.configs[&provider];
        let mut info = ModelInfo::new(&provider, &model);
        let added = self.added_model(&provider, &model);
        match cfg.model(&model).or(added.as_ref()) {
            Some(m) => {
                info.kind = m.kind;
                info.media_pricing = m.media_pricing;
                info.capabilities = m
                    .capabilities
                    .clone()
                    .or_else(|| cfg.default_capabilities.clone())
                    .unwrap_or_default();
                info.pricing = m.pricing;
            }
            None => info.capabilities = cfg.default_capabilities.clone().unwrap_or_default(),
        }
        Ok(info)
    }

    fn provider_listing(&self, model: &str) -> Result<String, LlmError> {
        let owners: Vec<&String> = self
            .configs
            .iter()
            .filter(|(_, c)| c.model(model).is_some())
            .map(|(n, _)| n)
            .collect();
        match owners.as_slice() {
            [one] => Ok((*one).clone()),
            [] => Err(LlmError::Config(format!(
                "{model:?} is not a known model or alias; use provider/model"
            ))),
            many => Err(LlmError::Config(format!(
                "{model:?} is offered by several providers ({}); use provider/model",
                many.iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }

    fn chain<'a>(&'a self, spec: &'a str) -> Result<Vec<&'a str>, LlmError> {
        Ok(match self.aliases.get(spec) {
            Some(a) => a.chain(),
            None => vec![spec],
        })
    }

    fn provider(&self, name: &str) -> Result<Arc<dyn Provider>, LlmError> {
        let mut built = self.built.lock().expect("registry lock");
        if let Some(p) = built.get(name) {
            return Ok(p.clone());
        }
        let cfg = self
            .configs
            .get(name)
            .ok_or_else(|| LlmError::Config(format!("unknown provider {name:?}")))?;
        let stored = self.stored_key(name);
        let p = match stored {
            Some(key) => {
                let mut cfg = cfg.clone();
                cfg.api_key = Some(key);
                self.dialects[&cfg.dialect].build(name, &cfg)?
            }
            None => self.dialects[&cfg.dialect].build(name, cfg)?,
        };
        built.insert(name.to_owned(), p.clone());
        Ok(p)
    }

    fn stored_key(&self, provider: &str) -> Option<SecretString> {
        self.stored_keys
            .read()
            .expect("stored keys lock")
            .get(provider)
            .cloned()
    }

    /// Use `key` for `provider` from now on (`None` forgets it, falling back
    /// to the configured key). Takes effect for the next model handle.
    pub fn set_key(&self, provider: &str, key: Option<SecretString>) -> Result<(), LlmError> {
        if !self.configs.contains_key(provider) {
            return Err(LlmError::Config(format!("unknown provider {provider:?}")));
        }
        {
            let mut keys = self.stored_keys.write().expect("stored keys lock");
            match key {
                Some(k) if !k.expose_secret().trim().is_empty() => {
                    keys.insert(provider.to_owned(), k);
                }
                _ => {
                    keys.remove(provider);
                }
            }
        }
        self.built.lock().expect("registry lock").remove(provider);
        Ok(())
    }

    /// The last four characters of a key set with [`ModelRegistry::set_key`].
    pub fn stored_key_hint(&self, provider: &str) -> Option<String> {
        let key = self.stored_key(provider)?;
        let k = key.expose_secret();
        let tail: String = k
            .chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        Some(tail)
    }

    /// Whether `provider` has the key it needs, counting stored keys.
    pub fn key_status(&self, provider: &str) -> KeyStatus {
        if self.stored_key(provider).is_some() {
            return KeyStatus::Present;
        }
        match self.configs.get(provider) {
            Some(c) => c.key_status(),
            None => KeyStatus::NotNeeded,
        }
    }

    fn added_model(&self, provider: &str, id: &str) -> Option<ModelConfig> {
        self.added_models
            .read()
            .expect("added models lock")
            .get(provider)?
            .iter()
            .find(|m| m.id == id)
            .cloned()
    }

    /// Add a model to a provider's catalog (replacing one with the same id
    /// that was added before). Models listed in config are left as they are.
    pub fn add_model(&self, provider: &str, model: ModelConfig) -> Result<(), LlmError> {
        if !self.configs.contains_key(provider) {
            return Err(LlmError::Config(format!("unknown provider {provider:?}")));
        }
        let mut added = self.added_models.write().expect("added models lock");
        let list = added.entry(provider.to_owned()).or_default();
        list.retain(|m| m.id != model.id);
        list.push(model);
        Ok(())
    }

    /// Every model listed in provider catalogs, with key status.
    pub fn catalog(&self) -> Vec<CatalogEntry> {
        let mut out = Vec::new();
        let added = self.added_models.read().expect("added models lock").clone();
        for (name, cfg) in &self.configs {
            let extra = added
                .get(name)
                .into_iter()
                .flatten()
                .filter(|m| cfg.model(&m.id).is_none());
            for m in cfg.models.iter().chain(extra) {
                if let Ok(info) = self.resolve(&format!("{name}/{}", m.id)) {
                    out.push(CatalogEntry {
                        info,
                        key: self.key_status(name).into(),
                    });
                }
            }
        }
        for name in self.routers.keys() {
            if let Ok(info) = self.router_info(name) {
                out.push(CatalogEntry {
                    info,
                    key: KeyState::NotNeeded,
                });
            }
        }
        out
    }

    pub fn providers(&self) -> impl Iterator<Item = (&str, &ProviderConfig)> {
        self.configs.iter().map(|(n, c)| (n.as_str(), c))
    }

    pub fn aliases(&self) -> &BTreeMap<String, AliasConfig> {
        &self.aliases
    }

    /// Ask a provider which models it serves right now.
    pub async fn discover(&self, provider: &str) -> Result<Vec<RemoteModel>, LlmError> {
        self.provider(provider)?.list_models().await
    }

    /// A sensible default when nothing is configured: the first provider
    /// with a usable key.
    pub fn default_model(&self) -> Option<String> {
        let ready =
            |p: &str| self.configs.contains_key(p) && self.key_status(p) == KeyStatus::Present;
        [("anthropic", "claude-opus-5-5")]
            .iter()
            .find(|(p, _)| ready(p))
            .map(|(p, m)| format!("{p}/{m}"))
    }
}

/// Stands in for a fallback-chain member that could not be built.
struct Unavailable {
    info: ModelInfo,
    error: String,
}

#[async_trait::async_trait]
impl crate::ChatModel for Unavailable {
    fn info(&self) -> &ModelInfo {
        &self.info
    }

    async fn stream(&self, _req: crate::ChatRequest) -> Result<crate::ChatStream, LlmError> {
        Err(LlmError::Config(self.error.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChatRequest;

    fn registry(toml_like: LlmConfig) -> ModelRegistry {
        ModelRegistry::from_config(toml_like).unwrap()
    }

    #[test]
    fn resolves_catalog_bare_and_unlisted_models() {
        let r = registry(LlmConfig::default());
        let info = r.resolve("anthropic/claude-opus-5-5").unwrap();
        assert_eq!(info.capabilities.context_window, Some(1_000_000));
        assert!(info.pricing.is_some());
        assert_eq!(
            r.resolve("claude-sonnet-5-5").unwrap().id,
            "anthropic/claude-sonnet-5-5"
        );
        let unknown = r.resolve("ollama/qwen3:32b").unwrap();
        assert_eq!(unknown.model, "qwen3:32b");
        assert!(r.resolve("nope/x").is_err());
        assert!(r.resolve("just-a-name").is_err());
    }

    #[test]
    fn user_config_merges_over_presets() {
        let mut cfg = LlmConfig::default();
        cfg.providers.insert(
            "anthropic".into(),
            ProviderConfig {
                api_key_env: Some("MY_KEY".into()),
                ..Default::default()
            },
        );
        cfg.providers.insert(
            "groq".into(),
            ProviderConfig {
                enabled: Some(false),
                ..Default::default()
            },
        );
        let r = registry(cfg);
        let (_, a) = r.providers().find(|(n, _)| *n == "anthropic").unwrap();
        assert_eq!(a.api_key_env.as_deref(), Some("MY_KEY"));
        assert_eq!(a.dialect, "anthropic");
        assert!(!a.models.is_empty());
        assert!(r.providers().all(|(n, _)| n != "groq"));
    }

    #[tokio::test]
    async fn aliases_build_fallback_chains() {
        let mut cfg = LlmConfig::default();
        cfg.providers.insert(
            "nokey".into(),
            ProviderConfig::new("openai", "http://127.0.0.1:9").key_env("GAI_TEST_UNSET_KEY"),
        );
        cfg.aliases.insert(
            "dev".into(),
            AliasConfig::Chain {
                model: "nokey/m".into(),
                fallback: vec!["mock/echo".into()],
            },
        );
        let r = registry(cfg);
        let m = r.chat("dev").unwrap();
        assert_eq!(m.info().id, "nokey/m");
        // The first model has no key, so the chain falls through to echo.
        let resp = m.complete(ChatRequest::user("hello")).await.unwrap();
        assert_eq!(resp.text(), "hello");
    }

    #[test]
    fn unknown_dialect_is_a_config_error() {
        let mut cfg = LlmConfig::default();
        cfg.providers.insert(
            "x".into(),
            ProviderConfig {
                dialect: "smoke-signals".into(),
                ..Default::default()
            },
        );
        assert!(matches!(
            ModelRegistry::from_config(cfg),
            Err(LlmError::Config(_))
        ));
    }
}
