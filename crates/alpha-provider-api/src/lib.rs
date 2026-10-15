//! Provider descriptors and API adapter contracts.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use alpha_kernel::{ActivateError, CapabilityScope, CapabilitySpec, Ctx, Module};
use alpha_module_credential::CredentialStore;
use alpha_module_http::{HttpClient, HttpRequest};
use alpha_module_loop::{
    ChatMessage, CompletionChunk, CompletionRequest, CompletionResponse, ModelProvider, ModelRoute,
    ModelRouteOverride, ProviderCapabilities, ProviderError, ToolCall, ToolDefinition,
};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use alpha_schema::ReasoningLevel;

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
    /// Selectable reasoning levels for this model.
    #[serde(default = "default_reasoning_levels")]
    pub reasoning_levels: Vec<ReasoningLevel>,
    /// Provider-native tools supported by the model.
    #[serde(default)]
    pub provider_tools: Vec<String>,
}

fn default_reasoning_levels() -> Vec<ReasoningLevel> {
    vec![ReasoningLevel::Auto]
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
    /// Default provider config used when creating an instance.
    pub default_config: Value,
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
    #[serde(default = "default_json_object")]
    pub model_overrides: Value,
    /// Whether the instance is disabled after a failed migration or validation.
    #[serde(default)]
    pub disabled: bool,
}

fn default_json_object() -> Value {
    Value::Object(Default::default())
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

/// Runtime registry of configured provider instances.
#[derive(Default)]
pub struct ProviderInstanceRegistry {
    instances: RwLock<BTreeMap<String, ProviderInstance>>,
}

impl ProviderInstanceRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace one instance.
    pub fn upsert(&self, instance: ProviderInstance) {
        self.instances
            .write()
            .expect("provider instance lock poisoned")
            .insert(instance.id.clone(), instance);
    }

    /// Remove one instance.
    pub fn remove(&self, id: &str) -> Option<ProviderInstance> {
        self.instances
            .write()
            .expect("provider instance lock poisoned")
            .remove(id)
    }

    /// Look up one instance.
    pub fn get(&self, id: &str) -> Option<ProviderInstance> {
        self.instances
            .read()
            .expect("provider instance lock poisoned")
            .get(id)
            .cloned()
    }

    /// List all instances sorted by id.
    pub fn list(&self) -> Vec<ProviderInstance> {
        self.instances
            .read()
            .expect("provider instance lock poisoned")
            .values()
            .cloned()
            .collect()
    }
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
    /// Requested reasoning effort.
    pub reasoning: ReasoningLevel,
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
    session_header: Option<&'static str>,
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
            session_header: None,
            provider_tools,
            options,
        }
    }

    /// Add model-specific adapter overrides.
    pub fn with_model_adapters(mut self, adapters: BTreeMap<String, Arc<dyn ApiAdapter>>) -> Self {
        self.model_adapters = adapters;
        self
    }

    /// Add the session id to requests using a provider-specific routing header.
    pub fn with_session_header(mut self, name: &'static str) -> Self {
        self.session_header = Some(name);
        self
    }

    fn adapter_for(&self, model: &str) -> Arc<dyn ApiAdapter> {
        self.model_adapters
            .get(model)
            .cloned()
            .unwrap_or_else(|| self.adapter.clone())
    }

    fn adapter_request(&self, request: CompletionRequest) -> AdapterRequest {
        let provider_tools = if request.provider_tools.is_empty() {
            self.provider_tools.clone()
        } else {
            request.provider_tools
        };
        let mut headers = self.headers.clone();
        if let Some(name) = self.session_header
            && !request.session_id.is_empty()
            && !headers
                .keys()
                .any(|header| header.eq_ignore_ascii_case(name))
        {
            headers.insert(name.to_owned(), request.session_id);
        }
        AdapterRequest {
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            headers,
            model: request.model,
            reasoning: request.reasoning,
            messages: request.messages,
            tools: request.tools,
            provider_tools,
            options: self.options.clone(),
        }
    }
}

