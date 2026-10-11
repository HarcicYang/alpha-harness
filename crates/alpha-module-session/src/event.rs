//! Durable session events.

use alpha_schema::{AssistantContentBlock, MessageId, PermissionMode, ToolCallId, TurnId};
use serde::{Deserialize, Serialize};

use crate::header::WorkspaceRef;

/// Why a turn ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnEndReason {
    /// The turn finished normally.
    Completed,
    /// The turn was interrupted, including crash repair.
    Interrupted,
    /// The turn failed before producing a completed assistant message.
    Failed,
    /// The turn was rejected before its first step.
    Rejected,
}

/// Outcome recorded for a tool call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    /// The tool completed.
    Completed,
    /// The tool returned an expected, model-visible failure.
    Failed,
    /// The tool did not finish and was recorded as interrupted.
    Interrupted,
}

/// One durable session event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    /// A turn opened.
    TurnStarted,
    /// A turn closed.
    TurnEnded {
        /// Why the turn ended.
        reason: TurnEndReason,
    },
    /// A user message entered the log.
    UserMessage {
        /// Message identity.
        id: MessageId,
        /// Message text.
        text: String,
    },
    /// A steer message entered the bounded queue.
    SteerQueued {
        /// Stable steer identity.
        #[serde(default = "default_steer_id")]
        steer_id: MessageId,
        /// Turn receiving the steer.
        turn_id: TurnId,
        /// Steer text.
        text: String,
    },
    /// A queued steer was injected at a safe boundary.
    SteerInjected {
        /// Stable steer identity.
        #[serde(default = "default_steer_id")]
        steer_id: MessageId,
        /// Turn receiving the steer.
        turn_id: TurnId,
        /// Message identity used in model history.
        message_id: MessageId,
        /// Steer text.
        text: String,
    },
    /// A queued steer was discarded before injection.
    SteerDiscarded {
        /// Stable steer identity.
        #[serde(default = "default_steer_id")]
        steer_id: MessageId,
    },
    /// The complete system prompt snapshot used by this session.
    SystemPromptSet {
        /// Full model-visible system prompt.
        text: String,
    },
    /// Frozen request capabilities for one model step.
    RequestHeader {
        /// Model identifier.
        model: String,
        /// Local tool names.
        tools: Vec<String>,
        /// Provider-native tool names.
        provider_tools: Vec<String>,
    },
    /// Session-level provider-native tool override.
    CapabilitiesSet {
        /// Provider-native tool names.
        provider_tools: Vec<String>,
    },
    /// Older context was replaced by a model-generated summary.
    ContextCompacted {
        /// Summary that replaces the recorded range.
        summary: String,
        /// First durable sequence replaced by the summary.
        from_seq: alpha_schema::Seq,
        /// Last durable sequence replaced by the summary.
        through_seq: alpha_schema::Seq,
    },
    /// An assistant message entered the log.
    AssistantMessage {
        /// Message identity.
        id: MessageId,
        /// Structured assistant content.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        blocks: Vec<AssistantContentBlock>,
        /// Legacy text form accepted when reading schema 1 logs.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        text: String,
    },
    /// The model requested a tool call.
    ToolCall {
        /// Call identity.
        id: ToolCallId,
        /// Assistant message that produced this call.
        message_id: MessageId,
        /// Tool name.
        name: String,
        /// Decoded tool input.
        input: serde_json::Value,
    },
    /// A tool call settled.
    ToolResult {
        /// Call identity.
        id: ToolCallId,
        /// Outcome status.
        status: ToolStatus,
        /// Model-visible content.
        content: String,
    },
    /// The permission mode changed.
    ModeChanged {
        /// The new mode.
        mode: PermissionMode,
    },
    /// A permission check was requested.
    PermissionRequested {
        /// Action key.
        action: String,
        /// Resources touched by the action.
        resources: Vec<String>,
    },
    /// A permission check settled.
    PermissionDecided {
        /// Action key.
        action: String,
        /// Resources touched by the action.
        resources: Vec<String>,
        /// Whether the action was allowed.
        allowed: bool,
        /// Decision source label.
        source: String,
    },
    /// A session bound to a workspace location.
    LocationBound {
        /// The bound workspace and cwd.
        workspace: WorkspaceRef,
    },
    /// A session rebound to a new workspace location.
    LocationRebound {
        /// The new workspace and cwd.
        workspace: WorkspaceRef,
    },
}

impl SessionEvent {
    /// Visible assistant text for assistant messages.
    pub fn assistant_text(&self) -> Option<String> {
        match self {
            Self::AssistantMessage { blocks, text, .. } => {
                if blocks.is_empty() {
                    Some(text.clone())
                } else {
                    Some(
                        blocks
                            .iter()
                            .filter_map(AssistantContentBlock::text)
                            .collect::<String>(),
                    )
                }
            }
            _ => None,
        }
    }
}

fn default_steer_id() -> MessageId {
    MessageId::new()
}
