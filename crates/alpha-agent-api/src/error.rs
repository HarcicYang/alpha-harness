//! Agent turn failures and outcomes.

use alpha_schema::MessageId;
use alpha_session_log::SessionError;

use crate::provider::ProviderError;
use thiserror::Error;

/// Loop failure.
#[derive(Debug, Error)]
pub enum LoopError {
    /// Session storage failure.
    #[error(transparent)]
    Session(#[from] SessionError),
    /// Provider failure.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// Operational tool failure that ends the turn.
    #[error("tool failure: {0}")]
    Tool(String),
    /// Dynamic context injection failed.
    #[error(transparent)]
    Context(#[from] ContextError),
    /// The turn was cancelled before it could finish.
    #[error("turn interrupted")]
    Interrupted,
    /// The turn exceeded the model step limit.
    #[error("turn exceeded {0} model steps")]
    StepLimit(usize),
}

/// Failure from an optional context-preparation step.
#[derive(Debug, Error)]
#[error("context preparation failed: {message}")]
pub struct ContextError {
    message: String,
    overflow: bool,
}

impl ContextError {
    /// Create an error from a plain message.
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            overflow: false,
        }
    }

    /// Create an error for a context that no longer fits the model window.
    ///
    /// The session cannot continue: the history has to be replaced instead of
    /// being retried in place.
    pub fn overflow(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            overflow: true,
        }
    }

    /// Whether the session must continue in a fresh log.
    pub fn requires_new_session(&self) -> bool {
        self.overflow
    }
}

/// Outcome of one completed turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnOutcome {
    /// Identity of the assistant message.
    pub assistant_message: MessageId,
    /// Assistant text.
    pub assistant_text: String,
}
