use schemars::JsonSchema;
use serde::Deserialize;

use crate::{FnTool, ToolError, ToolOutput, typed_tool};

#[derive(Deserialize, JsonSchema)]
pub struct CalcArgs {
    /// An arithmetic expression, e.g. `(3 + 4.5) * 2 ^ 3` or `math::sqrt(2)`.
    expression: String,
}

pub fn calculator() -> FnTool<CalcArgs> {
    typed_tool(
        "calculator",
        "Evaluate an arithmetic expression exactly. Supports + - * / % ^, parentheses, comparisons, and functions such as math::sqrt, math::ln, math::sin, floor, round, min, max.",
        |_ctx, a: CalcArgs| async move {
            evalexpr::eval(&a.expression)
                .map(|v| ToolOutput::text(v.to_string()))
                .map_err(|e| ToolError::InvalidInput(format!("{}: {e}", a.expression)))
        },
    )
    .idempotent()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Tool, ToolCtx};
    use serde_json::json;

    #[tokio::test]
    async fn evaluates() {
        let ctx = ToolCtx::for_workspace(".");
        let out = calculator()
            .call(&ctx, json!({"expression": "(3 + 4.5) * 2"}))
            .await
            .unwrap();
        assert_eq!(out.as_text(), "15");
        assert!(
            calculator()
                .call(&ctx, json!({"expression": "1 +"}))
                .await
                .is_err()
        );
    }
}
