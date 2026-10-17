//! Templates and expressions in workflow nodes (minijinja).

use minijinja::{Environment, Error, ErrorKind};
use serde_json::Value;

fn env() -> Environment<'static> {
    let mut env = Environment::new();
    env.set_keep_trailing_newline(true);
    // Split text into its non-empty lines, trimmed, and without list bullets
    // or numbers: the usual way to fan an agent's list out with `map`.
    env.add_filter("lines", |s: String| -> Vec<String> {
        s.lines()
            .map(|l| {
                let l = l.trim();
                let l = l
                    .strip_prefix(['-', '*', '•'])
                    .map(str::trim_start)
                    .unwrap_or(l);
                let digits = l.chars().take_while(char::is_ascii_digit).count();
                let l = if digits > 0 && l[digits..].starts_with(['.', ')']) {
                    l[digits + 1..].trim_start()
                } else {
                    l
                };
                l.trim().to_owned()
            })
            .filter(|l| !l.is_empty())
            .collect()
    });
    env.add_filter("fromjson", |s: String| -> Result<minijinja::Value, Error> {
        let t = s.trim();
        let t = t
            .strip_prefix("```json")
            .or_else(|| t.strip_prefix("```"))
            .and_then(|r| r.strip_suffix("```"))
            .unwrap_or(t);
        serde_json::from_str::<Value>(t.trim())
            .map(minijinja::Value::from_serialize)
            .map_err(|e| Error::new(ErrorKind::InvalidOperation, format!("not JSON: {e}")))
    });
    env
}

/// Render a template to text.
pub fn render(template: &str, ctx: &Value) -> Result<String, String> {
    env()
        .render_str(template, ctx)
        .map_err(|e| format!("template: {e:#}"))
}

/// The expression inside `{{ … }}` when `s` is exactly one, else `None`.
fn sole_expression(s: &str) -> Option<&str> {
    let t = s.trim();
    let inner = t.strip_prefix("{{")?.strip_suffix("}}")?;
    (!inner.contains("{{") && !inner.contains("}}")).then(|| inner.trim())
}

/// Evaluate an expression (bare, or wrapped in `{{ }}`) to JSON.
pub fn eval(expr: &str, ctx: &Value) -> Result<Value, String> {
    let expr = sole_expression(expr).unwrap_or(expr.trim());
    let env = env();
    let compiled = env
        .compile_expression(expr)
        .map_err(|e| format!("expression {expr:?}: {e:#}"))?;
    let v = compiled
        .eval(ctx)
        .map_err(|e| format!("expression {expr:?}: {e:#}"))?;
    if v.is_undefined() {
        return Ok(Value::Null);
    }
    serde_json::to_value(&v).map_err(|e| format!("expression {expr:?}: {e}"))
}

/// Whether an expression is true (minijinja truthiness).
pub fn truthy(expr: &str, ctx: &Value) -> Result<bool, String> {
    let expr = sole_expression(expr).unwrap_or(expr.trim());
    let env = env();
    let compiled = env
        .compile_expression(expr)
        .map_err(|e| format!("condition {expr:?}: {e:#}"))?;
    compiled
        .eval(ctx)
        .map(|v| v.is_true())
        .map_err(|e| format!("condition {expr:?}: {e:#}"))
}

/// Fill templates in every string of a JSON value. A string that is just
/// one `{{ expression }}` becomes the expression's value, keeping its type.
pub fn fill(v: &Value, ctx: &Value) -> Result<Value, String> {
    Ok(match v {
        Value::String(s) if sole_expression(s).is_some() => eval(s, ctx)?,
        Value::String(s) if s.contains("{{") || s.contains("{%") => Value::String(render(s, ctx)?),
        Value::Array(a) => Value::Array(a.iter().map(|x| fill(x, ctx)).collect::<Result<_, _>>()?),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| Ok((k.clone(), fill(x, ctx)?)))
                .collect::<Result<_, String>>()?,
        ),
        other => other.clone(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn templates_expressions_and_filters() {
        let ctx = json!({
            "inputs": {"topic": "bees", "n": 3},
            "nodes": {"outline": {"output": "1. Intro\n- Hives\n\n* Honey", "status": "completed"},
                      "data": {"output": "```json\n{\"a\": [1, 2]}\n```"}}
        });
        assert_eq!(
            render("About {{ inputs.topic }}", &ctx).unwrap(),
            "About bees"
        );
        assert_eq!(
            eval("nodes.outline.output | lines", &ctx).unwrap(),
            json!(["Intro", "Hives", "Honey"])
        );
        assert_eq!(
            eval("{{ (nodes.data.output | fromjson).a }}", &ctx).unwrap(),
            json!([1, 2])
        );
        assert!(truthy("nodes.outline.status == 'completed'", &ctx).unwrap());
        assert!(!truthy("{{ inputs.n > 5 }}", &ctx).unwrap());
        assert_eq!(eval("nodes.missing", &ctx).unwrap(), Value::Null);
        assert_eq!(
            fill(
                &json!({"q": "{{ inputs.topic }} facts", "n": "{{ inputs.n }}", "k": 2}),
                &ctx
            )
            .unwrap(),
            json!({"q": "bees facts", "n": 3, "k": 2})
        );
        assert!(render("{{ nope(", &ctx).is_err());
    }
}