impl ModelProvider for AdapterProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            provider_tools: self.provider_tools.clone(),
        }
    }

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
        self: Arc<Self>,
        request: CompletionRequest,
    ) -> BoxStream<'static, Result<CompletionChunk, ProviderError>> {
        let adapter_request = self.adapter_request(request);
        let adapter = self.adapter_for(&adapter_request.model);
        adapter.stream(self.client.clone(), adapter_request)
    }
}

/// Resolve one configured provider instance.
pub fn resolve_provider_instance(
    root: &Ctx,
    instance_id: &str,
) -> Result<ProviderInstance, ProviderError> {
    let instances = root
        .require::<ProviderInstanceRegistry>()
        .map_err(|error| ProviderError::Request(error.to_string()))?;
    instances.get(instance_id).ok_or_else(|| {
        ProviderError::Request(format!(
            "provider instance `{instance_id}` is not configured"
        ))
    })
}

/// Resolve a configured provider choice.
///
/// An empty choice uses the first registered instance.
/// An exact instance id wins; otherwise the first instance of that kind is used.
pub fn resolve_provider_choice(
    root: &Ctx,
    requested: &str,
) -> Result<(ProviderInstance, Arc<dyn ProviderKind>), ProviderError> {
    let instances = root
        .require::<ProviderInstanceRegistry>()
        .map_err(|error| ProviderError::Request(error.to_string()))?;
    let requested = requested.trim();
    let instance = if requested.is_empty() {
        instances.list().into_iter().next()
    } else {
        instances.get(requested).or_else(|| {
            instances
                .list()
                .into_iter()
                .find(|instance| instance.kind == requested)
        })
    }
    .ok_or_else(|| {
        ProviderError::Request(format!("provider instance `{requested}` is not configured"))
    })?;
    let kinds = root
        .require::<ProviderKindRegistry>()
        .map_err(|error| ProviderError::Request(error.to_string()))?;
    let kind = kinds.get(&instance.kind).ok_or_else(|| {
        ProviderError::Request(format!("provider kind `{}` is missing", instance.kind))
    })?;
    Ok((instance, kind))
}

/// Build a model provider from the current provider instance state.
pub fn build_provider_instance(
    root: &Ctx,
    instance_id: &str,
) -> Result<Arc<dyn ModelProvider>, ProviderError> {
    let instance = resolve_provider_instance(root, instance_id)?;
    if instance.disabled {
        return Err(ProviderError::Request(format!(
            "provider instance `{instance_id}` is disabled"
        )));
    }
    let kinds = root
        .require::<ProviderKindRegistry>()
        .map_err(|error| ProviderError::Request(error.to_string()))?;
    let kind = kinds.get(&instance.kind).ok_or_else(|| {
        ProviderError::Request(format!("provider kind `{}` is missing", instance.kind))
    })?;
    let adapters = root
        .require::<ApiAdapterRegistry>()
        .map_err(|error| ProviderError::Request(error.to_string()))?;
    let http = root
        .require::<dyn HttpClient>()
        .map_err(|error| ProviderError::Request(error.to_string()))?;
    let credential = resolve_credential(root, &instance)?;
    kind.build(
        &instance,
        &ProviderBuildContext {
            adapters,
            http,
            credential,
        },
    )
    .map_err(|error| ProviderError::Request(error.to_string()))
}

/// Resolve the stored credential for one provider instance.
pub fn resolve_credential(
    root: &Ctx,
    instance: &ProviderInstance,
) -> Result<Option<Vec<u8>>, ProviderError> {
    let Some(auth_ref) = instance.auth_ref.as_deref() else {
        return Ok(None);
    };
    let store = root
        .require::<dyn CredentialStore>()
        .map_err(|error| ProviderError::Request(error.to_string()))?;
    store
        .read(auth_ref)
        .map(Some)
        .map_err(|error| ProviderError::Request(error.to_string()))
}

