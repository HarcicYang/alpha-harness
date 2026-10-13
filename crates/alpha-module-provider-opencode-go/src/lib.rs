//! OpenCode Go provider kind.

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
use serde_json::{Value, json};

/// Stable module key.
pub const __ALPHA_MODULE__: &str = "provider.opencode-go";
/// Provider kind.
pub const PROVIDER_KIND: &str = "opencode-go";
/// Current OpenCode Go API base URL.
pub const DEFAULT_BASE_URL: &str = "https://opencode.ai/zen/go/v1";
const LEGACY_BASE_URL: &str = "https://opencode.ai/zen/v1";
const LEGACY_BASE_ROOT: &str = "https://opencode.ai/zen";

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

impl Default for OpenCodeGoConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_owned(),
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
            default_config: serde_json::to_value(OpenCodeGoConfig::default())
                .unwrap_or_else(|_| json!({})),
            auth_methods: vec![AuthMethodDescriptor {
                key: "api_key".to_owned(),
                label: "OpenCode API key".to_owned(),
                interactive: true,
            }],
            models: vec![
                model_with_levels(
                    "deepseek-v4.1-flash",
                    "openai-chat-completions",
                    1_000_000,
                    384_000,
                    vec![
                        ReasoningLevel::Auto,
                        ReasoningLevel::Low,
                        ReasoningLevel::High,
                        ReasoningLevel::Max,
                    ],
                ),
                model_with_levels(
                    "deepseek-v4-flash",
                    "openai-chat-completions",
                    1_000_000,
                    384_000,
                    vec![
                        ReasoningLevel::Auto,
                        ReasoningLevel::Low,
                        ReasoningLevel::High,
                        ReasoningLevel::Max,
                    ],
                ),
                model_with_levels(
                    "deepseek-v4-pro",
                    "openai-chat-completions",
                    1_000_000,
                    384_000,
                    vec![
                        ReasoningLevel::Auto,
                        ReasoningLevel::High,
                        ReasoningLevel::Max,
                    ],
                ),
                model(
                    "deepseek-v4-flash-vision-exp",
                    "openai-chat-completions",
                    1_000_000,
                    384_000,
                ),
                model("claude-haiku-5-5", "anthropic-messages", 1_000_000, 128_000),
                model("minimax-m3", "anthropic-messages", 1_000_000, 131_072),
                model("qwen3.8-flash", "anthropic-messages", 1_000_000, 131_072),
                model("qwen3.8-max", "anthropic-messages", 1_000_000, 131_072),
                model("gpt-5.6-luna", "openai-responses", 1_050_000, 128_000),
                model("gpt-6-luna", "openai-responses", 1_050_000, 128_000),
                model("grok-4.7", "openai-responses", 500_000, 500_000),
                model(
                    "muse-spark-1.3-contributor",
                    "openai-responses",
                    1_048_576,
                    131_072,
                ),
                model("glm-5.2", "openai-chat-completions", 1_000_000, 131_072),
                model("glm-5.3", "openai-chat-completions", 1_000_000, 131_072),
                model(
                    "glm-5.3-flash",
                    "openai-chat-completions",
                    1_000_000,
                    131_072,
                ),
                model(
                    "kimi-k2.7-code",
                    "openai-chat-completions",
                    262_144,
                    262_144,
                ),
                model("kimi-k3", "openai-chat-completions", 1_048_576, 131_072),
                model(
                    "mimo-v2.6-pro",
                    "openai-chat-completions",
                    1_048_576,
                    131_072,
                ),
                model("hy4-preview", "openai-chat-completions", 1_024_000, 64_000),
                model(
                    "longcat-2.5-preview-free",
                    "openai-chat-completions",
                    1_000_000,
                    131_072,
                ),
                model("space-bunny", "openai-chat-completions", 1_048_576, 524_288),
                model(
                    "step-5-preview-free",
                    "openai-chat-completions",
                    1_000_000,
                    65_536,
                ),
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
        2
    }

    fn migrate_config(&self, from: u32, config: Value) -> Result<Value, ProviderKindError> {
        if from != 1 {
            return Err(ProviderKindError::Migration(format!(
                "no migration from version {from} to 2"
            )));
        }
        let mut config: OpenCodeGoConfig = serde_json::from_value(config)
            .map_err(|error| ProviderKindError::InvalidConfig(error.to_string()))?;
        let base = config.base_url.trim_end_matches('/');
        if base == LEGACY_BASE_URL || base == LEGACY_BASE_ROOT {
            config.base_url = DEFAULT_BASE_URL.to_owned();
        }
        serde_json::to_value(config).map_err(|error| ProviderKindError::Build(error.to_string()))
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
            .with_model_adapters(model_adapters)
            .with_session_header("x-opencode-session"),
        ))
    }
}

fn model(id: &str, adapter: &str, context_window: u64, max_tokens: u64) -> ModelDescriptor {
    model_with_levels(
        id,
        adapter,
        context_window,
        max_tokens,
        ReasoningLevel::ALL.to_vec(),
    )
}

fn model_with_levels(
    id: &str,
    adapter: &str,
    context_window: u64,
    max_tokens: u64,
    reasoning_levels: Vec<ReasoningLevel>,
) -> ModelDescriptor {
    ModelDescriptor {
        id: id.to_owned(),
        name: id.to_owned(),
        adapter: adapter.to_owned(),
        context_window,
        max_tokens,
        reasoning: true,
        reasoning_levels,
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
