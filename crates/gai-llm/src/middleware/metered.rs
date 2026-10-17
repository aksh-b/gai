use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use gai_core::Usage;
use serde::{Deserialize, Serialize};

use crate::{ChatEvent, ChatModel, ChatRequest, ChatStream, LlmError, ModelInfo, SharedChatModel};

/// One model call's usage and cost.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageRecord {
    pub model: String,
    pub usage: Usage,
    /// USD, when the model has pricing.
    pub cost: Option<f64>,
}

/// Receives a record after every metered call: stores, budgets, logs.
pub trait UsageSink: Send + Sync {
    fn record(&self, record: UsageRecord);
}

impl<F: Fn(UsageRecord) + Send + Sync> UsageSink for F {
    fn record(&self, record: UsageRecord) {
        self(record)
    }
}

/// Reports usage and cost to a [`UsageSink`] once each stream ends, whether
/// it was read to completion or dropped early.
pub struct Metered {
    inner: SharedChatModel,
    sink: Arc<dyn UsageSink>,
}

impl Metered {
    pub fn new(inner: SharedChatModel, sink: Arc<dyn UsageSink>) -> Self {
        Self { inner, sink }
    }
}

struct Report {
    usage: Arc<Mutex<Usage>>,
    info: ModelInfo,
    sink: Arc<dyn UsageSink>,
}

impl Drop for Report {
    fn drop(&mut self) {
        let usage = *self.usage.lock().expect("usage lock");
        if usage.total() > 0 {
            self.sink.record(UsageRecord {
                model: self.info.id.clone(),
                cost: self.info.cost(&usage),
                usage,
            });
        }
    }
}

#[async_trait]
impl ChatModel for Metered {
    fn info(&self) -> &ModelInfo {
        self.inner.info()
    }

    async fn stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        let stream = self.inner.stream(req).await?;
        let usage = Arc::new(Mutex::new(Usage::default()));
        let report = Report {
            usage: usage.clone(),
            info: self.info().clone(),
            sink: self.sink.clone(),
        };
        Ok(Box::pin(stream.map(move |item| {
            let _keep_alive = &report;
            if let Ok(ChatEvent::Usage { usage: u }) = &item {
                usage.lock().expect("usage lock").merge_max(u);
            }
            item
        })))
    }

    async fn count_tokens(&self, req: &ChatRequest) -> Result<Option<u64>, LlmError> {
        self.inner.count_tokens(req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::mock::{MockModel, text_response};
    use gai_core::Pricing;

    #[tokio::test]
    async fn records_usage_and_cost_once_per_call() {
        let records = Arc::new(Mutex::new(Vec::new()));
        let sink = {
            let records = records.clone();
            move |r: UsageRecord| records.lock().unwrap().push(r)
        };
        let mut info = ModelInfo::new("mock", "scripted");
        info.pricing = Some(Pricing::new(1_000_000.0, 1_000_000.0));
        let model = MockModel::scripted(vec![text_response("a")]).with_info(info);
        let metered = Metered::new(Arc::new(model), Arc::new(sink));
        metered.complete(ChatRequest::user("x")).await.unwrap();
        let records = records.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].usage.input_tokens, 10);
        assert_eq!(records[0].cost, Some(15.0));
    }
}