/// Fetch the provider's current model catalog.
pub async fn discover_models(
    instance: &ProviderInstance,
    kind: &dyn ProviderKind,
    client: Arc<dyn HttpClient>,
    credential: Option<Vec<u8>>,
) -> Result<Vec<ModelDescriptor>, ProviderError> {
    let descriptor = kind.descriptor();
    let config = instance.config.as_object();
    let base_url = config
        .and_then(|config| config.get("base_url"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ProviderError::Request(format!(
                "provider instance `{}` has no base_url",
                instance.id
            ))
        })?;
    let credential = credential
        .map(|value| {
            String::from_utf8(value).map_err(|error| ProviderError::Request(error.to_string()))
        })
        .transpose()?;
    let mut request = match instance.kind.as_str() {
        "anthropic-messages" => {
            let url = anthropic_models_endpoint(base_url);
            let mut request = HttpRequest::get(url).with_header("anthropic-version", "2023-06-01");
            if let Some(credential) = credential.as_deref() {
                request = request.with_header("x-api-key", credential);
            }
            request
        }
        "deepseek" => {
            let url = deepseek_models_endpoint(base_url);
            let mut request = HttpRequest::get(url);
            if let Some(credential) = credential.as_deref() {
                request = request.with_header("authorization", format!("Bearer {credential}"));
            }
            request
        }
        _ => {
            let url = format!("{}/models", base_url.trim_end_matches('/'));
            let mut request = HttpRequest::get(url);
            if let Some(credential) = credential.as_deref() {
                request = request.with_header("authorization", format!("Bearer {credential}"));
            }
            request
        }
    };
    if let Some(headers) = config
        .and_then(|config| config.get("headers"))
        .and_then(Value::as_object)
    {
        for (name, value) in headers {
            if let Some(value) = value.as_str() {
                request = request.with_header(name, value);
            }
        }
    }
    let response = client
        .request(request)
        .await
        .map_err(|error| ProviderError::Request(error.to_string()))?;
    if !response.is_success() {
        return Err(ProviderError::Request(format!(
            "model catalog status {}: {}",
            response.status,
            String::from_utf8_lossy(&response.body)
        )));
    }
    let value: Value = serde_json::from_slice(&response.body)
        .map_err(|error| ProviderError::Request(format!("invalid model catalog: {error}")))?;
    let discovered = model_entries(&value);
    if discovered.is_empty() {
        return Err(ProviderError::Request(
            "model catalog returned no models".to_owned(),
        ));
    }
    let static_models = descriptor.models;
    let fallback_adapter = config
        .and_then(|config| config.get("adapter"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| static_models.first().map(|model| model.adapter.clone()))
        .or_else(|| {
            descriptor
                .adapters
                .first()
                .map(|adapter| adapter.key.clone())
        })
        .ok_or_else(|| ProviderError::Request("provider has no adapter".to_owned()))?;
    let reasoning = static_models.iter().any(|model| model.reasoning);
    let mut seen = BTreeMap::new();
    for (id, name) in discovered {
        if seen.contains_key(&id) {
            continue;
        }
        let inferred_reasoning = reasoning || model_id_is_reasoning(&id);
        let model = static_models
            .iter()
            .find(|model| model.id == id)
            .cloned()
            .map(|mut model| {
                if let Some(name) = name.clone() {
                    model.name = name;
                }
                model
            })
            .unwrap_or_else(|| ModelDescriptor {
                id: id.clone(),
                name: name.unwrap_or_else(|| id.clone()),
                adapter: fallback_adapter.clone(),
                context_window: 128_000,
                max_tokens: 8_192,
                reasoning: inferred_reasoning,
                reasoning_levels: if inferred_reasoning {
                    ReasoningLevel::ALL.to_vec()
                } else {
                    vec![ReasoningLevel::Auto]
                },
                provider_tools: Vec::new(),
            });
        seen.insert(id, model);
    }
    Ok(seen.into_values().collect())
}

fn model_id_is_reasoning(id: &str) -> bool {
    id.starts_with("gpt-5")
        || id.starts_with("o1")
        || id.starts_with("o3")
        || id.starts_with("o4")
        || id.contains("reason")
        || id.contains("thinking")
}

fn anthropic_models_endpoint(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/models")
    } else {
        format!("{base}/v1/models")
    }
}

fn deepseek_models_endpoint(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let base = base.strip_suffix("/anthropic").unwrap_or(base);
    format!("{base}/models")
}

