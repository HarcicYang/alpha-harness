//! OpenCode Go provider kind.

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
pub const __ALPHA_MODULE__: &str = "provider.opencode-go";
/// Provider kind.
pub const PROVIDER_KIND: &str = "opencode-go";

/// OpenCode Go provider config.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct OpenCodeGoConfig {
    /// API base URL.
    pub base_url: String,
    /// Adapter selected for this instance.
    #[serde(default = "default_adapter")]
    pub adapter: String,
    /// Extra request headers.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Adapter-specific request options.
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, String>")]
    pub options: BTreeMap<String, String>,
    /// Optional OpenCode session routing id.
    #[serde(default)]
    pub session_id: Option<String>,
}

impl Default for OpenCodeGoConfig {
    fn default() -> Self {
        Self {
            base_url: "https://opencode.ai/zen/v1".to_owned(),
            adapter: default_adapter(),
            headers: BTreeMap::new(),
            options: BTreeMap::new(),
            session_id: None,
        }
    }
}

fn default_adapter() -> String {
    "anthropic-messages".to_owned()
}

/// OpenCode Go provider kind.
pub struct OpenCodeGoProviderKind;

impl ProviderKind for OpenCodeGoProviderKind {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            kind: PROVIDER_KIND.to_owned(),
            name: "OpenCode Go".to_owned(),
            config_schema: serde_json::to_value(schemars::schema_for!(OpenCodeGoConfig))
                .unwrap_or_else(|_| json!({})),
            auth_methods: vec![AuthMethodDescriptor {
                key: "api_key".to_owned(),
                label: "OpenCode API key".to_owned(),
                interactive: true,
            }],
            models: vec![
                model("gpt-5", "openai-responses", 400_000, 128_000),
                model("claude-sonnet-4", "anthropic-messages", 200_000, 64_000),
                model("deepseek-v4-pro", "openai-chat-completions", 128_000, 8_192),
            ],
            adapters: vec![
                AdapterRequirement {
                    key: "anthropic-messages".to_owned(),
                    required: true,
                },
                AdapterRequirement {
                    key: "openai-responses".to_owned(),
                    required: false,
                },
                AdapterRequirement {
                    key: "openai-chat-completions".to_owned(),
                    required: false,
                },
            ],
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
        let config: OpenCodeGoConfig = serde_json::from_value(instance.config.clone())
            .map_err(|error| ProviderKindError::InvalidConfig(error.to_string()))?;
        let api_key = required_credential_text(instance, context)?;
        let adapter = required_adapter(&config.adapter, context)?;
        let model_adapters = self
            .descriptor()
            .models
            .into_iter()
            .filter_map(|model| {
                context
                    .adapters
                    .get(&model.adapter)
                    .map(|adapter| (model.id, adapter))
            })
            .collect();
        let mut headers = config.headers;
        if let Some(session_id) = config.session_id {
            headers.insert("x-opencode-session".to_owned(), session_id);
        }
        let options = serde_json::to_value(config.options)
            .map_err(|error| ProviderKindError::Build(error.to_string()))?;
        Ok(Arc::new(
            AdapterProvider::new(
                adapter,
                context.http.clone(),
                config.base_url,
                Some(api_key),
                headers,
                Vec::new(),
                options,
            )
            .with_model_adapters(model_adapters),
        ))
    }
}

fn model(id: &str, adapter: &str, context_window: u64, max_tokens: u64) -> ModelDescriptor {
    ModelDescriptor {
        id: id.to_owned(),
        name: id.to_owned(),
        adapter: adapter.to_owned(),
        context_window,
        max_tokens,
        reasoning: true,
        provider_tools: Vec::new(),
    }
}

/// Registers the OpenCode Go provider.
pub struct OpenCodeGoProviderModule;

impl Module for OpenCodeGoProviderModule {
    type Requirements = ();
    type Config = ();

    const KEY: &'static str = "provider.opencode-go";

    fn activate(&self, ctx: &Ctx, _config: &Self::Config) -> Result<(), ActivateError> {
        let registry = ctx
            .require::<ProviderKindRegistry>()
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        let kind: Arc<dyn ProviderKind> = Arc::new(OpenCodeGoProviderKind);
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
