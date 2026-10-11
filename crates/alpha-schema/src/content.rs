//! Structured assistant content blocks.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One block in an assistant message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantContentBlock {
    /// Visible assistant text.
    Text {
        /// Text content.
        text: String,
    },
    /// Provider reasoning or thinking content.
    Thinking {
        /// Thinking text.
        text: String,
        /// Provider signature when supplied.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// A tool call executed by the local tool registry.
    LocalToolCall {
        /// Provider call identity.
        id: String,
        /// Tool name.
        name: String,
        /// Decoded tool input.
        arguments: Value,
    },
    /// A tool call executed by the provider.
    ProviderToolCall {
        /// Provider call identity.
        id: String,
        /// Provider tool name.
        name: String,
        /// Provider tool input.
        input: Value,
    },
    /// A provider tool result returned in the assistant stream.
    ProviderToolResult {
        /// Provider call identity.
        id: String,
        /// Provider tool name.
        name: String,
        /// Provider result content.
        content: Value,
    },
    /// A citation attached to assistant output.
    Citation {
        /// Source URL.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        /// Source title.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// Source snippet.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        snippet: Option<String>,
    },
}

impl AssistantContentBlock {
    /// Visible text carried by this block.
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text { text } | Self::Thinking { text, .. } => Some(text),
            _ => None,
        }
    }
}
