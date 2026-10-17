//! Provider metadata shared by core modules and frontends.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ReasoningLevel;

/// Credential backend availability.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialBackendStatus {
    /// The backend can be used.
    Available,
    /// The backend cannot be accessed on this system.
    Unavailable,
}

/// Human-readable credential backend status.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialStatus {
    /// Backend name.
    pub backend: String,
    /// Availability.
    pub status: CredentialBackendStatus,
    /// Optional diagnostic detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

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

/// Provider kind metadata consumed by frontends.
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
pub fn default_model_id(instance: &ProviderInstance, models: &[ModelDescriptor]) -> Option<String> {
    instance
        .model_overrides
        .get("default")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            effective_models(instance, models)
                .first()
                .map(|model| model.id.clone())
        })
}
