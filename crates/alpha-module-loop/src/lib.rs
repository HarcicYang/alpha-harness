//! Default agent driver module, replaceable like any module.

#![forbid(unsafe_code)]

/// Stable module key used by the generated composition table.
pub const __ALPHA_MODULE__: &str = "loop";

pub mod agent;
pub mod provider;

use std::sync::Arc;

use alpha_kernel::{ActivateError, CapabilityScope, CapabilitySpec, Ctx, Module};
use serde::Deserialize;

pub use agent::{
    AgentDriver, AgentError, AgentLoop, AgentStepStarted, AgentTextDelta, AgentToolFinished,
    AgentToolStarted, AgentTurnFinished, ContextError, ContextPreparer, LoopError, TurnOutcome,
};
pub use provider::{
    ChatMessage, CompletionChunk, CompletionRequest, CompletionResponse, ModelProvider,
    ProviderError, ProviderRegistry, RegistryError, ToolCall, ToolCallDelta, ToolDefinition,
};

/// Configuration for the default loop module.
#[derive(Clone, Debug, Deserialize)]
pub struct LoopConfig {
    /// Provider name resolved from the registry.
    pub provider: String,
    /// Model identifier.
    pub model: String,
    /// Maximum model steps in one turn.
    #[serde(default = "default_max_steps")]
    pub max_steps: usize,
}

fn default_max_steps() -> usize {
    24
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            provider: String::new(),
            model: String::new(),
            max_steps: default_max_steps(),
        }
    }
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
        let mut agent =
            AgentLoop::new(provider, config.model.clone()).with_max_steps(config.max_steps);
        if let Some(tools) = ctx.get::<alpha_module_tools::ToolRegistry>() {
            agent = agent.with_tools(tools);
        }
        if let Some(preparer) = ctx.get::<dyn ContextPreparer>() {
            agent = agent.with_context_preparer(preparer);
        }
        let agent = Arc::new(agent);
        ctx.provide::<dyn AgentDriver>(agent.clone())
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        ctx.provide::<AgentLoop>(agent)
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        Ok(())
    }
}
