//! Prompt assembly contract.
//!
//! The fixed shell asks for a system prompt through this trait only, so the
//! prompt module is a replaceable dependency rather than a fixed one.

#![forbid(unsafe_code)]

use alpha_kernel::Ctx;
use alpha_session_log::SessionState;
use thiserror::Error;

/// Prompt assembly contract consumed by the application runtime.
pub trait PromptAssembler: Send + Sync + 'static {
    /// Assemble a deterministic system prompt for one session.
    fn assemble(&self, ctx: &Ctx, state: &SessionState) -> Result<String, PromptError>;
}

/// Errors from prompt assembly.
#[derive(Debug, Error)]
pub enum PromptError {
    /// Two providers use the same stable key.
    #[error("prompt section `{0}` is already registered")]
    DuplicateSection(String),
    /// Kernel registration failed.
    #[error("prompt registration failed: {0}")]
    Kernel(String),
    /// The session cwd is outside its workspace root.
    #[error("cwd `{cwd}` is outside workspace root `{root}`")]
    InvalidWorkspace {
        /// Workspace root.
        root: String,
        /// Session cwd.
        cwd: String,
    },
    /// Reading a project instruction file failed.
    #[error("failed to read project instructions `{path}`: {message}")]
    ProjectInstruction {
        /// Workspace-relative path.
        path: String,
        /// Underlying error.
        message: String,
    },
}
