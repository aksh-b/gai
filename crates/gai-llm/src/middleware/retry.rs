use std::time::Duration;

use async_trait::async_trait;

use super::open;
use crate::{
    ChatModel, ChatRequest, ChatStream, LlmError, ModelInfo, RetryConfig, SharedChatModel,
};

/// Retries transient failures (rate limits, overload, network) with
/// exponential backoff and jitter, honouring `retry-after`.
pub struct Retry {
    inner: SharedChatModel,
    cfg: RetryConfig,
}

impl Retry {
    pub fn new(inner: SharedChatModel, cfg: RetryConfig) -> Self {
        Self { inner, cfg }
    }

    fn backoff(&self, attempt: u32, err: &LlmError) -> Duration {
        if let Some(d) = err.retry_after() {
            return d.min(Duration::from_millis(self.cfg.max_backoff_ms));
        }
        let base = self
            .cfg
            .initial_backoff_ms
            .saturating_mul(1 << attempt.min(16));
        let capped = base.min(self.cfg.max_backoff_ms);
        // Full jitter between half and all of the delay.
        Duration::from_millis(capped / 2 + fastrand::u64(0..=capped / 2))
    }
}

#[async_trait]
impl ChatModel for Retry {
    fn info(&self) -> &ModelInfo {
        self.inner.info()
    }

    async fn stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        let mut attempt = 0;
        loop {
            match open(self.inner.as_ref(), req.clone()).await {
                Ok(s) => return Ok(s),
                Err(e) if e.is_retryable() && attempt < self.cfg.max_retries => {
                    let wait = self.backoff(attempt, &e);
                    tracing::warn!(model = %self.info().id, attempt, ?wait, error = %e, "retrying model call");
                    tokio::time::sleep(wait).await;
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn count_tokens(&self, req: &ChatRequest) -> Result<Option<u64>, LlmError> {
        self.inner.count_tokens(req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::mock::{MockModel, text_response};
    use std::sync::Arc;

    #[tokio::test]
    async fn retries_transient_errors_then_succeeds() {
        let mock = MockModel::scripted_results(vec![
            Err(LlmError::Provider {
                status: 529,
                message: "overloaded".into(),
            }),
            Err(LlmError::RateLimited {
                retry_after: Some(Duration::from_millis(1)),
                message: String::new(),
            }),
            Ok(text_response("ok")),
        ]);
        let cfg = RetryConfig {
            max_retries: 3,
            initial_backoff_ms: 1,
            max_backoff_ms: 2,
        };
        let r = Retry::new(Arc::new(mock), cfg)
            .complete(ChatRequest::user("x"))
            .await
            .unwrap();
        assert_eq!(r.text(), "ok");
    }

    #[tokio::test]
    async fn does_not_retry_bad_requests() {
        let mock = MockModel::scripted_results(vec![
            Err(LlmError::InvalidRequest("bad".into())),
            Ok(text_response("never")),
        ]);
        let r = Retry::new(Arc::new(mock), RetryConfig::default())
            .complete(ChatRequest::user("x"))
            .await;
        assert!(matches!(r, Err(LlmError::InvalidRequest(_))));
    }
}
