//! Process execution seam and local driver.
//!
//! The driver is constructed with a fixed cwd from the Location. Requests
//! cannot override it.

#![forbid(unsafe_code)]

/// Stable module key used by the generated composition table.
pub const __ALPHA_MODULE__: &str = "process.local";

pub mod local;
pub mod seam;

use std::path::PathBuf;
use std::sync::Arc;

use alpha_kernel::{ActivateError, CapabilityScope, CapabilitySpec, Ctx, Module};
use serde::Deserialize;

pub use local::LocalProcessRunner;
pub use seam::{ProcessError, ProcessOutcome, ProcessRequest, ProcessRunner};

/// Configuration for the local process driver.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ProcessConfig {
    /// Fixed working directory taken from the Location.
    pub cwd: PathBuf,
}

/// Provides the process seam backed by the local machine.
pub struct ProcessLocalModule;

impl Module for ProcessLocalModule {
    type Requirements = ();
    type Config = ProcessConfig;

    const KEY: &'static str = "process.local";
    const CAPABILITIES: &'static [CapabilitySpec] = &[CapabilitySpec::singleton(
        "process",
        CapabilityScope::Location,
    )];

    fn activate(&self, ctx: &Ctx, config: &Self::Config) -> Result<(), ActivateError> {
        if config.cwd.as_os_str().is_empty() {
            return Err(ActivateError::message(
                Self::KEY,
                "process cwd is not configured",
            ));
        }
        let runner: Arc<dyn ProcessRunner> = Arc::new(LocalProcessRunner::new(config.cwd.clone()));
        ctx.provide::<dyn ProcessRunner>(runner)
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        Ok(())
    }
}
