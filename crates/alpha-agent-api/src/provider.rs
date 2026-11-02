//! Model provider contract and registry.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock};

use alpha_schema::{AssistantContentBlock, ImageAttachment, ReasoningLevel};
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use serde_json::Value;
use thiserror::Error;

/// One chat message in provider-facing history.
#[derive(Clone, Debug, PartialEq)]
pub enum ChatMessage {
    /// System instructions.
    System(String),
    /// User message.
    User(String),
    /// User message with text and images.
    UserContent {
        /// Message text, possibly empty.
        text: String,
        /// Attached images.
        images: Vec<ImageAttachment>,
    },
    /// Assistant message.
    Assistant(String),
    /// Assistant message that also requested tool calls.
    AssistantToolCalls {
        /// Assistant text, possibly empty.
        text: String,
        /// Requested calls.
        calls: Vec<ToolCall>,
    },
    /// Assistant message with ordered structured content.
    AssistantContent(Vec<AssistantContentBlock>),
    /// Tool result returned to the model.
    ToolResult {
        /// Provider call identity.
        call_id: String,
        /// Result content.
        content: String,
        /// Images returned with the tool result.
        images: Vec<ImageAttachment>,
    },
}

impl ChatMessage {
    /// Provider-facing role name.
    pub fn role(&self) -> &'static str {
        match self {
            Self::System(_) => "system",
            Self::User(_) | Self::UserContent { .. } => "user",
            Self::Assistant(_) | Self::AssistantToolCalls { .. } | Self::AssistantContent(_) => {
                "assistant"
            }
            Self::ToolResult { .. } => "tool",
        }
    }

    /// Message text.
    pub fn text(&self) -> &str {
        match self {
            Self::System(text) | Self::User(text) | Self::Assistant(text) => text,
            Self::UserContent { text, .. } => text,
            Self::AssistantToolCalls { text, .. } => text,
            Self::AssistantContent(blocks) => visible_block_text(blocks),
            Self::ToolResult { content, .. } => content,
        }
    }
}

fn visible_block_text(blocks: &[AssistantContentBlock]) -> &str {
    blocks
        .iter()
        .find_map(|block| match block {
            AssistantContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .unwrap_or_default()
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
#[derive(Clone, Debug, PartialEq)]
pub struct CompletionRequest {
    /// Session identity used for provider routing.
    pub session_id: String,
    /// Model identifier.
    pub model: String,
    /// Requested reasoning effort.
    pub reasoning: ReasoningLevel,
    /// Conversation history.
    pub messages: Vec<ChatMessage>,
    /// Tools advertised for this request.
    pub tools: Vec<ToolDefinition>,
    /// Provider-native tools for this request.
    pub provider_tools: Vec<String>,
    /// Catalog maximum output budget for providers that require one.
    ///
    /// A `None` value leaves the provider/model default untouched.
    pub max_output_tokens: Option<u64>,
}

/// Effective provider route for a session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelRoute {
    /// Provider instance id or provider kind.
    pub provider: String,
    /// Model identifier. Empty means the provider default.
    pub model: String,
    /// Requested reasoning effort.
    pub reasoning: ReasoningLevel,
}

impl ModelRoute {
    /// Create a route.
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
        reasoning: ReasoningLevel,
    ) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            reasoning,
        }
    }
}

/// Mutable route shared by the session actor, loop, and dynamic provider.
#[derive(Debug, Default)]
pub struct ModelRouteOverride {
    route: RwLock<ModelRoute>,
}

impl ModelRouteOverride {
    /// Create an override with an initial route.
    pub fn new(route: ModelRoute) -> Self {
        Self {
            route: RwLock::new(route),
        }
    }

    /// Read the current route.
    pub fn get(&self) -> ModelRoute {
        self.route
            .read()
            .expect("model route lock poisoned")
            .clone()
    }

    /// Replace the current route.
    pub fn set(&self, route: ModelRoute) {
        *self.route.write().expect("model route lock poisoned") = route;
    }
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
    /// Provider-reported token usage for the request.
    Usage {
        /// Prompt tokens.
        input_tokens: u64,
        /// Completion tokens.
        output_tokens: u64,
        /// Prompt tokens the provider served from its cache.
        cached_input_tokens: u64,
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

    /// Context window reported for one model, when known.
    fn context_window(&self, _model: &str) -> Option<usize> {
        None
    }

    /// Maximum output tokens declared for one model, when the catalog knows it.
    fn output_limit(&self, _model: &str) -> Option<u64> {
        None
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
                    if response.blocks.is_empty() {
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
                    } else {
                        for (index, block) in response.blocks.into_iter().enumerate() {
                            match block {
                                AssistantContentBlock::Text { text } => {
                                    if !text.is_empty() {
                                        chunks.push(Ok(CompletionChunk::Text(text)));
                                    }
                                }
                                AssistantContentBlock::Thinking { text, signature } => {
                                    chunks.push(Ok(CompletionChunk::Thinking { text, signature }));
                                }
                                AssistantContentBlock::LocalToolCall {
                                    id,
                                    name,
                                    arguments,
                                } => {
                                    chunks.push(Ok(CompletionChunk::ToolCall(ToolCallDelta {
                                        index,
                                        id: Some(id),
                                        name: Some(name),
                                        arguments: serde_json::to_string(&arguments)
                                            .unwrap_or_else(|_| "null".to_owned()),
                                    })));
                                }
                                AssistantContentBlock::ProviderToolCall { id, name, input } => {
                                    chunks.push(Ok(CompletionChunk::ProviderToolCall {
                                        id,
                                        name,
                                        input,
                                    }));
                                }
                                AssistantContentBlock::ProviderToolResult { id, name, content } => {
                                    chunks.push(Ok(CompletionChunk::ProviderToolResult {
                                        id,
                                        name,
                                        content,
                                    }));
                                }
                                AssistantContentBlock::Citation {
                                    url,
                                    title,
                                    snippet,
                                } => {
                                    chunks.push(Ok(CompletionChunk::Citation {
                                        url,
                                        title,
                                        snippet,
                                    }));
                                }
                            }
                        }
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
