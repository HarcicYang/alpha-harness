//! Local machine driver for the process seam.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use futures::future::BoxFuture;
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::seam::{
    ProcessError, ProcessOutcome, ProcessOutputCallback, ProcessRequest, ProcessRunner,
    ProcessStream,
};

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
            for (key, value) in &request.env {
                command.env(key, value);
            }

            let mut child = command.spawn().map_err(ProcessError::Spawn)?;
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

            Ok(ProcessOutcome {
                code: status.code(),
                success: status.success(),
                stdout,
                stderr,
            })
        })
    }
}
