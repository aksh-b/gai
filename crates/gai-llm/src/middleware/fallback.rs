use async_trait::async_trait;

use super::open;
use crate::{ChatModel, ChatRequest, ChatStream, LlmError, ModelInfo, SharedChatModel};

/// Tries each model in order until one starts responding. Reports the first
/// model's info, since that is what callers asked for.
pub struct Fallback {
    models: Vec<SharedChatModel>,
}

impl Fallback {
    pub fn new(models: Vec<SharedChatModel>) -> Self {
        assert!(
            !models.is_empty(),
            "a fallback chain needs at least one model"
        );
        Self { models }
    }
}

#[async_trait]
impl ChatModel for Fallback {
    fn info(&self) -> &ModelInfo {
        self.models[0].info()
    }

    async fn stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        let mut last = None;
        for (i, model) in self.models.iter().enumerate() {
            match open(model.as_ref(), req.clone()).await {
                Ok(s) => return Ok(s),
                Err(e) if e.is_fallbackable() && i + 1 < self.models.len() => {
                    tracing::warn!(from = %model.info().id, to = %self.models[i + 1].info().id, error = %e, "falling back");
                    last = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| LlmError::Config("empty fallback chain".into())))
    }

    async fn count_tokens(&self, req: &ChatRequest) -> Result<Option<u64>, LlmError> {
        self.models[0].count_tokens(req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::mock::{MockModel, text_response};
    use std::sync::Arc;

    #[tokio::test]
    async fn moves_to_the_next_model_on_provider_failure() {
        let a = MockModel::scripted_results(vec![Err(LlmError::Auth("no key".into()))]);
        let b = MockModel::scripted(vec![text_response("from b")]);
        let f = Fallback::new(vec![Arc::new(a), Arc::new(b)]);
        assert_eq!(
            f.complete(ChatRequest::user("x")).await.unwrap().text(),
            "from b"
        );
    }

    #[tokio::test]
    async fn invalid_requests_do_not_fall_back() {
        let a = MockModel::scripted_results(vec![Err(LlmError::InvalidRequest("bad".into()))]);
        let b = MockModel::scripted(vec![text_response("from b")]);
        let f = Fallback::new(vec![Arc::new(a), Arc::new(b)]);
        assert!(f.complete(ChatRequest::user("x")).await.is_err());
    }
}
