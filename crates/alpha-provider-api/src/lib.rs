//! Provider descriptors and API adapter contracts.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use alpha_kernel::{ActivateError, CapabilityScope, CapabilitySpec, Ctx, Module};
use alpha_module_http::HttpClient;
use alpha_module_loop::{
    ChatMessage, CompletionChunk, CompletionRequest, CompletionResponse, ModelProvider,
    ProviderError, ToolCall, ToolDefinition,
};
use futures::future::BoxFuture;
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

/// Runtime services available while constructing a provider instance.
pub struct ProviderBuildContext {
    /// Registered API adapters.
    pub adapters: Arc<ApiAdapterRegistry>,
    /// HTTP seam used by adapters.
    pub http: Arc<dyn HttpClient>,
    /// Resolved credential bytes, when the selected auth method needs them.
    pub credential: Option<Vec<u8>>,
}

/// Provider kind construction failures.
#[derive(Debug, thiserror::Error)]
pub enum ProviderKindError {
    /// Provider config is invalid.
    #[error("invalid provider config: {0}")]
    InvalidConfig(String),
    /// Config migration failed.
    #[error("provider config migration failed: {0}")]
    Migration(String),
    /// A required adapter is not registered.
    #[error("required api adapter `{0}` is not registered")]
    AdapterMissing(String),
    /// A required credential is missing.
    #[error("credential is required for provider instance `{0}`")]
    CredentialMissing(String),
    /// Provider construction failed.
    #[error("provider build failed: {0}")]
    Build(String),
}

/// Resolve a UTF-8 credential required by a provider instance.
pub fn required_credential_text(
    instance: &ProviderInstance,
    context: &ProviderBuildContext,
) -> Result<String, ProviderKindError> {
    let bytes = context
        .credential
        .as_ref()
        .ok_or_else(|| ProviderKindError::CredentialMissing(instance.id.clone()))?;
    String::from_utf8(bytes.clone())
        .map_err(|error| ProviderKindError::Build(format!("credential is not UTF-8: {error}")))
}

/// Resolve a required API adapter.
pub fn required_adapter(
    key: &str,
    context: &ProviderBuildContext,
) -> Result<Arc<dyn ApiAdapter>, ProviderKindError> {
    context
        .adapters
        .get(key)
        .ok_or_else(|| ProviderKindError::AdapterMissing(key.to_owned()))
}

/// One provider kind.
pub trait ProviderKind: Send + Sync + 'static {
    /// Provider metadata.
    fn descriptor(&self) -> ProviderDescriptor;
    /// Current config schema version.
    fn config_version(&self) -> u32;
    /// Migrate old config to the current version.
    fn migrate_config(&self, from: u32, config: Value) -> Result<Value, ProviderKindError> {
        if from == self.config_version() {
            Ok(config)
        } else {
            Err(ProviderKindError::Migration(format!(
                "no migration from version {from} to {}",
                self.config_version()
            )))
        }
    }
    /// Build a model provider for one instance.
    fn build(
        &self,
        instance: &ProviderInstance,
        context: &ProviderBuildContext,
    ) -> Result<Arc<dyn ModelProvider>, ProviderKindError>;
}

/// Registry of provider kinds.
#[derive(Default)]
pub struct ProviderKindRegistry {
    kinds: RwLock<BTreeMap<String, Arc<dyn ProviderKind>>>,
}

impl ProviderKindRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a provider kind.
    pub fn register(&self, kind: Arc<dyn ProviderKind>) -> Result<(), ProviderKindRegistryError> {
        let key = kind.descriptor().kind;
        let mut kinds = self.kinds.write().expect("provider kind lock poisoned");
        if kinds.contains_key(&key) {
            return Err(ProviderKindRegistryError::Duplicate(key));
        }
        kinds.insert(key, kind);
        Ok(())
    }

    /// Remove a provider kind.
    pub fn unregister(&self, key: &str) -> Option<Arc<dyn ProviderKind>> {
        self.kinds
            .write()
            .expect("provider kind lock poisoned")
            .remove(key)
    }

    /// Look up a provider kind.
    pub fn get(&self, key: &str) -> Option<Arc<dyn ProviderKind>> {
        self.kinds
            .read()
            .expect("provider kind lock poisoned")
            .get(key)
            .cloned()
    }

    /// Descriptors for all registered kinds.
    pub fn descriptors(&self) -> Vec<ProviderDescriptor> {
        self.kinds
            .read()
            .expect("provider kind lock poisoned")
            .values()
            .map(|kind| kind.descriptor())
            .collect()
    }
}

