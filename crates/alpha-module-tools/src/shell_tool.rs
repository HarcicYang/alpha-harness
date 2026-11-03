//! Shell execution tool with per-location shell discovery.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alpha_module_process::{
    ProcessAccess, ProcessError, ProcessOutputCallback, ProcessRequest, ProcessRunner,
    ProcessRunnerSet,
};
use alpha_permission_api::{PermissionOutcome, PermissionRequest, PermissionService};
use alpha_schema::PermissionMode;
use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Notify;

use crate::contract::{Tool, ToolContext, ToolError, ToolResponse, truncate_head_tail};

const MAX_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Invocation {
    Posix,
    PowerShell,
    Cmd,
}

/// One supported shell kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShellKind {
    /// Stable shell id, for example `bash` or `pwsh`.
    pub id: &'static str,
    /// Executable name.
    pub program: &'static str,
    invocation: Invocation,
}

impl ShellKind {
    /// Built-in shell kinds, in preference order.
    pub fn builtin() -> Vec<Self> {
        vec![
            Self {
                id: "bash",
                program: "bash",
                invocation: Invocation::Posix,
            },
            Self {
                id: "zsh",
                program: "zsh",
                invocation: Invocation::Posix,
            },
            Self {
                id: "fish",
                program: "fish",
                invocation: Invocation::Posix,
            },
            Self {
                id: "sh",
                program: "sh",
                invocation: Invocation::Posix,
            },
            Self {
                id: "pwsh",
                program: "pwsh",
                invocation: Invocation::PowerShell,
            },
            Self {
                id: "powershell",
                program: "powershell",
                invocation: Invocation::PowerShell,
            },
            Self {
                id: "cmd",
                program: "cmd",
                invocation: Invocation::Cmd,
            },
        ]
    }

    fn command_args(&self, command: &str) -> Vec<String> {
        match self.invocation {
            Invocation::Posix => vec!["-c".to_owned(), command.to_owned()],
            Invocation::PowerShell => {
                vec![
                    "-NoLogo".to_owned(),
                    "-Command".to_owned(),
                    command.to_owned(),
                ]
            }
            Invocation::Cmd => vec!["/C".to_owned(), command.to_owned()],
        }
    }

    fn probe_args(&self) -> Vec<String> {
        self.command_args("exit 0")
    }
}

/// Probe the execution world for available shells.
pub async fn discover_shells(runner: &Arc<dyn ProcessRunner>) -> Vec<ShellKind> {
    let mut available = Vec::new();
    for kind in ShellKind::builtin() {
        let request = ProcessRequest::new(kind.program, kind.probe_args())
            .with_timeout(Duration::from_secs(3));
        if let Ok(outcome) = runner.run(request).await
            && outcome.success
        {
            available.push(kind);
        }
    }
    available
}

/// Runs commands through a selected shell.
pub struct ShellTool {
    runners: Arc<ProcessRunnerSet>,
    permission: Arc<dyn PermissionService>,
    default_shell: Option<String>,
    available: Mutex<Option<Vec<ShellKind>>>,
    jobs: Mutex<BTreeMap<String, Arc<ShellJob>>>,
}

impl ShellTool {
    /// Create a shell tool. Shells are probed on first use.
    pub fn new(
        runners: Arc<ProcessRunnerSet>,
        permission: Arc<dyn PermissionService>,
        default_shell: Option<String>,
    ) -> Self {
        Self {
            runners,
            permission,
            default_shell,
            available: Mutex::new(None),
            jobs: Mutex::new(BTreeMap::new()),
        }
    }

    fn runner(&self) -> Result<Arc<dyn ProcessRunner>, ToolError> {
        let access = if self.permission.mode() == PermissionMode::Free {
            ProcessAccess::FullAccess
        } else {
            ProcessAccess::Sandboxed
        };
        self.runners
            .get(access)
            .map_err(|error| ToolError::operational(error.to_string()))
    }

    async fn available_shells(&self) -> Result<Vec<ShellKind>, ToolError> {
        if let Some(shells) = self.available.lock().expect("shell lock poisoned").clone() {
            return Ok(shells);
        }
        let runner = self.runner()?;
        let shells = discover_shells(&runner).await;
        *self.available.lock().expect("shell lock poisoned") = Some(shells.clone());
        Ok(shells)
    }
}

#[derive(Debug, Deserialize)]
struct ShellInput {
    #[serde(default)]
    shell: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    job_id: Option<String>,
    #[serde(default)]
    wait_ms: Option<u64>,
    #[serde(default)]
    since_bytes: Option<usize>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Default)]
struct ShellJob {
    output: Mutex<ShellJobOutput>,
    notify: Notify,
}

#[derive(Default)]
struct ShellJobOutput {
    bytes: Vec<u8>,
    status: ShellJobStatus,
}

