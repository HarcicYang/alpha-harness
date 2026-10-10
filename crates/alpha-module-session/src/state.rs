//! Deterministic projections over the durable event log.

use std::collections::BTreeSet;

use alpha_schema::{EventEnvelope, MessageId, PermissionMode, Seq, ToolCallId};

use crate::event::{SessionEvent, ToolStatus};
use crate::header::{SessionHeader, WorkspaceRef, apply_location_event};

/// One message in the projected history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessageRecord {
    /// A user message.
    User {
        /// Message identity.
        id: MessageId,
        /// Message text.
        text: String,
    },
    /// An assistant message.
    Assistant {
        /// Message identity.
        id: MessageId,
        /// Message text.
        text: String,
    },
    /// A tool call requested by the assistant.
    ToolCall {
        /// Assistant message that produced the call.
        message_id: MessageId,
        /// Call identity.
        id: ToolCallId,
        /// Tool name.
        name: String,
        /// Decoded input.
        input: serde_json::Value,
    },
    /// A settled tool call.
    ToolResult {
        /// Call identity.
        id: ToolCallId,
        /// Outcome status.
        status: ToolStatus,
        /// Model-visible content.
        content: String,
    },
}

/// Projected session state, folded from durable events.
#[derive(Clone, Debug)]
pub struct SessionState {
    /// Effective permission mode.
    pub mode: PermissionMode,
    /// Effective workspace binding.
    pub workspace: WorkspaceRef,
    /// Message history.
    pub messages: Vec<MessageRecord>,
    /// Whether a turn is currently open.
    pub turn_open: bool,
    /// Tool calls that have not settled.
    pub pending_tool_calls: BTreeSet<ToolCallId>,
    /// Last applied sequence number.
    pub last_seq: Seq,
}

impl SessionState {
    /// Create an empty projection seeded from a session header.
    pub fn new(header: &SessionHeader) -> Self {
        Self {
            mode: header.mode,
            workspace: header.workspace.clone(),
            messages: Vec::new(),
            turn_open: false,
            pending_tool_calls: BTreeSet::new(),
            last_seq: Seq::new(0),
        }
    }

    /// Fold a list of durable events into a projection.
    pub fn from_events(header: &SessionHeader, events: &[EventEnvelope<SessionEvent>]) -> Self {
        let mut state = Self::new(header);
        for envelope in events {
            state.apply(envelope);
        }
        state
    }

    /// Apply one durable event.
    pub fn apply(&mut self, envelope: &EventEnvelope<SessionEvent>) {
        self.last_seq = envelope.seq;
        match &envelope.payload {
            SessionEvent::TurnStarted => self.turn_open = true,
            SessionEvent::TurnEnded { .. } => self.turn_open = false,
            SessionEvent::UserMessage { id, text } => self.messages.push(MessageRecord::User {
                id: *id,
                text: text.clone(),
            }),
            SessionEvent::AssistantMessage { id, text } => {
                self.messages.push(MessageRecord::Assistant {
                    id: *id,
                    text: text.clone(),
                });
            }
            SessionEvent::ToolCall {
                id,
                message_id,
                name,
                input,
            } => {
                self.pending_tool_calls.insert(*id);
                self.messages.push(MessageRecord::ToolCall {
                    message_id: *message_id,
                    id: *id,
                    name: name.clone(),
                    input: input.clone(),
                });
            }
            SessionEvent::ToolResult {
                id,
                status,
                content,
            } => {
                self.pending_tool_calls.remove(id);
                self.messages.push(MessageRecord::ToolResult {
                    id: *id,
                    status: *status,
                    content: content.clone(),
                });
            }
            SessionEvent::ModeChanged { mode } => self.mode = *mode,
            SessionEvent::PermissionRequested { .. } | SessionEvent::PermissionDecided { .. } => {}
            event @ (SessionEvent::LocationBound { .. } | SessionEvent::LocationRebound { .. }) => {
                apply_location_event(&mut self.workspace, event);
            }
        }
    }
}
