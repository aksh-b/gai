use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Who authored a message. System prompts are carried separately on the
/// request, so a conversation only alternates between these two.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::text(text)],
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentBlock::text(text)],
        }
    }

    /// All text blocks joined, ignoring reasoning and tool blocks.
    pub fn text(&self) -> String {
        join_text(&self.content)
    }

    pub fn tool_uses(&self) -> impl Iterator<Item = (&str, &str, &Value)> {
        self.content.iter().filter_map(|b| match b {
            ContentBlock::ToolUse { id, name, input } => Some((id.as_str(), name.as_str(), input)),
            _ => None,
        })
    }
}

/// One piece of message content.
///
/// Reasoning blocks remember which model produced them (`origin`), because
/// providers only accept their own reasoning back and require it unchanged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        source: MediaSource,
    },
    /// Speech or other sound, for models that take audio input (and as
    /// output of tools like `speak`).
    Audio {
        source: MediaSource,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: Vec<ContentBlock>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
    Thinking {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<String>,
    },
    RedactedThinking {
        data: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<String>,
    },
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    pub fn tool_result(
        tool_use_id: impl Into<String>,
        text: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self::ToolResult {
            tool_use_id: tool_use_id.into(),
            content: vec![Self::text(text)],
            is_error,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            _ => None,
        }
    }
}

/// Where image or audio bytes come from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MediaSource {
    Base64 {
        media_type: String,
        data: String,
    },
    Url {
        url: String,
    },
    /// Bytes kept in gai's blob store. The runtime swaps these for
    /// `Base64` before a request reaches a provider.
    Blob {
        id: String,
        media_type: String,
    },
}

impl MediaSource {
    /// A URL any provider accepts: the URL itself, or a `data:` URL. Blob
    /// references become `gai-blob:<id>`, which no provider accepts: they
    /// must be resolved first.
    pub fn to_url(&self) -> String {
        match self {
            Self::Base64 { media_type, data } => format!("data:{media_type};base64,{data}"),
            Self::Url { url } => url.clone(),
            Self::Blob { id, .. } => format!("gai-blob:{id}"),
        }
    }

    /// The media type, when the source says it.
    pub fn media_type(&self) -> Option<&str> {
        match self {
            Self::Base64 { media_type, .. } | Self::Blob { media_type, .. } => Some(media_type),
            Self::Url { .. } => None,
        }
    }
}

pub(crate) fn join_text(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(ContentBlock::as_text)
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn content_blocks_round_trip_with_type_tags() {
        let msg = Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Thinking {
                    text: "hm".into(),
                    signature: Some("sig".into()),
                    origin: None,
                },
                ContentBlock::text("hi"),
                ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "calc".into(),
                    input: json!({"x": 1}),
                },
            ],
        };
        let v = serde_json::to_value(&msg).unwrap();
        assert_eq!(v["role"], "assistant");
        assert_eq!(v["content"][0]["type"], "thinking");
        assert_eq!(v["content"][2]["type"], "tool_use");
        let back: Message = serde_json::from_value(v).unwrap();
        assert_eq!(back, msg);
        assert_eq!(back.text(), "hi");
        assert_eq!(back.tool_uses().count(), 1);
    }
}
