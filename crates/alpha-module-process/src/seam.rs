//! Process execution seam types.

use std::collections::BTreeMap;
use std::time::Duration;

use futures::future::BoxFuture;

/// One process request.
///
/// The working directory comes from the driver's Location and cannot be
/// overridden here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessRequest {
    /// Program to execute.
    pub program: String,
    /// Program arguments.
    pub args: Vec<String>,
    /// Additional environment variables.
    pub env: BTreeMap<String, String>,
    /// Optional wall-clock timeout.
    pub timeout: Option<Duration>,
}

impl ProcessRequest {
    /// Build a request for a program and arguments.
    pub fn new(program: impl Into<String>, args: impl IntoIterator<Item = String>) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().collect(),
            env: BTreeMap::new(),
            timeout: None,
        }
    }

    /// Set the wall-clock timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Add an environment variable.
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }
}

/// Result of one process run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessOutcome {
    /// Exit code when the process exited normally.
    pub code: Option<i32>,
    /// Whether the process exited successfully.
    pub success: bool,
    /// Captured stdout.
    pub stdout: Vec<u8>,
    /// Captured stderr.
    pub stderr: Vec<u8>,
}

/// Process errors.
#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    /// The process could not be spawned.
    #[error("failed to spawn process: {0}")]
    Spawn(std::io::Error),
    /// IO failure while running the process.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The process exceeded its timeout and was killed.
    #[error("process timed out after {0:?}")]
    Timeout(Duration),
}

/// Process execution contract.
pub trait ProcessRunner: Send + Sync + 'static {
    /// Run one process to completion.
    fn run(&self, request: ProcessRequest) -> BoxFuture<'_, Result<ProcessOutcome, ProcessError>>;
}
