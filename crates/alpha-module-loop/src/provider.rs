//! Model provider contract and registry.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock};

use alpha_schema::AssistantContentBlock;
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use serde_json::Value;
use thiserror::Error;

/// One chat message in provider-facing history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatMessage {
    /// System instructions.
    System(String),
    /// User message.
    User(String),
    /// Assistant message.
    Assistant(String),
    /// Assistant message that also requested tool calls.
    AssistantToolCalls {
        /// Assistant text, possibly empty.
        text: String,
        /// Requested calls.
        calls: Vec<ToolCall>,
    },
    /// Tool result returned to the model.
    ToolResult {
        /// Provider call identity.
        call_id: String,
        /// Result content.
        content: String,
    },
}

impl ChatMessage {
    /// Provider-facing role name.
    pub fn role(&self) -> &'static str {
        match self {
            Self::System(_) => "system",
            Self::User(_) => "user",
            Self::Assistant(_) | Self::AssistantToolCalls { .. } => "assistant",
            Self::ToolResult { .. } => "tool",
        }
    }

    /// Message text.
    pub fn text(&self) -> &str {
        match self {
            Self::System(text) | Self::User(text) | Self::Assistant(text) => text,
            Self::AssistantToolCalls { text, .. } => text,
            Self::ToolResult { content, .. } => content,
        }
    }
}

/// One tool definition advertised to the model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolDefinition {
    /// Tool name.
    pub name: String,
    /// Model-facing description.
    pub description: String,
    /// JSON schema for the tool input.
    pub input_schema: Value,
}

/// Provider capabilities that affect one model request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProviderCapabilities {
    /// Provider-native tools enabled for the request.
    pub provider_tools: Vec<String>,
}

/// One tool call requested by the model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCall {
    /// Provider call identity.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Decoded arguments.
    pub arguments: Value,
}

/// One completion request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionRequest {
    /// Model identifier.
    pub model: String,
    /// Conversation history.
    pub messages: Vec<ChatMessage>,
    /// Tools advertised for this request.
    pub tools: Vec<ToolDefinition>,
    /// Provider-native tools for this request.
    pub provider_tools: Vec<String>,
}

/// Session-level provider-native tool override.
#[derive(Default)]
pub struct ProviderToolsOverride {
    provider_tools: Mutex<Option<Vec<String>>>,
}

impl ProviderToolsOverride {
    /// Read the current override.
    pub fn get(&self) -> Option<Vec<String>> {
        self.provider_tools
            .lock()
            .expect("provider tools override lock poisoned")
            .clone()
    }

    /// Replace the current override.
    pub fn set(&self, provider_tools: Option<Vec<String>>) {
        *self
            .provider_tools
            .lock()
            .expect("provider tools override lock poisoned") = provider_tools;
    }
}

/// One completion response.
#[derive(Clone, Debug, PartialEq)]
pub struct CompletionResponse {
    /// Assistant text.
    pub text: String,
    /// Tool calls requested by the model.
    pub tool_calls: Vec<ToolCall>,
    /// Structured assistant content.
    pub blocks: Vec<AssistantContentBlock>,
}

/// One incremental completion chunk.
#[derive(Clone, Debug, PartialEq)]
pub enum CompletionChunk {
    /// Assistant text delta.
    Text(String),
    /// Incremental tool-call data.
    ToolCall(ToolCallDelta),
    /// Thinking content.
    Thinking {
        /// Thinking text.
        text: String,
        /// Provider signature.
        signature: Option<String>,
    },
    /// Provider-native tool call.
    ProviderToolCall {
        /// Provider call identity.
        id: String,
        /// Tool name.
        name: String,
        /// Tool input.
        input: Value,
    },
    /// Provider-native tool result.
    ProviderToolResult {
        /// Provider call identity.
        id: String,
        /// Tool name.
        name: String,
        /// Result content.
        content: Value,
    },
    /// Citation attached to assistant output.
    Citation {
        /// Source URL.
        url: Option<String>,
        /// Source title.
        title: Option<String>,
        /// Source snippet.
        snippet: Option<String>,
    },
}

/// One tool-call delta.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallDelta {
    /// Stable index within the streamed response.
    pub index: usize,
    /// Provider call identity when supplied.
    pub id: Option<String>,
    /// Tool name when supplied.
    pub name: Option<String>,
    /// JSON argument fragment.
    pub arguments: String,
}

/// Provider failure.
#[derive(Debug, Error)]
pub enum ProviderError {
    /// The provider request failed.
    #[error("provider request failed: {0}")]
    Request(String),
}

/// Model provider contract.
pub trait ModelProvider: Send + Sync + 'static {
    /// Capabilities applied to each request.
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::default()
    }

    /// Produce a completion for the request.
    fn complete(
        &self,
        request: CompletionRequest,
    ) -> BoxFuture<'_, Result<CompletionResponse, ProviderError>>;

    /// Stream a completion.
    ///
    /// The default implementation calls [`ModelProvider::complete`] and emits
    /// the complete response as one or more chunks.
    fn stream(
        self: Arc<Self>,
        request: CompletionRequest,
    ) -> BoxStream<'static, Result<CompletionChunk, ProviderError>> {
        let completion = async move { self.complete(request).await };
        Box::pin(
            futures::stream::once(completion).flat_map(|result| match result {
                Ok(response) => {
                    let mut chunks = Vec::new();
                    if !response.text.is_empty() {
                        chunks.push(Ok(CompletionChunk::Text(response.text)));
                    }
                    for (index, call) in response.tool_calls.into_iter().enumerate() {
                        chunks.push(Ok(CompletionChunk::ToolCall(ToolCallDelta {
                            index,
                            id: Some(call.id),
                            name: Some(call.name),
                            arguments: serde_json::to_string(&call.arguments)
                                .unwrap_or_else(|_| "null".to_owned()),
                        })));
                    }
                    futures::stream::iter(chunks)
                }
                Err(error) => futures::stream::iter(vec![Err(error)]),
            }),
        )
    }
}

/// Registry of named model providers.
#[derive(Default)]
pub struct ProviderRegistry {
    providers: RwLock<BTreeMap<String, Arc<dyn ModelProvider>>>,
}

impl ProviderRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a provider. Duplicate names are rejected.
    pub fn register(
        &self,
        name: impl Into<String>,
        provider: Arc<dyn ModelProvider>,
    ) -> Result<(), RegistryError> {
        let name = name.into();
        let mut providers = self.providers.write().expect("provider lock poisoned");
        if providers.contains_key(&name) {
            return Err(RegistryError::Duplicate(name));
        }
        providers.insert(name, provider);
        Ok(())
    }

    /// Remove a provider by name.
    pub fn unregister(&self, name: &str) -> Option<Arc<dyn ModelProvider>> {
        self.providers
            .write()
            .expect("provider lock poisoned")
            .remove(name)
    }

    /// Look up a provider by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn ModelProvider>> {
        self.providers
            .read()
            .expect("provider lock poisoned")
            .get(name)
            .cloned()
    }

    /// Registered provider names.
    pub fn names(&self) -> Vec<String> {
        self.providers
            .read()
            .expect("provider lock poisoned")
            .keys()
            .cloned()
            .collect()
    }
}

/// Registry errors.
#[derive(Debug, Error)]
pub enum RegistryError {
    /// A provider with this name already exists.
    #[error("provider `{0}` is already registered")]
    Duplicate(String),
}
