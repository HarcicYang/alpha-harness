//! Generic Anthropic Messages provider kind.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::Arc;

use alpha_kernel::{ActivateError, Ctx, Module};
use alpha_module_loop::ModelProvider;
use alpha_provider_api::{
    AdapterProvider, AdapterRequirement, AuthMethodDescriptor, ModelDescriptor,
    ProviderBuildContext, ProviderDescriptor, ProviderInstance, ProviderKind, ProviderKindError,
    ProviderKindRegistry, required_adapter, required_credential_text,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Stable module key.
pub const __ALPHA_MODULE__: &str = "provider.anthropic-messages";
/// Provider kind.
pub const PROVIDER_KIND: &str = "anthropic-messages";
/// Adapter key.
pub const ADAPTER_KEY: &str = "anthropic-messages";

/// Provider config.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AnthropicMessagesConfig {
    /// API base URL.
    pub base_url: String,
    /// Extra request headers.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Adapter-specific request options.
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, String>")]
    pub options: BTreeMap<String, String>,
}

impl Default for AnthropicMessagesConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.anthropic.com".to_owned(),
            headers: BTreeMap::new(),
            options: BTreeMap::new(),
        }
    }
}

/// Generic Anthropic Messages provider kind.
pub struct AnthropicMessagesProviderKind;

impl ProviderKind for AnthropicMessagesProviderKind {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            kind: PROVIDER_KIND.to_owned(),
            name: "Anthropic Messages".to_owned(),
            config_schema: serde_json::to_value(schemars::schema_for!(AnthropicMessagesConfig))
                .unwrap_or_else(|_| json!({})),
            auth_methods: vec![AuthMethodDescriptor {
                key: "api_key".to_owned(),
                label: "API key".to_owned(),
                interactive: true,
            }],
            models: vec![
                model("claude-sonnet-4-20250514", 200_000, 64_000, false),
                model("claude-opus-4-20250514", 200_000, 32_000, true),
                model("claude-3-7-sonnet-20250219", 200_000, 64_000, true),
            ],
            adapters: vec![AdapterRequirement {
                key: ADAPTER_KEY.to_owned(),
                required: true,
            }],
        }
    }

    fn config_version(&self) -> u32 {
        1
    }

    fn build(
        &self,
        instance: &ProviderInstance,
        context: &ProviderBuildContext,
    ) -> Result<Arc<dyn ModelProvider>, ProviderKindError> {
        let config: AnthropicMessagesConfig = serde_json::from_value(instance.config.clone())
            .map_err(|error| ProviderKindError::InvalidConfig(error.to_string()))?;
        let api_key = required_credential_text(instance, context)?;
        let adapter = required_adapter(ADAPTER_KEY, context)?;
        let options = serde_json::to_value(config.options)
            .map_err(|error| ProviderKindError::Build(error.to_string()))?;
        Ok(Arc::new(AdapterProvider::new(
            adapter,
            context.http.clone(),
            config.base_url,
            Some(api_key),
            config.headers,
            Vec::new(),
            options,
        )))
    }
}

fn model(id: &str, context_window: u64, max_tokens: u64, provider_tools: bool) -> ModelDescriptor {
    ModelDescriptor {
        id: id.to_owned(),
        name: id.to_owned(),
        adapter: ADAPTER_KEY.to_owned(),
        context_window,
        max_tokens,
        reasoning: true,
        provider_tools: if provider_tools {
            vec!["web_search".to_owned()]
        } else {
            Vec::new()
        },
    }
}

/// Registers the generic provider kind.
pub struct AnthropicMessagesProviderModule;

impl Module for AnthropicMessagesProviderModule {
    type Requirements = ();
    type Config = ();

    const KEY: &'static str = "provider.anthropic-messages";

    fn activate(&self, ctx: &Ctx, _config: &Self::Config) -> Result<(), ActivateError> {
        let registry = ctx
            .require::<ProviderKindRegistry>()
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        let kind: Arc<dyn ProviderKind> = Arc::new(AnthropicMessagesProviderKind);
        registry
            .register(kind)
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        let cleanup = registry.clone();
        ctx.scope()
            .effect(move || {
                cleanup.unregister(PROVIDER_KIND);
            })
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        Ok(())
    }
}
