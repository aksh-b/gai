//! Wrappers that add behaviour to any [`ChatModel`].
//!
//! Each one is itself a `ChatModel`, so they compose:
//! `Metered(Fallback([Retry(a), Retry(b)]))`.

mod fallback;
mod metered;
mod retry;

pub use fallback::Fallback;
pub use metered::{Metered, UsageRecord, UsageSink};
pub use retry::Retry;

use futures::StreamExt;

use crate::{ChatModel, ChatRequest, ChatStream, LlmError};

/// Open a stream and wait for its first item, so errors the provider sends
/// as the first event (overloaded, rate limited) surface as `Err` where
/// retry and fallback can act on them. Errors after output has started are
/// passed through: replaying would duplicate what the caller already saw.
pub(crate) async fn open(model: &dyn ChatModel, req: ChatRequest) -> Result<ChatStream, LlmError> {
    let mut stream = model.stream(req).await?;
    match stream.next().await {
        Some(Err(e)) => Err(e),
        Some(Ok(first)) => Ok(Box::pin(
            futures::stream::once(async { Ok(first) }).chain(stream),
        )),
        None => Ok(Box::pin(futures::stream::empty())),
    }
}
