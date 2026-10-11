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
    /// Explicit API key supplied by the caller or credential seam.
    pub api_key: String,
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".to_owned(),
            api_key: String::new(),
        }
    }
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
        if config.api_key.trim().is_empty() {
            return Err(ActivateError::message(
                Self::KEY,
                "configure `api_key` for the provider",
            ));
        }
        let api_key = config.api_key.clone();

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
