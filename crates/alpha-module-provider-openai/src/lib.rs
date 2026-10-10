//! OpenAI-compatible model provider module.

#![forbid(unsafe_code)]

pub mod provider;

use std::sync::Arc;

use alpha_kernel::{ActivateError, CapabilitySpec, Ctx, Module};
use alpha_module_http::HttpClient;
use alpha_module_loop::ProviderRegistry;
use serde::Deserialize;

/// Stable module key used by the generated composition table.
pub const __ALPHA_MODULE__: &str = "provider.openai";

pub use provider::{OpenAiProvider, PROVIDER_NAME};

/// Configuration for the OpenAI-compatible provider.
#[derive(Clone, Debug, Deserialize)]
pub struct OpenAiConfig {
    /// API base URL, for example `https://api.openai.com/v1`.
    pub base_url: String,
    /// Environment variable holding the API key.
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,
    /// Explicit API key, intended for tests and CI. Prefer the environment.
    #[serde(default)]
    pub api_key: Option<String>,
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".to_owned(),
            api_key_env: default_api_key_env(),
            api_key: None,
        }
    }
}

fn default_api_key_env() -> String {
    "OPENAI_API_KEY".to_owned()
}

/// Registers the OpenAI-compatible provider into the loop provider registry.
pub struct OpenAiProviderModule;

impl Module for OpenAiProviderModule {
    type Requirements = ();
    type Config = OpenAiConfig;

    const KEY: &'static str = "provider.openai";
    const CAPABILITIES: &'static [CapabilitySpec] = &[];

    fn activate(&self, ctx: &Ctx, config: &Self::Config) -> Result<(), ActivateError> {
        let registry = ctx
            .require::<ProviderRegistry>()
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        let client = ctx
            .require::<dyn HttpClient>()
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        let api_key = config
            .api_key
            .clone()
            .or_else(|| std::env::var(&config.api_key_env).ok())
            .ok_or_else(|| {
                ActivateError::message(
                    Self::KEY,
                    format!(
                        "set `{}` or configure `api_key` for the provider",
                        config.api_key_env
                    ),
                )
            })?;

        let provider = Arc::new(OpenAiProvider::new(
            client,
            config.base_url.clone(),
            api_key,
        ));
        registry
            .register(PROVIDER_NAME, provider)
            .map_err(|error| ActivateError::new(Self::KEY, error))?;

        let cleanup_registry = registry.clone();
        let cleanup_name = PROVIDER_NAME.to_owned();
        match ctx.scope().effect(move || {
            cleanup_registry.unregister(&cleanup_name);
        }) {
            Ok(_) => Ok(()),
            Err(error) => {
                registry.unregister(PROVIDER_NAME);
                Err(ActivateError::new(Self::KEY, error))
            }
        }
    }
}
