//! Generic OpenAI Chat Completions provider kind.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::Arc;

use alpha_kernel::{ActivateError, Ctx, Module};
use alpha_module_loop::ModelProvider;
use alpha_provider_api::{
    AdapterProvider, AdapterRequirement, AuthMethodDescriptor, ModelDescriptor,
    ProviderBuildContext, ProviderDescriptor, ProviderInstance, ProviderKind, ProviderKindError,
    ProviderKindRegistry, ReasoningLevel, required_adapter, required_credential_text,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Stable module key used by the generated composition table.
pub const __ALPHA_MODULE__: &str = "provider.openai-chat-completions";
/// Provider kind.
pub const PROVIDER_KIND: &str = "openai-chat-completions";
/// Adapter key.
pub const ADAPTER_KEY: &str = "openai-chat-completions";

/// Provider config.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct OpenAiChatConfig {
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

impl Default for OpenAiChatConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".to_owned(),
            headers: BTreeMap::new(),
            options: BTreeMap::new(),
        }
    }
}

/// Generic OpenAI Chat Completions provider kind.
pub struct OpenAiChatProviderKind;

impl ProviderKind for OpenAiChatProviderKind {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            kind: PROVIDER_KIND.to_owned(),
            name: "OpenAI Chat Completions".to_owned(),
            config_schema: serde_json::to_value(schemars::schema_for!(OpenAiChatConfig))
                .unwrap_or_else(|_| json!({})),
            default_config: serde_json::to_value(OpenAiChatConfig::default())
                .unwrap_or_else(|_| json!({})),
            auth_methods: vec![AuthMethodDescriptor {
                key: "api_key".to_owned(),
                label: "API key".to_owned(),
                interactive: true,
            }],
            models: vec![
                model("gpt-4.1", 1_000_000, 32_768),
                model("gpt-4.1-mini", 1_000_000, 32_768),
                model("gpt-4o-mini", 128_000, 16_384),
                model("gpt-4o", 128_000, 16_384),
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
        let config: OpenAiChatConfig = serde_json::from_value(instance.config.clone())
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

fn model(id: &str, context_window: u64, max_tokens: u64) -> ModelDescriptor {
    ModelDescriptor {
        id: id.to_owned(),
        name: id.to_owned(),
        adapter: ADAPTER_KEY.to_owned(),
        context_window,
        max_tokens,
        reasoning: false,
        reasoning_levels: vec![ReasoningLevel::Auto],
        provider_tools: Vec::new(),
    }
}

/// Registers the generic provider kind.
pub struct OpenAiChatProviderModule;

impl Module for OpenAiChatProviderModule {
    type Requirements = ();
    type Config = ();

    const KEY: &'static str = "provider.openai-chat-completions";

    fn activate(&self, ctx: &Ctx, _config: &Self::Config) -> Result<(), ActivateError> {
        let registry = ctx
            .require::<ProviderKindRegistry>()
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        let kind: Arc<dyn ProviderKind> = Arc::new(OpenAiChatProviderKind);
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
