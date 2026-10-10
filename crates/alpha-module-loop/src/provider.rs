//! Model provider contract and registry.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use futures::future::BoxFuture;
use serde_json::Value;
use thiserror::Error;

/// One chat message in provider-facing history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatMessage {
    /// User message.
    User(String),
    /// Assistant message.
    Assistant(String),
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
            Self::User(_) => "user",
            Self::Assistant(_) => "assistant",
            Self::ToolResult { .. } => "tool",
        }
    }

    /// Message text.
    pub fn text(&self) -> &str {
        match self {
            Self::User(text) | Self::Assistant(text) => text,
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
}

/// One completion response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionResponse {
    /// Assistant text.
    pub text: String,
    /// Tool calls requested by the model.
    pub tool_calls: Vec<ToolCall>,
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
    /// Produce a completion for the request.
    fn complete(
        &self,
        request: CompletionRequest,
    ) -> BoxFuture<'_, Result<CompletionResponse, ProviderError>>;
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
