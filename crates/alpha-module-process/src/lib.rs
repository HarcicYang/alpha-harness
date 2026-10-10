//! Process execution seam and local driver.
//!
//! The driver is constructed with a fixed cwd from the Location. Requests
//! cannot override it.

#![forbid(unsafe_code)]

/// Stable module key used by the generated composition table.
pub const __ALPHA_MODULE__: &str = "process.local";

pub mod local;
pub mod sandbox;
pub mod seam;

use std::path::PathBuf;
use std::sync::Arc;

use alpha_kernel::{ActivateError, CapabilityScope, CapabilitySpec, Ctx, Module};
use serde::Deserialize;

pub use local::LocalProcessRunner;
pub use sandbox::{SandboxConfig, SandboxedProcessRunner, available_bwrap, bwrap_args, find_bwrap};
pub use seam::{ProcessError, ProcessOutcome, ProcessRequest, ProcessRunner};

/// Configuration for the local process driver.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ProcessConfig {
    /// Fixed working directory taken from the Location.
    pub cwd: PathBuf,
    /// Sandbox settings.
    #[serde(default)]
    pub sandbox: SandboxConfig,
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
        let local: Arc<dyn ProcessRunner> = Arc::new(LocalProcessRunner::new(config.cwd.clone()));
        let runner = select_runner(local, config)?;
        ctx.provide::<dyn ProcessRunner>(runner)
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        Ok(())
    }
}

fn select_runner(
    local: Arc<dyn ProcessRunner>,
    config: &ProcessConfig,
) -> Result<Arc<dyn ProcessRunner>, ActivateError> {
    let sandbox = &config.sandbox;
    let sandboxed = |bwrap: PathBuf| -> Arc<dyn ProcessRunner> {
        Arc::new(SandboxedProcessRunner::new(
            local.clone(),
            config.cwd.clone(),
            sandbox.clone(),
            bwrap,
        ))
    };
    match sandbox.enabled {
        Some(false) => Ok(local),
        Some(true) => match sandbox::available_bwrap() {
            Some(bwrap) => Ok(sandboxed(bwrap)),
            None => Err(ActivateError::message(
                ProcessLocalModule::KEY,
                "sandbox.enabled = true but bubblewrap is unavailable; \
                 install bubblewrap or set sandbox.enabled = false",
            )),
        },
        None => match sandbox::available_bwrap() {
            Some(bwrap) => Ok(sandboxed(bwrap)),
            None => {
                eprintln!("alpha: bubblewrap sandbox is unavailable; running commands unsandboxed");
                Ok(local)
            }
        },
    }
}