fn model_entries(value: &Value) -> Vec<(String, Option<String>)> {
    let entries = value
        .get("data")
        .or_else(|| value.get("models"))
        .and_then(Value::as_array)
        .or_else(|| value.as_array());
    let Some(entries) = entries else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            if let Some(id) = entry.as_str() {
                return Some((id.to_owned(), None));
            }
            let id = entry.get("id").and_then(Value::as_str)?;
            let name = entry
                .get("display_name")
                .or_else(|| entry.get("name"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            Some((id.to_owned(), name))
        })
        .collect()
}

/// Apply user metadata overrides to a provider kind model catalog.
pub fn effective_model_catalog(
    instance: &ProviderInstance,
    kind: &dyn ProviderKind,
) -> Vec<ModelDescriptor> {
    effective_models(instance, &kind.descriptor().models)
}

/// Apply user metadata overrides to a model list.
pub fn effective_models(
    instance: &ProviderInstance,
    models: &[ModelDescriptor],
) -> Vec<ModelDescriptor> {
    let overrides = instance.model_overrides.as_object();
    models
        .iter()
        .cloned()
        .map(|mut model| {
            let Some(override_value) = overrides
                .and_then(|overrides| overrides.get(&model.id))
                .and_then(Value::as_object)
            else {
                return model;
            };
            if let Some(name) = override_value.get("name").and_then(Value::as_str) {
                model.name = name.to_owned();
            }
            if let Some(adapter) = override_value.get("adapter").and_then(Value::as_str) {
                model.adapter = adapter.to_owned();
            }
            if let Some(context_window) =
                override_value.get("context_window").and_then(Value::as_u64)
            {
                model.context_window = context_window;
            }
            if let Some(max_tokens) = override_value.get("max_tokens").and_then(Value::as_u64) {
                model.max_tokens = max_tokens;
            }
            let explicit_reasoning = override_value.get("reasoning").and_then(Value::as_bool);
            if let Some(reasoning) = explicit_reasoning {
                model.reasoning = reasoning;
                if reasoning && model.reasoning_levels.as_slice() == [ReasoningLevel::Auto] {
                    model.reasoning_levels = ReasoningLevel::ALL.to_vec();
                }
            }
            if let Some(levels) = override_value
                .get("reasoning_levels")
                .or_else(|| override_value.get("thinking_levels"))
                .cloned()
                .and_then(|value| serde_json::from_value::<Vec<ReasoningLevel>>(value).ok())
            {
                if explicit_reasoning.is_none() {
                    model.reasoning = levels.iter().any(|level| *level != ReasoningLevel::Auto);
                }
                model.reasoning_levels = levels;
            }
            if let Some(tools) = override_value
                .get("provider_tools")
                .and_then(Value::as_array)
            {
                model.provider_tools = tools
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect();
            }
            model
        })
        .collect()
}

/// Select the default model after applying user overrides.
pub fn default_model_id(instance: &ProviderInstance, kind: &dyn ProviderKind) -> Option<String> {
    instance
        .model_overrides
        .get("default")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            effective_model_catalog(instance, kind)
                .first()
                .map(|model| model.id.clone())
        })
}

/// Provider wrapper that rebuilds from the current registry on each request.
pub struct DynamicProvider {
    root: Ctx,
    instance_id: String,
    route: Option<Arc<ModelRouteOverride>>,
}

impl DynamicProvider {
    /// Create a dynamic provider for one instance id.
    pub fn new(root: Ctx, instance_id: impl Into<String>) -> Self {
        Self {
            root,
            instance_id: instance_id.into(),
            route: None,
        }
    }

    /// Create a dynamic provider whose instance and model follow a session route.
    pub fn new_routed(
        root: Ctx,
        instance_id: impl Into<String>,
        route: Arc<ModelRouteOverride>,
    ) -> Self {
        Self {
            root,
            instance_id: instance_id.into(),
            route: Some(route),
        }
    }

    fn current_route(&self) -> ModelRoute {
        self.route
            .as_ref()
            .map(|route| route.get())
            .unwrap_or_else(|| ModelRoute::new(self.instance_id.clone(), "", ReasoningLevel::Auto))
    }

