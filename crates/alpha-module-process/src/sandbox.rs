//! Linux bubblewrap sandbox driver for the process seam.
//!
//! The wrapped command runs inside mount and namespace isolation: the
//! workspace is writable, system paths and the home directory are read-only,
//! `/tmp` is a private tmpfs, and the network is unshared by default.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use futures::future::BoxFuture;

use crate::seam::{ProcessError, ProcessOutcome, ProcessRequest, ProcessRunner};

/// Sandbox configuration.
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct SandboxConfig {
    /// `None` probes at activation, `Some(true)` requires the sandbox, and
    /// `Some(false)` disables it.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Allow network access inside the sandbox.
    #[serde(default)]
    pub network: bool,
    /// Extra read-only bind mounts.
    #[serde(default)]
    pub read_only: Vec<PathBuf>,
    /// Extra writable bind mounts.
    #[serde(default)]
    pub writable: Vec<PathBuf>,
}

/// Finds `bwrap` on `PATH`.
pub fn find_bwrap() -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|directory| directory.join("bwrap"))
        .find(|candidate| candidate.is_file())
}

/// Whether bubblewrap can actually create a sandbox here.
pub fn available_bwrap() -> Option<PathBuf> {
    let bwrap = find_bwrap()?;
    let probe = Command::new(&bwrap)
        .args([
            "--die-with-parent",
            "--unshare-net",
            "--ro-bind-try",
            "/usr",
            "/usr",
            "--ro-bind-try",
            "/bin",
            "/bin",
            "--ro-bind-try",
            "/lib",
            "/lib",
            "--ro-bind-try",
            "/lib64",
            "/lib64",
            "--ro-bind-try",
            "/etc",
            "/etc",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "--chdir",
            "/",
            "sh",
            "-c",
            "exit 0",
        ])
        .output()
        .ok()?;
    probe.status.success().then_some(bwrap)
}

/// Process runner that wraps commands in bubblewrap.
pub struct SandboxedProcessRunner {
    inner: Arc<dyn ProcessRunner>,
    workspace: PathBuf,
    config: SandboxConfig,
    bwrap: PathBuf,
}

impl SandboxedProcessRunner {
    /// Create a sandboxed runner.
    pub fn new(
        inner: Arc<dyn ProcessRunner>,
        workspace: impl Into<PathBuf>,
        config: SandboxConfig,
        bwrap: impl Into<PathBuf>,
    ) -> Self {
        Self {
            inner,
            workspace: workspace.into(),
            config,
            bwrap: bwrap.into(),
        }
    }
}

impl ProcessRunner for SandboxedProcessRunner {
    fn run(&self, request: ProcessRequest) -> BoxFuture<'_, Result<ProcessOutcome, ProcessError>> {
        Box::pin(async move {
            let args = bwrap_args(&self.workspace, &self.config, &request);
            let sandboxed = ProcessRequest {
                program: self.bwrap.display().to_string(),
                args,
                env: request.env,
                timeout: request.timeout,
            };
            self.inner.run(sandboxed).await
        })
    }
}

/// Build the bubblewrap argument list for one request.
pub fn bwrap_args(
    workspace: &Path,
    config: &SandboxConfig,
    request: &ProcessRequest,
) -> Vec<String> {
    let mut args = Vec::new();
    push(&mut args, "--die-with-parent");
    push(&mut args, "--new-session");
    push(&mut args, "--unshare-ipc");
    push(&mut args, "--unshare-uts");
    push(&mut args, "--unshare-pid");
    if !config.network {
        push(&mut args, "--unshare-net");
    }

    for system in ["/usr", "/bin", "/lib", "/lib64", "/etc", "/opt"] {
        push(&mut args, "--ro-bind-try");
        push(&mut args, system);
        push(&mut args, system);
    }
    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home).display().to_string();
        push(&mut args, "--ro-bind-try");
        push(&mut args, &home);
        push(&mut args, &home);
    }

    push(&mut args, "--proc");
    push(&mut args, "/proc");
    push(&mut args, "--dev");
    push(&mut args, "/dev");
    push(&mut args, "--tmpfs");
    push(&mut args, "/tmp");

    let workspace_path = workspace;
    let workspace = workspace_path.display().to_string();
    push(&mut args, "--bind");
    push(&mut args, &workspace);
    push(&mut args, &workspace);
    for protected in [".git", ".agents", ".codex", ".aws"] {
        let path = workspace_path.join(protected);
        let path = path.display().to_string();
        push(&mut args, "--ro-bind-try");
        push(&mut args, &path);
        push(&mut args, &path);
    }
    for path in &config.read_only {
        let path = path.display().to_string();
        push(&mut args, "--ro-bind-try");
        push(&mut args, &path);
        push(&mut args, &path);
    }
    for path in &config.writable {
        let path = path.display().to_string();
        push(&mut args, "--bind-try");
        push(&mut args, &path);
        push(&mut args, &path);
    }
    push(&mut args, "--chdir");
    push(&mut args, &workspace);

    push(&mut args, "--");
    push(&mut args, &request.program);
    for argument in &request.args {
        push(&mut args, argument);
    }
    args
}

fn push(args: &mut Vec<String>, value: &str) {
    args.push(value.to_owned());
}
