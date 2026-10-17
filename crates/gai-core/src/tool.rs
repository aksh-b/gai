use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A tool as the model sees it: a name, what it does, and a JSON Schema for
/// its input.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    /// Ask the provider to guarantee schema-valid arguments where supported.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub strict: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoice {
    #[default]
    Auto,
    None,
    /// The model must call some tool. Several newer models reject this.
    Required,
    Tool {
        name: String,
    },
}
