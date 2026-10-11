//! Provider descriptors and API adapter contracts.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use alpha_kernel::{ActivateError, CapabilityScope, CapabilitySpec, Ctx, Module};
use alpha_module_http::HttpClient;
use alpha_module_loop::{ChatMessage, CompletionChunk, ProviderError, ToolDefinition};
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Stable module key used by the generated composition table.
pub const __ALPHA_MODULE__: &str = "provider.api";

/// One adapter dependency declared by a provider kind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterRequirement {
    /// Adapter registry key.
    pub key: String,
    /// Whether the provider cannot operate without this adapter.
    pub required: bool,
}

/// Provider authentication method metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthMethodDescriptor {
    /// Stable method key.
    pub key: String,
    /// User-facing label.
    pub label: String,
    /// Whether the method uses an interactive flow.
    pub interactive: bool,
}

/// One model advertised by a provider kind.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelDescriptor {
    /// Stable model id.
    pub id: String,
    /// User-facing model name.
    pub name: String,
    /// Default API adapter key.
    pub adapter: String,
    /// Maximum context window in tokens.
    pub context_window: u64,
    /// Maximum output tokens.
    pub max_tokens: u64,
    /// Whether reasoning controls are supported.
    pub reasoning: bool,
    /// Provider-native tools supported by the model.
    #[serde(default)]
    pub provider_tools: Vec<String>,
}

/// Provider kind metadata consumed by the front.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderDescriptor {
    /// Stable provider kind.
    pub kind: String,
    /// User-facing name.
    pub name: String,
    /// JSON Schema for provider-specific config.
    pub config_schema: Value,
    /// Authentication methods.
    pub auth_methods: Vec<AuthMethodDescriptor>,
    /// Model catalog.
    pub models: Vec<ModelDescriptor>,
    /// Required or optional adapter capabilities.
    pub adapters: Vec<AdapterRequirement>,
}

/// Opaque provider instance persisted by core configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderInstance {
    /// Stable instance id.
    pub id: String,
    /// Provider kind.
    pub kind: String,
    /// Provider config schema version.
    pub config_version: u32,
    /// Provider-specific config.
    pub config: Value,
    /// Keyring credential reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_ref: Option<String>,
    /// Per-model metadata overrides.
    #[serde(default)]
    pub model_overrides: Value,
}

/// One normalized request sent to an API adapter.
#[derive(Clone, Debug, PartialEq)]
pub struct AdapterRequest {
    /// Provider endpoint.
    pub base_url: String,
    /// Resolved API key, when the adapter uses bearer-style auth.
    pub api_key: Option<String>,
    /// Provider-level request headers.
    pub headers: BTreeMap<String, String>,
    /// Model id.
    pub model: String,
    /// Provider-facing history.
    pub messages: Vec<ChatMessage>,
    /// Tool definitions.
    pub tools: Vec<ToolDefinition>,
    /// Provider-native tools.
    pub provider_tools: Vec<String>,
    /// Adapter-specific options.
    pub options: Value,
}

/// One normalized API adapter.
pub trait ApiAdapter: Send + Sync + 'static {
    /// Stable registry key.
    fn key(&self) -> &'static str;
    /// Adapter capability metadata.
    fn capabilities(&self) -> AdapterCapabilities;
    /// Stream a completion.
    fn stream(
        &self,
        client: Arc<dyn HttpClient>,
        request: AdapterRequest,
    ) -> BoxStream<'static, Result<CompletionChunk, ProviderError>>;
}

/// Adapter feature metadata.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterCapabilities {
    /// Whether the adapter supports text streaming.
    pub streaming: bool,
    /// Whether the adapter supports tool calls.
    pub tool_calls: bool,
    /// Whether the adapter supports provider-native tools.
    pub provider_tools: bool,
}

/// Registry of named API adapters.
#[derive(Default)]
pub struct ApiAdapterRegistry {
    adapters: RwLock<BTreeMap<String, Arc<dyn ApiAdapter>>>,
}

impl ApiAdapterRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an adapter.
    pub fn register(&self, adapter: Arc<dyn ApiAdapter>) -> Result<(), AdapterRegistryError> {
        let key = adapter.key().to_owned();
        let mut adapters = self.adapters.write().expect("adapter lock poisoned");
        if adapters.contains_key(&key) {
            return Err(AdapterRegistryError::Duplicate(key));
        }
        adapters.insert(key, adapter);
        Ok(())
    }

    /// Remove an adapter by key.
    pub fn unregister(&self, key: &str) -> Option<Arc<dyn ApiAdapter>> {
        self.adapters
            .write()
            .expect("adapter lock poisoned")
            .remove(key)
    }

    /// Look up an adapter.
    pub fn get(&self, key: &str) -> Option<Arc<dyn ApiAdapter>> {
        self.adapters
            .read()
            .expect("adapter lock poisoned")
            .get(key)
            .cloned()
    }

    /// Registered adapter keys.
    pub fn keys(&self) -> Vec<String> {
        self.adapters
            .read()
            .expect("adapter lock poisoned")
            .keys()
            .cloned()
            .collect()
    }
}

/// Adapter registry failures.
#[derive(Debug, thiserror::Error)]
pub enum AdapterRegistryError {
    /// An adapter with the same key is already registered.
    #[error("api adapter `{0}` is already registered")]
    Duplicate(String),
}

/// Registers the API adapter registry.
pub struct ProviderApiModule;

impl Module for ProviderApiModule {
    type Requirements = ();
    type Config = ();

    const KEY: &'static str = "provider.api";
    const CAPABILITIES: &'static [CapabilitySpec] = &[CapabilitySpec::registry(
        "api.adapter",
        CapabilityScope::Global,
    )];

    fn activate(&self, ctx: &Ctx, _config: &Self::Config) -> Result<(), ActivateError> {
        ctx.provide::<ApiAdapterRegistry>(Arc::new(ApiAdapterRegistry::new()))
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        Ok(())
    }
}
