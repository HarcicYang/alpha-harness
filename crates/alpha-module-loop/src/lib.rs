//! Default agent driver module, replaceable like any module.

#![forbid(unsafe_code)]

/// Stable module key used by the generated composition table.
pub const __ALPHA_MODULE__: &str = "loop";

pub mod agent;
pub mod provider;

use std::sync::Arc;

use alpha_kernel::{ActivateError, CapabilityScope, CapabilitySpec, Ctx, Module};
use serde::Deserialize;

pub use agent::{AgentDriver, AgentLoop, LoopError, TurnOutcome};
pub use provider::{
    ChatMessage, CompletionRequest, CompletionResponse, ModelProvider, ProviderError,
    ProviderRegistry, RegistryError, ToolCall, ToolDefinition,
};

/// Configuration for the default loop module.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct LoopConfig {
    /// Provider name resolved from the registry.
    pub provider: String,
    /// Model identifier.
    pub model: String,
}

/// Registers the provider registry and the default agent loop.
pub struct LoopModule;

impl Module for LoopModule {
    type Requirements = ();
    type Config = LoopConfig;

    const KEY: &'static str = "loop";
    const CAPABILITIES: &'static [CapabilitySpec] = &[
        CapabilitySpec::singleton("agent.loop", CapabilityScope::Agent),
        CapabilitySpec::registry("model.provider", CapabilityScope::Global),
    ];

    fn activate(&self, ctx: &Ctx, config: &Self::Config) -> Result<(), ActivateError> {
        let registry = match ctx.get::<ProviderRegistry>() {
            Some(registry) => registry,
            None => {
                let registry = Arc::new(ProviderRegistry::new());
                ctx.provide::<ProviderRegistry>(registry.clone())
                    .map_err(|error| ActivateError::new(Self::KEY, error))?;
                registry
            }
        };
        let provider = registry.get(&config.provider).ok_or_else(|| {
            ActivateError::message(
                Self::KEY,
                format!("provider `{}` is not registered", config.provider),
            )
        })?;
        let mut agent = AgentLoop::new(provider, config.model.clone());
        if let Some(tools) = ctx.get::<alpha_module_tools::ToolRegistry>() {
            agent = agent.with_tools(tools);
        }
        let agent = Arc::new(agent);
        ctx.provide::<dyn AgentDriver>(agent.clone())
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        ctx.provide::<AgentLoop>(agent)
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        Ok(())
    }
}
