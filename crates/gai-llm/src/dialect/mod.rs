//! Dialects translate gai's neutral request and event types to and from one
//! wire format. A configured instance of a dialect is a [`Provider`].

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    LlmError, ModelInfo, ProviderConfig, RealtimeTarget, SharedChatModel, SharedEmbeddingModel,
    SharedImageModel, SharedSpeechToText, SharedTextToSpeech,
};

pub mod anthropic;
pub mod elevenlabs;
pub mod mock;
pub mod openai;

/// A configured endpoint that hands out model handles.
#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;

    fn chat(&self, info: ModelInfo) -> Result<SharedChatModel, LlmError>;

    fn embedding(&self, info: ModelInfo) -> Result<SharedEmbeddingModel, LlmError> {
        Err(LlmError::Unsupported(format!(
            "provider {:?} has no embedding models ({})",
            self.name(),
            info.id
        )))
    }

    fn transcription(&self, info: ModelInfo) -> Result<SharedSpeechToText, LlmError> {
        Err(LlmError::Unsupported(format!(
            "provider {:?} has no speech-to-text models ({})",
            self.name(),
            info.id
        )))
    }

    fn speech(&self, info: ModelInfo) -> Result<SharedTextToSpeech, LlmError> {
        Err(LlmError::Unsupported(format!(
            "provider {:?} has no text-to-speech models ({})",
            self.name(),
            info.id
        )))
    }

    fn image(&self, info: ModelInfo) -> Result<SharedImageModel, LlmError> {
        Err(LlmError::Unsupported(format!(
            "provider {:?} has no image models ({})",
            self.name(),
            info.id
        )))
    }

    /// Where to open a live speech session with this model.
    fn realtime(&self, info: ModelInfo) -> Result<RealtimeTarget, LlmError> {
        Err(LlmError::Unsupported(format!(
            "provider {:?} has no realtime models ({})",
            self.name(),
            info.id
        )))
    }

    /// Models the provider reports as available, when it can list them.
    async fn list_models(&self) -> Result<Vec<RemoteModel>, LlmError> {
        Ok(Vec::new())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RemoteModel {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}

/// Builds providers of one dialect from config. Register your own with
/// [`RegistryBuilder::dialect`](crate::RegistryBuilder::dialect) to support a
/// wire format gai does not ship.
pub trait Dialect: Send + Sync {
    fn name(&self) -> &str;
    fn build(&self, provider: &str, cfg: &ProviderConfig) -> Result<Arc<dyn Provider>, LlmError>;
}

pub fn builtin() -> Vec<Arc<dyn Dialect>> {
    vec![
        Arc::new(anthropic::AnthropicDialect),
        Arc::new(openai::OpenAiDialect),
        Arc::new(elevenlabs::ElevenLabsDialect),
        Arc::new(mock::MockDialect),
    ]
}
