//! Agent driver contract.

use alpha_kernel::Ctx;
use alpha_session_log::{SessionState, SessionWriter};
use futures::future::BoxFuture;

use crate::error::{ContextError, LoopError, TurnOutcome};
use crate::provider::ChatMessage;

/// Appends durable dynamic context messages when their rendered content changes.
pub trait ContextInjector: Send + Sync + 'static {
    /// Update the provider-facing history before one model request.
    fn inject<'a>(
        &'a self,
        ctx: &'a Ctx,
        writer: &'a SessionWriter,
        state: &'a mut SessionState,
        messages: &'a mut Vec<ChatMessage>,
    ) -> BoxFuture<'a, Result<(), ContextError>>;
}

/// Replaceable driver contract for one agent turn.
///
/// Consumers depend on this trait, so a composition can mount a different
/// driver under the same service key.
pub trait AgentDriver: Send + Sync + 'static {
    /// Run one turn and record it in the session log.
    fn run_turn<'a>(
        &'a self,
        ctx: &'a Ctx,
        writer: &'a SessionWriter,
        state: &'a SessionState,
        prompt: &'a str,
    ) -> BoxFuture<'a, Result<TurnOutcome, LoopError>>;
}