/// Provider kind registry failures.
#[derive(Debug, thiserror::Error)]
pub enum ProviderKindRegistryError {
    /// A provider kind with the same key is already registered.
    #[error("provider kind `{0}` is already registered")]
    Duplicate(String),
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

/// Provider implementation over one normalized API adapter.
pub struct AdapterProvider {
    adapter: Arc<dyn ApiAdapter>,
    model_adapters: BTreeMap<String, Arc<dyn ApiAdapter>>,
    client: Arc<dyn HttpClient>,
    base_url: String,
    api_key: Option<String>,
    headers: BTreeMap<String, String>,
    provider_tools: Vec<String>,
    options: Value,
}

impl AdapterProvider {
    /// Create a provider over an adapter.
    pub fn new(
        adapter: Arc<dyn ApiAdapter>,
        client: Arc<dyn HttpClient>,
        base_url: impl Into<String>,
        api_key: Option<String>,
        headers: BTreeMap<String, String>,
        provider_tools: Vec<String>,
        options: Value,
    ) -> Self {
        Self {
            adapter,
            model_adapters: BTreeMap::new(),
            client,
            base_url: base_url.into(),
            api_key,
            headers,
            provider_tools,
            options,
        }
    }

    /// Add model-specific adapter overrides.
    pub fn with_model_adapters(mut self, adapters: BTreeMap<String, Arc<dyn ApiAdapter>>) -> Self {
        self.model_adapters = adapters;
        self
    }

    fn adapter_for(&self, model: &str) -> Arc<dyn ApiAdapter> {
        self.model_adapters
            .get(model)
            .cloned()
            .unwrap_or_else(|| self.adapter.clone())
    }

    fn adapter_request(&self, request: CompletionRequest) -> AdapterRequest {
        AdapterRequest {
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            headers: self.headers.clone(),
            model: request.model,
            messages: request.messages,
            tools: request.tools,
            provider_tools: self.provider_tools.clone(),
            options: self.options.clone(),
        }
    }
}

impl ModelProvider for AdapterProvider {
    fn complete(
        &self,
        request: CompletionRequest,
    ) -> BoxFuture<'_, Result<CompletionResponse, ProviderError>> {
        Box::pin(async move {
            let adapter_request = self.adapter_request(request);
            let adapter = self.adapter_for(&adapter_request.model);
            let mut stream = adapter.stream(self.client.clone(), adapter_request);
            collect_stream(&mut stream).await
        })
    }

    fn stream(
        &self,
        request: CompletionRequest,
    ) -> BoxStream<'_, Result<CompletionChunk, ProviderError>> {
        let adapter_request = self.adapter_request(request);
        let adapter = self.adapter_for(&adapter_request.model);
        adapter.stream(self.client.clone(), adapter_request)
    }
}

async fn collect_stream(
    stream: &mut BoxStream<'_, Result<CompletionChunk, ProviderError>>,
) -> Result<CompletionResponse, ProviderError> {
    use futures::StreamExt;

    let mut text = String::new();
    let mut calls: BTreeMap<usize, ToolCallBuilder> = BTreeMap::new();
    while let Some(chunk) = stream.next().await {
        match chunk? {
            CompletionChunk::Text(delta) => text.push_str(&delta),
            CompletionChunk::ToolCall(delta) => {
                let call = calls.entry(delta.index).or_default();
                if let Some(id) = delta.id {
                    call.id = Some(id);
                }
                if let Some(name) = delta.name {
                    call.name = Some(name);
                }
                call.arguments.push_str(&delta.arguments);
            }
        }
    }
    let mut tool_calls = Vec::with_capacity(calls.len());
    for (index, call) in calls {
        let id = call.id.unwrap_or_else(|| format!("call_{index}"));
        let name = call
            .name
            .ok_or_else(|| ProviderError::Request(format!("tool call {index} has no name")))?;
        let arguments = if call.arguments.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&call.arguments).map_err(|error| {
                ProviderError::Request(format!("invalid tool arguments for `{name}`: {error}"))
            })?
        };
        tool_calls.push(ToolCall {
            id,
            name,
            arguments,
        });
    }
    Ok(CompletionResponse { text, tool_calls })
}

#[derive(Default)]
struct ToolCallBuilder {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
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
    const CAPABILITIES: &'static [CapabilitySpec] = &[
        CapabilitySpec::registry("api.adapter", CapabilityScope::Global),
        CapabilitySpec::registry("provider.kind", CapabilityScope::Global),
    ];

    fn activate(&self, ctx: &Ctx, _config: &Self::Config) -> Result<(), ActivateError> {
        ctx.provide::<ApiAdapterRegistry>(Arc::new(ApiAdapterRegistry::new()))
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        ctx.provide::<ProviderKindRegistry>(Arc::new(ProviderKindRegistry::new()))
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        Ok(())
    }
}
