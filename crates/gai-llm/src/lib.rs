//! The model layer: one interface over every model provider.
//!
//! - [`ChatModel`] / [`EmbeddingModel`] are the traits the rest of gai uses,
//!   with [`SpeechToText`], [`TextToSpeech`] and [`ImageModel`] for media.
//! - `router/<name>` models ([`router`]) pick a real model per request.
//! - Dialects ([`dialect`]) translate to wire formats: Anthropic Messages,
//!   OpenAI Chat Completions (which also covers Gemini, Groq, OpenRouter,
//!   DeepSeek, Mistral, Together, xAI, Ollama, vLLM, LM Studio...), and a
//!   mock for tests.
//! - Providers are declared in config ([`LlmConfig`]); any endpoint that
//!   speaks a known dialect works without code.
//! - [`ModelRegistry`] resolves `provider/model` addresses and aliases and
//!   wraps models in retry, fallback and usage [`middleware`].

mod config;
pub mod dialect;
mod error;
pub mod http;
mod media;
pub mod middleware;
mod model;
mod presets;
mod registry;
pub mod router;

pub use config::{AliasConfig, KeyStatus, LlmConfig, ModelConfig, ProviderConfig, RetryConfig};
pub use dialect::mock::{MockModel, text_response, tool_response};
pub use dialect::{Dialect, Provider, RemoteModel};
pub use error::LlmError;
pub use media::*;
pub use middleware::{UsageRecord, UsageSink};
pub use model::*;
pub use presets::presets;
pub use registry::{CatalogEntry, KeyState, ModelRegistry, RegistryBuilder};