#[derive(Default)]
enum ShellJobStatus {
    #[default]
    Running,
    Completed {
        code: Option<i32>,
        success: bool,
    },
    Failed(String),
}

impl ShellJob {
    fn is_running(&self) -> bool {
        matches!(
            self.output.lock().expect("shell job lock poisoned").status,
            ShellJobStatus::Running
        )
    }
}

impl Tool for ShellTool {
    fn name(&self) -> &'static str {
        "shell"
    }

    fn description(&self) -> String {
        let available = self.available.lock().expect("shell lock poisoned").clone();
        match available {
            Some(shells) if !shells.is_empty() => format!(
                "Run a command in a selected shell, or query a running job by `job_id`. \
                 Available shells: {}. The `wait_ms` field controls only this call's wait; \
                 it never kills the process and has no maximum.",
                shells
                    .iter()
                    .map(|shell| shell.id)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Some(_) => {
                "Run a command in a selected shell. No supported shells are available.".to_owned()
            }
            _ => "Run a command in a selected shell, or query a running job by `job_id`. \
                  The `wait_ms` field controls only this call's wait. Shells are probed on first use."
                .to_owned(),
        }
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "shell": { "type": "string" },
                "command": { "type": "string", "description": "Command to start." },
                "job_id": { "type": "string", "description": "Previously returned background job id." },
                "wait_ms": { "type": "integer", "minimum": 0, "description": "How long this call waits for completion. Omit to wait until the process exits; 0 returns immediately." },
                "since_bytes": { "type": "integer", "minimum": 0, "description": "Byte offset for incremental output when querying a job." },
                "description": { "type": "string" }
            },
            "oneOf": [
                { "required": ["command"] },
                { "required": ["job_id"] }
            ]
        })
    }

    fn prepare(&self) -> BoxFuture<'_, Result<(), ToolError>> {
        Box::pin(async move {
            if self
                .available
                .lock()
                .expect("shell lock poisoned")
                .is_some()
            {
                return Ok(());
            }
            if self.available_shells().await.is_err() {
                *self.available.lock().expect("shell lock poisoned") = Some(Vec::new());
            }
            Ok(())
        })
    }

    fn execute(
        &self,
        input: Value,
        context: ToolContext,
    ) -> BoxFuture<'_, Result<ToolResponse, ToolError>> {
        Box::pin(async move {
            let input: ShellInput = serde_json::from_value(input)
                .map_err(|error| ToolError::failure(format!("invalid input: {error}")))?;
            if let Some(job_id) = input.job_id.as_deref() {
                return self.query_job(job_id, &input, &context).await;
            }
            let command = input
                .command
                .as_deref()
                .filter(|command| !command.trim().is_empty())
                .ok_or_else(|| ToolError::failure("command or job_id is required"))?;
            let runner = self.runner()?;
            let sandboxed = self.permission.mode() != PermissionMode::Free;
            let available = self.available_shells().await?;
            if available.is_empty() {
                return Err(ToolError::failure(
                    "no supported shells were found in this workspace",
                ));
            }

            let chosen = input
                .shell
                .clone()
                .or_else(|| self.default_shell.clone())
                .unwrap_or_else(|| available[0].id.to_owned());
            let Some(kind) = available.iter().find(|kind| kind.id == chosen) else {
                let list = available
                    .iter()
                    .map(|kind| kind.id)
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(ToolError::failure(format!(
                    "shell `{chosen}` is not available; available shells: {list}"
                )));
            };

            let request = PermissionRequest {
                action: "shell.exec".to_owned(),
                resources: vec![command.to_owned()],
                reason: input
                    .description
                    .clone()
                    .or_else(|| Some(format!("run command in {}", kind.id))),
                sandboxed,
            };
            match self.permission.check(request).await {
                PermissionOutcome::Allowed { .. } => {}
                PermissionOutcome::Denied { reason } => {
                    return Err(ToolError::failure(format!("permission denied: {reason}")));
                }
            }

            let process_request = ProcessRequest::new(kind.program, kind.command_args(command));
            let started = Instant::now();
            let job_id = format!("shell-{}", uuid::Uuid::now_v7());
            let job = Arc::new(ShellJob::default());
            self.jobs
                .lock()
                .expect("shell jobs lock poisoned")
                .insert(job_id.clone(), job.clone());

            let output_job = job.clone();
            let output_context = context.clone();
            let output: ProcessOutputCallback = Arc::new(move |_, bytes| {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                output_job
                    .output
                    .lock()
                    .expect("shell job lock poisoned")
                    .bytes
                    .extend_from_slice(&bytes);
                output_context.emit_output(text);
            });
            let background_job = job.clone();
            tokio::spawn(async move {
                let result = runner.run_streaming(process_request, output).await;
                let mut output = background_job
                    .output
                    .lock()
                    .expect("shell job lock poisoned");
                output.status = match result {
                    Ok(outcome) => ShellJobStatus::Completed {
                        code: outcome.code,
                        success: outcome.success,
                    },
                    Err(ProcessError::Timeout(timeout)) => {
                        ShellJobStatus::Failed(format!("process timed out after {timeout:?}"))
                    }
                    Err(error) => ShellJobStatus::Failed(error.to_string()),
                };
                drop(output);
                background_job.notify.notify_waiters();
            });

            wait_for_job(&job, input.wait_ms, &context).await?;
            job_response(
                &job_id,
                &job,
                input.since_bytes.unwrap_or(0),
                started,
                Some(kind.id),
                true,
            )
        })
    }
}

