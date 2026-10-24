use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use gai_core::ToolSpec;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{Tool, ToolCtx, ToolError, ToolOutput, ToolPolicy};

type BoxFut = Pin<Box<dyn Future<Output = Result<ToolOutput, ToolError>> + Send>>;

/// A tool made from an async function over a typed argument struct.
pub struct FnTool<A> {
    name: String,
    description: String,
    schema: Value,
    policy: ToolPolicy,
    idempotent: bool,
    f: Arc<dyn Fn(ToolCtx, A) -> BoxFut + Send + Sync>,
    _args: PhantomData<fn(A)>,
}

/// Build a tool from an async function. The input schema comes from `A`'s
/// [`JsonSchema`] derive, and inputs are validated by deserializing into `A`.
///
/// ```
/// use gai_tools::{typed_tool, ToolOutput};
/// use schemars::JsonSchema;
/// use serde::Deserialize;
///
/// #[derive(Deserialize, JsonSchema)]
/// struct Greet {
///     /// Who to greet
///     name: String,
/// }
///
/// let tool = typed_tool("greet", "Greet someone by name", |_ctx, a: Greet| async move {
///     Ok(ToolOutput::text(format!("Hello, {}!", a.name)))
/// });
/// ```
pub fn typed_tool<A, F, Fut>(name: &str, description: &str, f: F) -> FnTool<A>
where
    A: DeserializeOwned + JsonSchema + Send + 'static,
    F: Fn(ToolCtx, A) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<ToolOutput, ToolError>> + Send + 'static,
{
    let mut schema =
        serde_json::to_value(schemars::schema_for!(A)).unwrap_or_else(|_| serde_json::json!({}));
    if let Value::Object(o) = &mut schema {
        // Providers want a bare object schema.
        o.remove("$schema");
        o.remove("title");
    }
    FnTool {
        name: name.to_owned(),
        description: description.to_owned(),
        schema,
        policy: ToolPolicy::Auto,
        idempotent: false,
        f: Arc::new(move |ctx, a| Box::pin(f(ctx, a))),
        _args: PhantomData,
    }
}

impl<A> FnTool<A> {
    pub fn policy(mut self, policy: ToolPolicy) -> Self {
        self.policy = policy;
        self
    }

    pub fn idempotent(mut self) -> Self {
        self.idempotent = true;
        self
    }
}

#[async_trait]
impl<A: DeserializeOwned + Send + 'static> Tool for FnTool<A> {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.clone(),
            description: self.description.clone(),
            input_schema: self.schema.clone(),
            strict: false,
        }
    }

    fn default_policy(&self) -> ToolPolicy {
        self.policy
    }

    fn idempotent(&self) -> bool {
        self.idempotent
    }

    async fn call(&self, ctx: &ToolCtx, input: Value) -> Result<ToolOutput, ToolError> {
        let args: A =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        (self.f)(ctx.clone(), args).await
    }
}

/// What a tool function may return: turned into the tool's output for the
/// model. Errors (the `Err` of a `Result`) are reported to the model as a
/// failed call.
pub trait IntoToolOutput {
    fn into_tool_output(self) -> Result<ToolOutput, ToolError>;
}

/// Return any serializable value as JSON from a tool function.
pub struct Json<T>(pub T);

impl IntoToolOutput for ToolOutput {
    fn into_tool_output(self) -> Result<ToolOutput, ToolError> {
        Ok(self)
    }
}

impl IntoToolOutput for String {
    fn into_tool_output(self) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::text(self))
    }
}

impl IntoToolOutput for &str {
    fn into_tool_output(self) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::text(self))
    }
}

impl IntoToolOutput for () {
    fn into_tool_output(self) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::text("done"))
    }
}

impl IntoToolOutput for Value {
    fn into_tool_output(self) -> Result<ToolOutput, ToolError> {
        Ok(match self {
            Value::String(s) => ToolOutput::text(s),
            v => ToolOutput::json(&v),
        })
    }
}

impl<T: serde::Serialize> IntoToolOutput for Json<T> {
    fn into_tool_output(self) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::json(&self.0))
    }
}

macro_rules! display_output {
    ($($t:ty),*) => {$(
        impl IntoToolOutput for $t {
            fn into_tool_output(self) -> Result<ToolOutput, ToolError> {
                Ok(ToolOutput::text(self.to_string()))
            }
        }
    )*};
}
display_output!(
    bool, char, i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize, f32, f64
);

impl<T: IntoToolOutput, E: std::fmt::Display> IntoToolOutput for Result<T, E> {
    fn into_tool_output(self) -> Result<ToolOutput, ToolError> {
        match self {
            Ok(v) => v.into_tool_output(),
            Err(e) => Err(ToolError::Failed(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Deserialize, JsonSchema)]
    struct Add {
        /// First number
        a: f64,
        b: f64,
    }

    #[tokio::test]
    async fn schema_and_validation_come_from_the_type() {
        let t = typed_tool("add", "Add numbers", |_c, x: Add| async move {
            Ok(ToolOutput::text((x.a + x.b).to_string()))
        });
        let spec = t.spec();
        assert_eq!(spec.input_schema["type"], "object");
        assert_eq!(
            spec.input_schema["properties"]["a"]["description"],
            "First number"
        );
        assert!(spec.input_schema.get("$schema").is_none());
        let ctx = ToolCtx::for_workspace(".");
        assert_eq!(
            t.call(&ctx, json!({"a": 1, "b": 2}))
                .await
                .unwrap()
                .as_text(),
            "3"
        );
        assert!(matches!(
            t.call(&ctx, json!({"a": "x"})).await,
            Err(ToolError::InvalidInput(_))
        ));
    }
}
