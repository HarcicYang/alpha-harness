//! Process lifecycle management for the alpha front/core split.

#![forbid(unsafe_code)]

use std::fmt;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use alpha_front_client::{ClientError, CoreInfo, ProtocolClient};
use serde_json::json;

/// Core startup and retry configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SupervisorConfig {
    /// Additional attempts after the initial attempt.
    pub retries: usize,
    /// Grace period before a core is forcibly terminated.
    pub shutdown_grace: Duration,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            retries: 3,
            shutdown_grace: Duration::from_secs(2),
        }
    }
}

/// Failure while starting or stopping a core process.
#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    /// Process IO failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The protocol handshake failed.
    #[error(transparent)]
    Client(#[from] ClientError),
    /// The core executable could not be determined.
    #[error("current executable path is unavailable")]
    MissingExecutable,
    /// Every startup attempt failed.
    #[error("core startup failed after {attempts} attempts: {last_error}")]
    Startup {
        /// Total number of attempts.
        attempts: usize,
        /// Final failure.
        last_error: String,
    },
}

/// A managed core process and its alpha-protocol connection.
pub struct CoreSupervisor {
    child: Child,
    client: ProtocolClient<BufReader<ChildStdout>, ChildStdin>,
    info: CoreInfo,
    shutdown_grace: Duration,
}

impl fmt::Debug for CoreSupervisor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CoreSupervisor")
            .field("pid", &self.child.id())
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

impl CoreSupervisor {
    /// Start a core using the current executable and retry policy.
    pub fn start(config: SupervisorConfig) -> Result<Self, SupervisorError> {
        let executable = std::env::current_exe().map_err(|_| SupervisorError::MissingExecutable)?;
        Self::start_with_executable(executable, config)
    }

    /// Start a core from an explicit executable.
    pub fn start_with_executable(
        executable: impl AsRef<Path>,
        config: SupervisorConfig,
    ) -> Result<Self, SupervisorError> {
        let attempts = config.retries.saturating_add(1);
        let mut last_error = String::new();
        for _ in 0..attempts {
            match Self::start_once(executable.as_ref(), config.shutdown_grace) {
                Ok(supervisor) => return Ok(supervisor),
                Err(error) => last_error = error.to_string(),
            }
        }
        Err(SupervisorError::Startup {
            attempts,
            last_error,
        })
    }

    /// Information returned by the core handshake.
    pub fn info(&self) -> &CoreInfo {
        &self.info
    }

    /// The managed child process id.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Send a request to the managed core.
    pub fn request(
        &mut self,
        method: impl Into<String>,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, SupervisorError> {
        Ok(self.client.request(method, params)?)
    }

    /// Gracefully stop the core and reap the child process.
    pub fn shutdown(mut self) -> Result<(), SupervisorError> {
        let _ = self.client.request("body.shutdown", None);
        self.client.close_writer();
        self.wait_for_exit()?;
        Ok(())
    }

    fn start_once(executable: &Path, shutdown_grace: Duration) -> Result<Self, SupervisorError> {
        let mut child = Command::new(executable)
            .arg("__core")
            .arg("--stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("core stdin was not piped"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("core stdout was not piped"))?;
        let mut client = ProtocolClient::new(BufReader::new(stdout), stdin);
        let hello = client.request(
            "body.hello",
            Some(json!({
                "protocol": "alpha-protocol/1",
                "frontInstanceId": format!("front-{}", std::process::id()),
            })),
        )?;
        let info = CoreInfo::from_value(hello)?;
        Ok(Self {
            child,
            client,
            info,
            shutdown_grace,
        })
    }

    fn wait_for_exit(&mut self) -> Result<(), SupervisorError> {
        let deadline = Instant::now() + self.shutdown_grace;
        loop {
            if self.child.try_wait()?.is_some() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                self.child.kill()?;
                self.child.wait()?;
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for CoreSupervisor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Current executable path for tests and diagnostics.
pub fn executable_path() -> Option<PathBuf> {
    std::env::current_exe().ok()
}
