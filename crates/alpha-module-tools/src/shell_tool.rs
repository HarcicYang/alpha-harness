//! Shell execution tool with per-location shell discovery.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alpha_module_permission::{PermissionOutcome, PermissionRequest, PermissionService};
use alpha_module_process::{
    ProcessAccess, ProcessError, ProcessRequest, ProcessRunner, ProcessRunnerSet,
};
use alpha_schema::PermissionMode;
use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::contract::{Tool, ToolContext, ToolError, ToolResponse, truncate_head_tail};

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
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
        if let Ok(outcome) = runner.run(request).await {
            if outcome.success {
                available.push(kind);
            }
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
    command: String,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    description: Option<String>,
}

impl Tool for ShellTool {
    fn name(&self) -> &'static str {
        "shell"
    }

    fn description(&self) -> String {
        let available = self.available.lock().expect("shell lock poisoned").clone();
        match available {
            Some(shells) if !shells.is_empty() => format!(
                "Run a command in a selected shell. Available shells: {}.",
                shells
                    .iter()
                    .map(|shell| shell.id)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Some(_) => {
                "Run a command in a selected shell. No supported shells are available.".to_owned()
            }
            _ => "Run a command in a selected shell. Shells are probed on first use.".to_owned(),
        }
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "shell": { "type": "string" },
                "command": { "type": "string" },
                "timeout_ms": { "type": "integer" },
                "description": { "type": "string" }
            },
            "required": ["command"]
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
                resources: vec![input.command.clone()],
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

            let timeout = Duration::from_millis(
                input
                    .timeout_ms
                    .unwrap_or(DEFAULT_TIMEOUT_MS)
                    .min(MAX_TIMEOUT_MS),
            );
            let process_request =
                ProcessRequest::new(kind.program, kind.command_args(&input.command))
                    .with_timeout(timeout);
            let started = Instant::now();
            let outcome = tokio::select! {
                _ = context.cancellation.cancelled() => {
                    return Err(ToolError::operational("shell call cancelled"));
                }
                result = runner.run(process_request) => result,
            };

            match outcome {
                Ok(outcome) => {
                    let mut combined = String::from_utf8_lossy(&outcome.stdout).into_owned();
                    let stderr = String::from_utf8_lossy(&outcome.stderr);
                    if !stderr.is_empty() {
                        if !combined.is_empty() {
                            combined.push('\n');
                        }
                        combined.push_str(&stderr);
                    }
                    let (content, truncated) = truncate_head_tail(&combined, MAX_OUTPUT_BYTES);
                    let exit_code = outcome.code.unwrap_or(-1);
                    if !outcome.success {
                        return Err(ToolError::failure(format!(
                            "command exited with code {exit_code}\n{content}"
                        )));
                    }
                    let content = if content.is_empty() {
                        "(no output)".to_owned()
                    } else {
                        content
                    };
                    Ok(ToolResponse::text(content).with_output(json!({
                        "shell": kind.id,
                        "exit_code": exit_code,
                        "duration_ms": started.elapsed().as_millis() as u64,
                        "truncated": truncated,
                    })))
                }
                Err(ProcessError::Timeout(_)) => Err(ToolError::failure(format!(
                    "command timed out after {}ms",
                    timeout.as_millis()
                ))),
                Err(error) => Err(ToolError::failure(format!("failed to run shell: {error}"))),
            }
        })
    }
}
