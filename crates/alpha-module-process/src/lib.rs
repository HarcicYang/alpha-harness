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

/// One process execution boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessAccess {
    /// Run inside the platform sandbox.
    Sandboxed,
    /// Run with the local user's full process and network access.
    FullAccess,
}

/// Process runners available to a Location.
#[derive(Clone)]
pub struct ProcessRunnerSet {
    full_access: Arc<dyn ProcessRunner>,
    sandboxed: Option<Arc<dyn ProcessRunner>>,
}

impl ProcessRunnerSet {
    /// Build a runner set.
    pub fn new(
        full_access: Arc<dyn ProcessRunner>,
        sandboxed: Option<Arc<dyn ProcessRunner>>,
    ) -> Self {
        Self {
            full_access,
            sandboxed,
        }
    }

    /// Whether the sandbox runner is available.
    pub fn sandbox_available(&self) -> bool {
        self.sandboxed.is_some()
    }

    /// Resolve one execution boundary.
    pub fn get(&self, access: ProcessAccess) -> Result<Arc<dyn ProcessRunner>, ProcessError> {
        match access {
            ProcessAccess::Sandboxed => self
                .sandboxed
                .clone()
                .ok_or(ProcessError::SandboxUnavailable),
            ProcessAccess::FullAccess => Ok(self.full_access.clone()),
        }
    }
}

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
        let sandboxed = select_sandbox(local.clone(), config)?;
        let default = sandboxed.clone().unwrap_or_else(|| local.clone());
        ctx.provide::<ProcessRunnerSet>(Arc::new(ProcessRunnerSet::new(local, sandboxed)))
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        ctx.provide::<dyn ProcessRunner>(default)
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        Ok(())
    }
}

fn select_sandbox(
    local: Arc<dyn ProcessRunner>,
    config: &ProcessConfig,
) -> Result<Option<Arc<dyn ProcessRunner>>, ActivateError> {
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
        Some(false) => Ok(None),
        Some(true) => match sandbox::available_bwrap() {
            Some(bwrap) => Ok(Some(sandboxed(bwrap))),
            None => Err(ActivateError::message(
                ProcessLocalModule::KEY,
                "sandbox.enabled = true but bubblewrap is unavailable; \
                 install bubblewrap or set sandbox.enabled = false",
            )),
        },
        None => match sandbox::available_bwrap() {
            Some(bwrap) => Ok(Some(sandboxed(bwrap))),
            None => {
                eprintln!("alpha: bubblewrap sandbox is unavailable; sandboxed commands disabled");
                Ok(None)
            }
        },
    }
}
