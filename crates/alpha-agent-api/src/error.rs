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
    /// The turn exceeded the model step limit.
    #[error("turn exceeded {0} model steps")]
    StepLimit(usize),
}

/// Failure from an optional context-preparation step.
#[derive(Debug, Error)]
#[error("context preparation failed: {0}")]
pub struct ContextError(String);

impl ContextError {
    /// Create an error from a plain message.
    pub fn message(message: impl Into<String>) -> Self {
        Self(message.into())
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
