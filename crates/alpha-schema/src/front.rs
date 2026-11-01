//! Cross-process messages shared by every frontend.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    EventEnvelope, MessageId, PermissionMode, ProviderInstance, ReasoningLevel, SessionEvent,
    SessionId, Timestamp, ToolStatus, TurnId, WorkspaceRef,
};

/// One `session.event` notification.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEventNotification {
    /// A durable event replayed from or appended to the session log.
    Durable {
        /// Session that emitted the event.
        #[serde(rename = "sessionId")]
        session_id: SessionId,
        /// Durable event envelope.
        event: EventEnvelope<SessionEvent>,
    },
    /// A process-local live event.
    Live {
        /// Session that emitted the event.
        #[serde(rename = "sessionId")]
        session_id: SessionId,
        /// Turn that emitted the event.
        #[serde(rename = "turnId", default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        /// Stream identity for incremental updates.
        #[serde(rename = "streamId", default, skip_serializing_if = "Option::is_none")]
        stream_id: Option<String>,
        /// Model step that emitted the event.
        #[serde(rename = "stepId", default, skip_serializing_if = "Option::is_none")]
        step_id: Option<u64>,
        /// Live event payload.
        event: LiveAgentEvent,
    },
}

impl SessionEventNotification {
    /// Session that emitted this notification.
    pub fn session_id(&self) -> SessionId {
        match self {
            Self::Durable { session_id, .. } | Self::Live { session_id, .. } => *session_id,
        }
    }
}

/// One process-local agent event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LiveAgentEvent {
    /// A model step is about to start.
    StepStarted {
        /// One-based model step.
        step: u64,
    },
    /// Assistant text was received.
    TextDelta {
        /// Text fragment.
        text: String,
    },
    /// Assistant thinking was received.
    ThinkingDelta {
        /// Thinking fragment.
        text: String,
    },
    /// A completed tool call is about to execute.
    ToolStarted {
        /// Durable call identity.
        call_id: crate::ToolCallId,
        /// Tool name.
        name: String,
        /// Decoded tool input.
        input: Value,
    },
    /// Incremental output from a running tool call.
    ToolOutput {
        /// Durable call identity.
        call_id: crate::ToolCallId,
        /// Output delta.
        text: String,
    },
    /// A tool call settled.
    ToolFinished {
        /// Durable call identity.
        call_id: crate::ToolCallId,
        /// Tool name.
        name: String,
        /// Tool outcome.
        status: ToolStatus,
        /// Model-visible result.
        content: String,
    },
    /// A turn failed.
    Error {
        /// Human-readable failure.
        message: String,
    },
}

/// Result of creating a session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCreated {
    /// Session identity.
    pub session_id: SessionId,
    /// Effective permission mode.
    pub mode: PermissionMode,
    /// Workspace root.
    pub workspace: String,
    /// Effective provider instance id or provider kind.
    pub provider: String,
    /// Effective model identifier.
    pub model: String,
    /// Effective reasoning effort.
    pub reasoning: ReasoningLevel,
}

/// Result of resuming a session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResumed {
    /// Session identity.
    pub session_id: SessionId,
    /// Effective mode.
    pub mode: PermissionMode,
    /// Workspace root.
    pub workspace: String,
    /// Number of messages in the projection.
    pub messages: usize,
    /// Whether a turn was open before repair.
    pub turn_open: bool,
    /// Number of repair events appended.
    pub repairs: usize,
    /// Effective provider instance id or provider kind.
    pub provider: String,
    /// Effective model identifier.
    pub model: String,
    /// Effective reasoning effort.
    pub reasoning: ReasoningLevel,
}

/// Result of attaching to a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionAttached {
    /// Number of durable events replayed.
    pub replayed: usize,
}

/// One session listing returned by `session.list`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionListEntry {
    /// Session identity.
    pub id: SessionId,
    /// Creation time.
    pub created_at: Timestamp,
    /// Permission mode recorded at creation.
    pub mode: PermissionMode,
    /// Workspace binding.
    pub workspace: WorkspaceRef,
    /// Provider instance or provider kind selected at creation.
    #[serde(default)]
    pub provider: String,
    /// Model selected at creation.
    #[serde(default)]
    pub model: String,
    /// First user message in the durable log, when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_message: Option<String>,
}

/// Result of submitting a prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStarted {
    /// Identity of the started turn.
    pub turn_id: TurnId,
}

/// Parameters of one server-initiated interaction request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractionRequest {
    /// Session that needs an interaction response.
    pub session_id: SessionId,
    /// Interaction kind such as `permission`.
    pub kind: String,
    /// Interaction-specific payload.
    pub prompt: Value,
}

/// One steer message removed from a pending queue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscardedSteer {
    /// Stable steer identity.
    pub steer_id: MessageId,
    /// Steer text.
    pub text: String,
}

/// Pending steers removed by one operation.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscardedSteers {
    /// Discarded messages in queue order.
    pub steers: Vec<DiscardedSteer>,
}

/// Result of reading a provider model catalog.
pub type ProviderModelList = Vec<crate::ModelDescriptor>;

/// Result of creating or updating a provider instance.
pub type ProviderInstanceResponse = ProviderInstance;
