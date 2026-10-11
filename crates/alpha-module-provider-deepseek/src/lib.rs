//! DeepSeek provider kind.

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
pub const __ALPHA_MODULE__: &str = "provider.deepseek";
/// Provider kind.
pub const PROVIDER_KIND: &str = "deepseek";
/// Adapter key.
pub const ADAPTER_KEY: &str = "anthropic-messages";

/// DeepSeek provider config.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct DeepSeekConfig {
    /// API base URL.
    pub base_url: String,
    /// Extra request headers.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Adapter-specific request options.
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, String>")]
    pub options: BTreeMap<String, String>,
    /// Enable the provider-native web search tool.
    #[serde(default = "default_true")]
    pub enable_web_search: bool,
}

impl Default for DeepSeekConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.deepseek.com".to_owned(),
            headers: BTreeMap::new(),
            options: BTreeMap::new(),
            enable_web_search: true,
        }
    }
}

fn default_true() -> bool {
    true
}

/// DeepSeek provider kind.
pub struct DeepSeekProviderKind;

impl ProviderKind for DeepSeekProviderKind {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            kind: PROVIDER_KIND.to_owned(),
            name: "DeepSeek".to_owned(),
            config_schema: serde_json::to_value(schemars::schema_for!(DeepSeekConfig))
                .unwrap_or_else(|_| json!({})),
            auth_methods: vec![AuthMethodDescriptor {
                key: "api_key".to_owned(),
                label: "DeepSeek API key".to_owned(),
                interactive: true,
            }],
            models: vec![
                model("deepseek-chat", 128_000, 8_192, true),
                model("deepseek-reasoner", 128_000, 8_192, true),
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
        let config: DeepSeekConfig = serde_json::from_value(instance.config.clone())
            .map_err(|error| ProviderKindError::InvalidConfig(error.to_string()))?;
        let api_key = required_credential_text(instance, context)?;
        let adapter = required_adapter(ADAPTER_KEY, context)?;
        let options = serde_json::to_value(config.options)
            .map_err(|error| ProviderKindError::Build(error.to_string()))?;
        let provider_tools = if config.enable_web_search {
            vec!["web_search".to_owned()]
        } else {
            Vec::new()
        };
        Ok(Arc::new(AdapterProvider::new(
            adapter,
            context.http.clone(),
            config.base_url,
            Some(api_key),
            config.headers,
            provider_tools,
            options,
        )))
    }
}

fn model(id: &str, context_window: u64, max_tokens: u64, web_search: bool) -> ModelDescriptor {
    ModelDescriptor {
        id: id.to_owned(),
        name: id.to_owned(),
        adapter: ADAPTER_KEY.to_owned(),
        context_window,
        max_tokens,
        reasoning: true,
        provider_tools: if web_search {
            vec!["web_search".to_owned()]
        } else {
            Vec::new()
        },
    }
}

/// Registers the DeepSeek provider.
pub struct DeepSeekProviderModule;

impl Module for DeepSeekProviderModule {
    type Requirements = ();
    type Config = ();

    const KEY: &'static str = "provider.deepseek";

    fn activate(&self, ctx: &Ctx, _config: &Self::Config) -> Result<(), ActivateError> {
        let registry = ctx
            .require::<ProviderKindRegistry>()
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        let kind: Arc<dyn ProviderKind> = Arc::new(DeepSeekProviderKind);
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
