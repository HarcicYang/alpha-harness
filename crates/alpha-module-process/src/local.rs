//! Local machine driver for the process seam.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use futures::future::BoxFuture;
use tokio::io::{AsyncRead, AsyncReadExt};

#[cfg(unix)]
use nix::sys::signal::{Signal, killpg};
#[cfg(unix)]
use nix::unistd::Pid;

use crate::seam::{
    ProcessError, ProcessOutcome, ProcessOutputCallback, ProcessRequest, ProcessRunner,
    ProcessStream,
};

/// Kills the child's process group if the run is dropped before it exits.
#[cfg(unix)]
struct ProcessGroup {
    pid: Option<u32>,
}

#[cfg(unix)]
impl ProcessGroup {
    fn new(pid: u32) -> Self {
        Self { pid: Some(pid) }
    }

    fn disarm(&mut self) {
        self.pid = None;
    }
}

#[cfg(unix)]
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        if let Some(pid) = self.pid {
            let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
        }
    }
}

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

    async fn read_stream<R>(
        mut reader: Option<R>,
        stream: ProcessStream,
        output: ProcessOutputCallback,
    ) -> std::io::Result<Vec<u8>>
    where
        R: AsyncRead + Unpin,
    {
        let Some(reader) = reader.as_mut() else {
            return Ok(Vec::new());
        };
        let mut collected = Vec::new();
        let mut buffer = vec![0u8; 16 * 1024];
        loop {
            let read = reader.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            let chunk = buffer[..read].to_vec();
            collected.extend_from_slice(&chunk);
            output(stream, chunk);
        }
        Ok(collected)
    }
}

impl ProcessRunner for LocalProcessRunner {
    fn run(&self, request: ProcessRequest) -> BoxFuture<'_, Result<ProcessOutcome, ProcessError>> {
        self.run_streaming(request, Arc::new(|_, _| {}))
    }

    fn run_streaming(
        &self,
        request: ProcessRequest,
        output: ProcessOutputCallback,
    ) -> BoxFuture<'_, Result<ProcessOutcome, ProcessError>> {
        Box::pin(async move {
            let mut command = tokio::process::Command::new(&request.program);
            command
                .args(&request.args)
                .current_dir(&self.cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            #[cfg(unix)]
            command.process_group(0);
            for (key, value) in &request.env {
                command.env(key, value);
            }

            let mut child = command.spawn().map_err(ProcessError::Spawn)?;
            // The shell spawns its own children. Killing only the direct child
            // on interruption leaves those descendants running, so own the
            // whole process group and kill it if this future is dropped.
            #[cfg(unix)]
            let mut group = child.id().map(ProcessGroup::new);
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            let stdout_output = Self::read_stream(stdout, ProcessStream::Stdout, output.clone());
            let stderr_output = Self::read_stream(stderr, ProcessStream::Stderr, output);
            let wait = async {
                let (stdout, stderr, status) =
                    tokio::try_join!(stdout_output, stderr_output, child.wait())?;
                Ok::<_, std::io::Error>((stdout, stderr, status))
            };
            let (stdout, stderr, status) = match request.timeout {
                Some(timeout) => match tokio::time::timeout(timeout, wait).await {
                    Ok(result) => result?,
                    Err(_) => return Err(ProcessError::Timeout(timeout)),
                },
                None => wait.await?,
            };
            // The command exited; its process group is empty.
            #[cfg(unix)]
            if let Some(group) = group.as_mut() {
                group.disarm();
            }

            Ok(ProcessOutcome {
                code: status.code(),
                success: status.success(),
                stdout,
                stderr,
            })
        })
    }
}
