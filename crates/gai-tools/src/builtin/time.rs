use schemars::JsonSchema;
use serde::Deserialize;

use crate::{FnTool, ToolOutput, typed_tool};

#[derive(Deserialize, JsonSchema)]
pub struct NoArgs {}

pub fn current_time() -> FnTool<NoArgs> {
    typed_tool(
        "current_time",
        "The current date and time (UTC, RFC 3339) and weekday.",
        |_ctx, _a: NoArgs| async move {
            let now = chrono::Utc::now();
            Ok(ToolOutput::text(format!(
                "{} ({})",
                now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                now.format("%A")
            )))
        },
    )
    .idempotent()
}
