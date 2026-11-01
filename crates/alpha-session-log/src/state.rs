//! Deterministic projections over the durable event log.

use std::collections::BTreeSet;

use alpha_schema::{
    AssistantContentBlock, EventEnvelope, ImageAttachment, MessageId, PermissionMode,
    ReasoningLevel, Seq, ToolCallId,
};

use crate::event::{SessionEvent, ToolStatus};
use crate::header::{SessionHeader, WorkspaceRef, apply_location_event};

/// One message in the projected history.
#[derive(Clone, Debug, PartialEq)]
pub enum MessageRecord {
    /// A user message.
    User {
        /// Durable sequence of the source event.
        seq: Seq,
        /// Message identity.
        id: MessageId,
        /// Message text.
        text: String,
        /// Attached images.
        images: Vec<ImageAttachment>,
    },
    /// Injected dynamic context.
    Context {
        /// Durable sequence of the source event.
        seq: Seq,
        /// Message identity.
        id: MessageId,
        /// Stable context section key.
        key: String,
        /// Model-visible message.
        text: String,
    },
    /// An assistant message.
    Assistant {
        /// Durable sequence of the source event.
        seq: Seq,
        /// Message identity.
        id: MessageId,
        /// Structured content blocks.
        blocks: Vec<AssistantContentBlock>,
        /// Message text.
        text: String,
    },
    /// A tool call requested by the assistant.
    ToolCall {
        /// Durable sequence of the source event.
        seq: Seq,
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
        /// Durable sequence of the source event.
        seq: Seq,
        /// Call identity.
        id: ToolCallId,
        /// Outcome status.
        status: ToolStatus,
        /// Model-visible content.
        content: String,
        /// Images returned to the model.
        images: Vec<ImageAttachment>,
    },
}

impl MessageRecord {
    /// Durable sequence of the source event.
    pub fn seq(&self) -> Seq {
        match self {
            Self::User { seq, .. }
            | Self::Context { seq, .. }
            | Self::Assistant { seq, .. }
            | Self::ToolCall { seq, .. }
            | Self::ToolResult { seq, .. } => *seq,
        }
    }
}

/// Current compaction projection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompactionRecord {
    /// Summary replacing the recorded range.
    pub summary: String,
    /// First durable sequence replaced by the summary.
    pub from_seq: Seq,
    /// Last durable sequence replaced by the summary.
    pub through_seq: Seq,
}

/// Projected session state, folded from durable events.
#[derive(Clone, Debug)]
pub struct SessionState {
    /// Latest complete system prompt snapshot.
    pub system_prompt: Option<String>,
    /// Latest context compaction.
    pub compaction: Option<CompactionRecord>,
    /// Effective permission mode.
    pub mode: PermissionMode,
    /// Effective provider instance id or provider kind.
    pub provider: String,
    /// Effective model identifier. Empty means the provider default.
    pub model: String,
    /// Effective reasoning effort.
    pub reasoning: ReasoningLevel,
    /// Session-level provider-native tool override.
    pub provider_tools_override: Option<Vec<String>>,
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
            system_prompt: None,
            compaction: None,
            mode: header.mode,
            provider: header.provider.clone(),
            model: header.model.clone(),
            reasoning: ReasoningLevel::Auto,
            provider_tools_override: None,
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
            SessionEvent::UserMessage { id, text, images } => {
                self.messages.push(MessageRecord::User {
                    seq: envelope.seq,
                    id: *id,
                    text: text.clone(),
                    images: images.clone(),
                })
            }
            SessionEvent::ContextInjected { id, key, text } => {
                self.messages.push(MessageRecord::Context {
                    seq: envelope.seq,
                    id: *id,
                    key: key.clone(),
                    text: text.clone(),
                });
            }
            SessionEvent::SteerQueued { .. } => {}
            SessionEvent::SteerDiscarded { .. } => {}
            SessionEvent::SteerInjected {
                message_id, text, ..
            } => self.messages.push(MessageRecord::User {
                seq: envelope.seq,
                id: *message_id,
                text: text.clone(),
                images: Vec::new(),
            }),
            SessionEvent::SystemPromptSet { text } => self.system_prompt = Some(text.clone()),
            SessionEvent::RequestHeader { .. } => {}
            SessionEvent::RouteChanged {
                provider,
                model,
                reasoning,
            } => {
                self.provider = provider.clone();
                self.model = model.clone();
                self.reasoning = *reasoning;
            }
            SessionEvent::CapabilitiesSet { provider_tools } => {
                self.provider_tools_override = Some(provider_tools.clone());
            }
            SessionEvent::ContextCompacted {
                summary,
                from_seq,
                through_seq,
            } => {
                self.compaction = Some(CompactionRecord {
                    summary: summary.clone(),
                    from_seq: *from_seq,
                    through_seq: *through_seq,
                });
            }
            SessionEvent::AssistantMessage { id, blocks, text } => {
                let blocks = if blocks.is_empty() && !text.is_empty() {
                    vec![AssistantContentBlock::Text { text: text.clone() }]
                } else {
                    blocks.clone()
                };
                let text = if text.is_empty() {
                    blocks
                        .iter()
                        .filter_map(AssistantContentBlock::text)
                        .collect::<String>()
                } else {
                    text.clone()
                };
                self.messages.push(MessageRecord::Assistant {
                    seq: envelope.seq,
                    id: *id,
                    blocks,
                    text,
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
                    seq: envelope.seq,
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
                images,
            } => {
                self.pending_tool_calls.remove(id);
                self.messages.push(MessageRecord::ToolResult {
                    seq: envelope.seq,
                    id: *id,
                    status: *status,
                    content: content.clone(),
                    images: images.clone(),
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
