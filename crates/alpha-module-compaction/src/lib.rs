//! Budget-based context compaction.

use std::sync::Arc;

use alpha_kernel::{ActivateError, CapabilityScope, CapabilitySpec, Ctx, Module};
use alpha_module_loop::ContextPreparer;
use serde::Deserialize;

mod compactor;

pub use compactor::{ConservativeTokenEstimator, DefaultContextCompactor, TokenEstimator};

/// Stable module key used by the generated composition table.
pub const __ALPHA_MODULE__: &str = "compaction";

/// Compaction thresholds and budgets.
#[derive(Clone, Debug, Deserialize)]
pub struct CompactionConfig {
    /// Estimated context window in tokens. Zero disables compaction.
    #[serde(default = "default_context_window_tokens")]
    pub context_window_tokens: usize,
    /// Percentage of the context window that triggers compaction.
    #[serde(default = "default_threshold_percent")]
    pub threshold_percent: u8,
    /// Percentage of the context window kept as recent raw history.
    #[serde(default = "default_keep_recent_percent")]
    pub keep_recent_percent: u8,
    /// Maximum estimated tokens sent in one summarization call.
    #[serde(default = "default_max_summary_input_tokens")]
    pub max_summary_input_tokens: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            context_window_tokens: default_context_window_tokens(),
            threshold_percent: default_threshold_percent(),
            keep_recent_percent: default_keep_recent_percent(),
            max_summary_input_tokens: default_max_summary_input_tokens(),
        }
    }
}

fn default_context_window_tokens() -> usize {
    64 * 1024
}

fn default_threshold_percent() -> u8 {
    70
}

fn default_keep_recent_percent() -> u8 {
    30
}

fn default_max_summary_input_tokens() -> usize {
    32 * 1024
}

/// Registers automatic context compaction.
pub struct CompactionModule;

impl Module for CompactionModule {
    type Requirements = ();
    type Config = CompactionConfig;

    const KEY: &'static str = "compaction";
    const CAPABILITIES: &'static [CapabilitySpec] = &[
        CapabilitySpec::singleton("context.compaction", CapabilityScope::Location),
        CapabilitySpec::singleton("context.token_estimator", CapabilityScope::Location),
    ];

    fn activate(&self, ctx: &Ctx, config: &Self::Config) -> Result<(), ActivateError> {
        validate(config).map_err(|error| ActivateError::message(Self::KEY, error))?;
        let estimator = match ctx.get::<dyn TokenEstimator>() {
            Some(estimator) => estimator,
            None => {
                let estimator: Arc<dyn TokenEstimator> = Arc::new(ConservativeTokenEstimator);
                ctx.provide::<dyn TokenEstimator>(estimator.clone())
                    .map_err(|error| ActivateError::new(Self::KEY, error))?;
                estimator
            }
        };
        let compactor: Arc<dyn ContextPreparer> =
            Arc::new(DefaultContextCompactor::new(config.clone(), estimator));
        ctx.provide::<dyn ContextPreparer>(compactor)
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        Ok(())
    }
}

fn validate(config: &CompactionConfig) -> Result<(), &'static str> {
    if config.context_window_tokens == 0 {
        return Ok(());
    }
    if !(1..=100).contains(&config.threshold_percent) {
        return Err("threshold_percent must be between 1 and 100");
    }
    if config.keep_recent_percent >= config.threshold_percent {
        return Err("keep_recent_percent must be lower than threshold_percent");
    }
    if config.max_summary_input_tokens == 0 {
        return Err("max_summary_input_tokens must be greater than zero");
    }
    Ok(())
}