    fn prepare_request(
        &self,
        mut request: CompletionRequest,
    ) -> Result<(Arc<dyn ModelProvider>, CompletionRequest), ProviderError> {
        let route = self.current_route();
        let (instance, kind) = resolve_provider_choice(&self.root, &route.provider)?;
        let model = if route.model.trim().is_empty() {
            default_model_id(&instance, kind.as_ref()).ok_or_else(|| {
                ProviderError::Request(format!("provider kind `{}` has no models", instance.kind))
            })?
        } else {
            route.model
        };
        request.model = model;
        request.reasoning = route.reasoning;
        let provider = build_provider_instance(&self.root, &instance.id)?;
        Ok((provider, request))
    }
}

impl ModelProvider for DynamicProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        let route = self.current_route();
        resolve_provider_choice(&self.root, &route.provider)
            .and_then(|(instance, _)| build_provider_instance(&self.root, &instance.id))
            .map(|provider| provider.capabilities())
            .unwrap_or_default()
    }

    fn complete(
        &self,
        request: CompletionRequest,
    ) -> BoxFuture<'_, Result<CompletionResponse, ProviderError>> {
        let (provider, request) = match self.prepare_request(request) {
            Ok(prepared) => prepared,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        Box::pin(async move { provider.complete(request).await })
    }

    fn stream(
        self: Arc<Self>,
        request: CompletionRequest,
    ) -> BoxStream<'static, Result<CompletionChunk, ProviderError>> {
        match self.prepare_request(request) {
            Ok((provider, request)) => provider.stream(request),
            Err(error) => Box::pin(futures::stream::iter(vec![Err(error)])),
        }
    }
}

async fn collect_stream(
    stream: &mut BoxStream<'_, Result<CompletionChunk, ProviderError>>,
) -> Result<CompletionResponse, ProviderError> {
    use futures::StreamExt;

    let mut text = String::new();
    let mut calls: BTreeMap<usize, ToolCallBuilder> = BTreeMap::new();
    let mut blocks: Vec<alpha_schema::AssistantContentBlock> = Vec::new();
    while let Some(chunk) = stream.next().await {
        match chunk? {
            CompletionChunk::Text(delta) => {
                text.push_str(&delta);
                if let Some(alpha_schema::AssistantContentBlock::Text { text }) = blocks.last_mut()
                {
                    text.push_str(&delta);
                } else {
                    blocks.push(alpha_schema::AssistantContentBlock::Text { text: delta });
                }
            }
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
            CompletionChunk::Thinking { text, signature } => {
                if let Some(alpha_schema::AssistantContentBlock::Thinking {
                    text: current,
                    signature: current_signature,
                }) = blocks.last_mut()
                {
                    current.push_str(&text);
                    if signature.is_some() {
                        *current_signature = signature;
                    }
                } else {
                    blocks.push(alpha_schema::AssistantContentBlock::Thinking { text, signature });
                }
            }
            CompletionChunk::ProviderToolCall { id, name, input } => {
                blocks.push(alpha_schema::AssistantContentBlock::ProviderToolCall {
                    id,
                    name,
                    input,
                });
            }
            CompletionChunk::ProviderToolResult { id, name, content } => {
                blocks.push(alpha_schema::AssistantContentBlock::ProviderToolResult {
                    id,
                    name,
                    content,
                });
            }
            CompletionChunk::Citation {
                url,
                title,
                snippet,
            } => {
                blocks.push(alpha_schema::AssistantContentBlock::Citation {
                    url,
                    title,
                    snippet,
                });
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
        let call = ToolCall {
            id,
            name,
            arguments,
        };
        blocks.push(alpha_schema::AssistantContentBlock::LocalToolCall {
            id: call.id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
        });
        tool_calls.push(call);
    }
    Ok(CompletionResponse {
        text,
        tool_calls,
        blocks,
    })
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
        CapabilitySpec::singleton("provider.instances", CapabilityScope::Global),
    ];

    fn activate(&self, ctx: &Ctx, _config: &Self::Config) -> Result<(), ActivateError> {
        ctx.provide::<ApiAdapterRegistry>(Arc::new(ApiAdapterRegistry::new()))
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        ctx.provide::<ProviderKindRegistry>(Arc::new(ProviderKindRegistry::new()))
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        ctx.provide::<ProviderInstanceRegistry>(Arc::new(ProviderInstanceRegistry::new()))
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        Ok(())
    }
}