impl ShellTool {
    async fn query_job(
        &self,
        job_id: &str,
        input: &ShellInput,
        context: &ToolContext,
    ) -> Result<ToolResponse, ToolError> {
        let job = self
            .jobs
            .lock()
            .expect("shell jobs lock poisoned")
            .get(job_id)
            .cloned()
            .ok_or_else(|| ToolError::failure(format!("unknown shell job `{job_id}`")))?;
        let since = input.since_bytes.unwrap_or(0);
        let before = job
            .output
            .lock()
            .expect("shell job lock poisoned")
            .bytes
            .len();
        wait_for_job(&job, input.wait_ms, context).await?;
        let after = job
            .output
            .lock()
            .expect("shell job lock poisoned")
            .bytes
            .len();
        if after > before {
            let output = job.output.lock().expect("shell job lock poisoned");
            context.emit_output(String::from_utf8_lossy(&output.bytes[before..after]).into_owned());
        }
        job_response(
            job_id,
            &job,
            since,
            Instant::now(),
            input.shell.as_deref(),
            false,
        )
    }
}

async fn wait_for_job(
    job: &Arc<ShellJob>,
    wait_ms: Option<u64>,
    context: &ToolContext,
) -> Result<(), ToolError> {
    if !job.is_running() {
        return Ok(());
    }
    let wait = async {
        let notified = job.notify.notified();
        tokio::pin!(notified);
        if !job.is_running() {
            return;
        }
        tokio::select! {
            _ = context.cancellation.cancelled() => {}
            _ = &mut notified => {}
        }
    };
    match wait_ms {
        None => wait.await,
        Some(0) => return Ok(()),
        Some(wait_ms) => {
            let _ = tokio::time::timeout(Duration::from_millis(wait_ms), wait).await;
        }
    }
    if context.cancellation.is_cancelled() {
        return Err(ToolError::operational("shell call cancelled"));
    }
    Ok(())
}

fn job_response(
    job_id: &str,
    job: &ShellJob,
    since_bytes: usize,
    started: Instant,
    shell: Option<&str>,
    initial: bool,
) -> Result<ToolResponse, ToolError> {
    let output = job.output.lock().expect("shell job lock poisoned");
    let since = floor_char_boundary_bytes(&output.bytes, since_bytes.min(output.bytes.len()));
    let next_offset = output.bytes.len();
    let raw = String::from_utf8_lossy(&output.bytes[since..]).into_owned();
    let (body, truncated) = truncate_head_tail(&raw, MAX_OUTPUT_BYTES);
    let body = if body.is_empty() {
        "(no output yet)".to_owned()
    } else {
        body
    };
    let (status, exit_code, failed) = match &output.status {
        ShellJobStatus::Running => ("running", None, false),
        ShellJobStatus::Completed { code, success } => (
            if *success { "completed" } else { "failed" },
            *code,
            !success,
        ),
        ShellJobStatus::Failed(_) => ("failed", None, true),
    };
    let content = format!(
        "status: {status}\nexit_code: {}\njob_id: {job_id}\nnext_offset: {next_offset}\n{body}",
        exit_code.map_or_else(|| "none".to_owned(), |code| code.to_string())
    );
    let metadata = json!({
        "job_id": job_id,
        "status": status,
        "exit_code": exit_code,
        "next_offset": next_offset,
        "truncated": truncated,
        "initial": initial,
        "shell": shell,
        "duration_ms": started.elapsed().as_millis() as u64,
    });
    if failed {
        return Err(ToolError::failure(content));
    }
    match &output.status {
        ShellJobStatus::Failed(error) => Err(ToolError::failure(format!("{content}\n{error}"))),
        _ => Ok(ToolResponse::text(content).with_output(metadata)),
    }
}

fn floor_char_boundary_bytes(text: &[u8], mut index: usize) -> usize {
    let Ok(text) = std::str::from_utf8(text) else {
        return index;
    };
    let _ = text;
    while index > 0 && (text.as_bytes()[index] & 0b1100_0000) == 0b1000_0000 {
        index -= 1;
    }
    index
}
