//! Local machine driver for the process seam.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use futures::future::BoxFuture;

use crate::seam::{ProcessError, ProcessOutcome, ProcessRequest, ProcessRunner};

/// Process seam backed by the local machine.
#[derive(Clone, Debug)]
pub struct LocalProcessRunner {
    cwd: PathBuf,
}

impl LocalProcessRunner {
    /// Create a runner whose commands start in `cwd`.
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self { cwd: cwd.into() }
    }

    /// The fixed working directory.
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }
}

impl ProcessRunner for LocalProcessRunner {
    fn run(&self, request: ProcessRequest) -> BoxFuture<'_, Result<ProcessOutcome, ProcessError>> {
        Box::pin(async move {
            let mut command = tokio::process::Command::new(&request.program);
            command
                .args(&request.args)
                .current_dir(&self.cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            for (key, value) in &request.env {
                command.env(key, value);
            }

            let child = command.spawn().map_err(ProcessError::Spawn)?;
            let output = match request.timeout {
                Some(timeout) => {
                    match tokio::time::timeout(timeout, child.wait_with_output()).await {
                        Ok(result) => result?,
                        Err(_) => return Err(ProcessError::Timeout(timeout)),
                    }
                }
                None => child.wait_with_output().await?,
            };

            Ok(ProcessOutcome {
                code: output.status.code(),
                success: output.status.success(),
                stdout: output.stdout,
                stderr: output.stderr,
            })
        })
    }
}
