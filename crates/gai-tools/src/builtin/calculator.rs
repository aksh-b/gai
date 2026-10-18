use schemars::JsonSchema;
use serde::Deserialize;

use crate::{FnTool, ToolError, ToolOutput, typed_tool};

#[derive(Deserialize, JsonSchema)]
pub struct CalcArgs {
    /// An arithmetic expression, e.g. `(3 + 4.5) * 2 ^ 3`, `17 / 100 * 2340`,
    /// `17% of 2340` or `math::sqrt(2)`.
    expression: String,
}

pub fn calculator() -> FnTool<CalcArgs> {
    typed_tool(
        "calculator",
        "Evaluate an arithmetic expression. Numbers are decimals, so 7 / 2 is 3.5. Supports + - * / ^, % (remainder), `X% of Y`, parentheses, comparisons, and functions such as math::sqrt, math::ln, math::sin, floor, round, min, max.",
        |_ctx, a: CalcArgs| async move {
            evalexpr::eval(&prepare(&a.expression))
                .map(|v| ToolOutput::text(show(v)))
                .map_err(|e| ToolError::InvalidInput(format!("{}: {e}", a.expression)))
        },
    )
    .idempotent()
}

/// Rewrite an expression so it means what people mean: `X% of Y` becomes
/// `(X / 100 * Y)`, and whole numbers become decimals, so `17 / 100` is
/// 0.17 rather than integer division's 0.
fn prepare(expr: &str) -> String {
    let expr = percent_of(expr);
    let chars: Vec<char> = expr.chars().collect();
    let mut out = String::with_capacity(expr.len() + 8);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let starts_number = c.is_ascii_digit()
            && !(i > 0
                && (chars[i - 1].is_alphanumeric() || matches!(chars[i - 1], '_' | '.' | ':')));
        if !starts_number {
            out.push(c);
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        out.extend(&chars[start..i]);
        // Already a decimal, an exponent, or part of a name: leave it.
        let next = chars.get(i).copied();
        if !matches!(next, Some(n) if n == '.' || n.is_alphanumeric() || n == '_') {
            out.push_str(".0");
        }
    }
    out
}

/// `17% of 2340` → `(17 / 100 * 2340)`.
fn percent_of(expr: &str) -> String {
    let re = regex::Regex::new(r"(\d+(?:\.\d+)?)\s*%\s*of\s+").expect("valid regex");
    re.replace_all(expr, "$1 / 100 * ").into_owned()
}

/// Whole results without a trailing `.0`, others rounded to what f64 can
/// say honestly.
fn show(v: evalexpr::Value) -> String {
    match v {
        evalexpr::Value::Float(f) if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e15 => {
            format!("{}", f as i64)
        }
        evalexpr::Value::Float(f) => {
            let s = format!("{:.12}", f);
            let s = s.trim_end_matches('0').trim_end_matches('.');
            s.to_owned()
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Tool, ToolCtx};
    use serde_json::json;

    async fn calc(e: &str) -> Result<String, String> {
        calculator()
            .call(&ToolCtx::for_workspace("."), json!({"expression": e}))
            .await
            .map(|o| o.as_text())
            .map_err(|e| e.to_string())
    }

    #[tokio::test]
    async fn evaluates() {
        assert_eq!(calc("(3 + 4.5) * 2").await.unwrap(), "15");
        assert!(calc("1 +").await.is_err());
    }

    #[tokio::test]
    async fn divides_like_people_expect() {
        // Integer division used to make this 0.
        assert_eq!(calc("17 / 100 * 2340").await.unwrap(), "397.8");
        assert_eq!(calc("17% of 2340").await.unwrap(), "397.8");
        assert_eq!(calc("7 / 2").await.unwrap(), "3.5");
        assert_eq!(calc("6 * 7").await.unwrap(), "42");
        assert_eq!(calc("1 / 3").await.unwrap(), "0.333333333333");
        assert_eq!(calc("10 % 3").await.unwrap(), "1");
        assert_eq!(calc("2 ^ 10").await.unwrap(), "1024");
        assert_eq!(calc("math::sqrt(16)").await.unwrap(), "4");
        assert_eq!(calc("max(3, 9) > 5").await.unwrap(), "true");
        assert_eq!(calc("1.5e3 + 1").await.unwrap(), "1501");
    }
}
