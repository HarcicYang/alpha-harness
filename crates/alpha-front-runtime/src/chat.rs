use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use alpha_protocol::Id;
use alpha_schema::{
    AssistantContentBlock, EventEnvelope, LiveAgentEvent, MessageId, PermissionMode,
    ReasoningLevel, Seq, SessionEvent, SessionEventNotification, SessionId, ToolCallId, ToolStatus,
    TurnEndReason, TurnId,
};
use serde_json::Value;

/// One structured transcript item independent of any renderer.
#[derive(Clone, Debug, PartialEq)]
pub enum TranscriptItem {
    /// A durable user message.
    User {
        /// Message text.
        text: String,
    },
    /// A steer message waiting in the bounded queue.
    QueuedSteer {
        /// Stable steer identity.
        id: MessageId,
        /// Steer text.
        text: String,
    },
    /// A live assistant text stream.
    AssistantDelta {
        /// Accumulated text.
        text: String,
    },
    /// A live reasoning stream.
    ThinkingDelta {
        /// Accumulated reasoning.
        text: String,
    },
    /// A durable assistant message preserving block order.
    AssistantMessage {
        /// Structured assistant content.
        blocks: Vec<AssistantContentBlock>,
    },
    /// A durable local tool call.
    ToolCall {
        /// Call identity.
        id: ToolCallId,
        /// Tool name.
        name: String,
        /// Decoded input.
        input: Value,
    },
    /// A durable settled tool call.
    ToolResult {
        /// Call identity.
        id: ToolCallId,
        /// Tool name.
        name: String,
        /// Outcome status.
        status: ToolStatus,
        /// Model-visible content.
        content: String,
    },
    /// A visible runtime error.
    Error {
        /// Error text.
        message: String,
    },
}

/// A pending server-initiated interaction.
#[derive(Clone, Debug)]
pub struct InteractionPrompt {
    /// Protocol request identity used for the response.
    pub id: Id,
    /// Session that requested the interaction.
    pub session_id: SessionId,
    /// Interaction kind.
    pub kind: String,
    /// Interaction-specific payload.
    pub prompt: Value,
    /// Deadline after which the runtime responds with timeout.
    pub deadline: Instant,
}

/// A user decision for an interaction request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InteractionDecision {
    /// Allow one execution.
    Once,
    /// Allow for the rest of the session.
    Session,
    /// Deny the request.
    Deny,
    /// Allow the core to observe a client timeout.
    Timeout,
}

impl InteractionDecision {
    /// Protocol decision string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Once => "once",
            Self::Session => "session",
            Self::Deny => "deny",
            Self::Timeout => "timeout",
        }
    }
}

/// UI-independent chat and session state.
#[derive(Clone, Debug, Default)]
pub struct ChatState {
    /// Active session identity.
    pub session_id: Option<SessionId>,
    /// Structured transcript.
    pub transcript: Vec<TranscriptItem>,
    /// Index of the in-progress assistant stream.
    pub streaming_assistant: Option<usize>,
    /// Index of the in-progress thinking stream.
    pub streaming_thinking: Option<usize>,
    /// Current running turn.
    pub running_turn: Option<TurnId>,
    /// Pending interaction request.
    pub interaction: Option<InteractionPrompt>,
    /// Provider instance or provider kind.
    pub provider: String,
    /// Model identifier.
    pub model: String,
    /// Requested reasoning level.
    pub reasoning: ReasoningLevel,
    /// Effective permission mode.
    pub mode: PermissionMode,
    /// Provider-native tools waiting for the next session creation.
    pub pending_provider_tools: Option<Vec<String>>,
    /// Human-readable status line.
    pub status: Option<String>,
    /// Last durable sequence applied.
    pub last_seq: Option<Seq>,
    pub(crate) tool_names: BTreeMap<ToolCallId, String>,
}

impl ChatState {
    /// Start a new unpersisted draft.
    pub fn new_session(&mut self) {
        self.session_id = None;
        self.transcript.clear();
        self.streaming_assistant = None;
        self.streaming_thinking = None;
        self.running_turn = None;
        self.interaction = None;
        self.pending_provider_tools = None;
        self.status = Some("new session".to_owned());
        self.last_seq = None;
        self.tool_names.clear();
    }

    /// Apply one typed session notification.
    pub fn apply(&mut self, notification: SessionEventNotification) -> Vec<ChatUpdate> {
        if self.session_id != Some(notification.session_id()) {
            return Vec::new();
        }
        match notification {
            SessionEventNotification::Durable { event, .. } => self.apply_durable(&event),
            SessionEventNotification::Live { event, .. } => self.apply_live(&event),
        }
    }

    /// Apply an interaction request.
    pub fn set_interaction(
        &mut self,
        id: Id,
        session_id: SessionId,
        kind: String,
        prompt: Value,
    ) -> ChatUpdate {
        self.interaction = Some(InteractionPrompt {
            id,
            session_id,
            kind: kind.clone(),
            prompt,
            deadline: Instant::now() + Duration::from_secs(300),
        });
        ChatUpdate::InteractionRequested { kind }
    }

    /// Clear the current interaction after a response.
    pub fn finish_interaction(&mut self) -> Option<InteractionPrompt> {
        self.interaction.take()
    }

    /// Return whether the current interaction has expired.
    pub fn interaction_expired(&self) -> bool {
        self.interaction
            .as_ref()
            .is_some_and(|prompt| Instant::now() >= prompt.deadline)
    }

    /// Remove any in-progress live assistant or thinking item.
    pub fn clear_live_assistant(&mut self) {
        let mut indices = [self.streaming_thinking, self.streaming_assistant]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        indices.sort_unstable();
        indices.dedup();
        for index in indices.into_iter().rev() {
            if index < self.transcript.len()
                && matches!(
                    self.transcript[index],
                    TranscriptItem::AssistantDelta { .. } | TranscriptItem::ThinkingDelta { .. }
                )
            {
                self.transcript.remove(index);
            }
        }
        self.streaming_assistant = None;
        self.streaming_thinking = None;
    }

    /// Visible text in a transcript item, when it has a textual body.
    pub fn item_text(item: &TranscriptItem) -> Option<&str> {
        match item {
            TranscriptItem::User { text }
            | TranscriptItem::QueuedSteer { text, .. }
            | TranscriptItem::AssistantDelta { text }
            | TranscriptItem::ThinkingDelta { text } => Some(text),
            TranscriptItem::AssistantMessage { blocks } => {
                blocks.iter().find_map(AssistantContentBlock::text)
            }
            TranscriptItem::ToolCall { .. }
            | TranscriptItem::ToolResult { .. }
            | TranscriptItem::Error { .. } => None,
        }
    }

    /// Remove queued steer items after the core confirms their discard.
    pub(crate) fn discard_steers(&mut self, steer_ids: &[MessageId]) {
        let steer_ids = steer_ids.iter().copied().collect::<BTreeSet<_>>();
        self.transcript.retain(|item| {
            !matches!(
                item,
                TranscriptItem::QueuedSteer { id, .. } if steer_ids.contains(id)
            )
        });
    }

    fn apply_durable(&mut self, envelope: &EventEnvelope<SessionEvent>) -> Vec<ChatUpdate> {
        self.last_seq = Some(envelope.seq);
        let status = match &envelope.payload {
            SessionEvent::UserMessage { text, .. } => {
                self.transcript
                    .push(TranscriptItem::User { text: text.clone() });
                self.streaming_assistant = None;
                self.streaming_thinking = None;
                None
            }
            SessionEvent::AssistantMessage { blocks, text, .. } => {
                let mut blocks = blocks.clone();
                if blocks.is_empty() && !text.is_empty() {
                    blocks.push(AssistantContentBlock::Text { text: text.clone() });
                }
                self.clear_live_assistant();
                self.transcript
                    .push(TranscriptItem::AssistantMessage { blocks });
                None
            }
            SessionEvent::ToolCall {
                id, name, input, ..
            } => {
                self.tool_names.insert(*id, name.clone());
                self.transcript.push(TranscriptItem::ToolCall {
                    id: *id,
                    name: name.clone(),
                    input: input.clone(),
                });
                None
            }
            SessionEvent::ToolResult {
                id,
                status,
                content,
            } => {
                let name = self
                    .tool_names
                    .remove(id)
                    .unwrap_or_else(|| "tool".to_owned());
                self.transcript.push(TranscriptItem::ToolResult {
                    id: *id,
                    name,
                    status: *status,
                    content: content.clone(),
                });
                None
            }
            SessionEvent::TurnEnded { reason } => {
                self.running_turn = None;
                self.status = Some("turn finished".to_owned());
                return vec![ChatUpdate::TurnEnded { reason: *reason }];
            }
            SessionEvent::RouteChanged {
                provider,
                model,
                reasoning,
            } => {
                self.provider = provider.clone();
                self.model = model.clone();
                self.reasoning = *reasoning;
                Some(format!(
                    "route: {}/{}  effort:{}",
                    self.provider, self.model, self.reasoning
                ))
            }
            SessionEvent::ModeChanged { mode } => {
                self.mode = *mode;
                Some(format!("mode: {mode}"))
            }
            SessionEvent::SteerQueued { steer_id, text, .. } => {
                self.transcript.push(TranscriptItem::QueuedSteer {
                    id: *steer_id,
                    text: text.clone(),
                });
                Some("steer queued".to_owned())
            }
            SessionEvent::SteerInjected { steer_id, text, .. } => {
                self.transcript.retain(|item| {
                    !matches!(
                        item,
                        TranscriptItem::QueuedSteer { id, .. } if id == steer_id
                    )
                });
                self.transcript
                    .push(TranscriptItem::User { text: text.clone() });
                Some("steer injected".to_owned())
            }
            SessionEvent::SteerDiscarded { steer_id } => {
                self.transcript.retain(|item| {
                    !matches!(
                        item,
                        TranscriptItem::QueuedSteer { id, .. } if id == steer_id
                    )
                });
                Some("steer discarded".to_owned())
            }
            SessionEvent::CapabilitiesSet { provider_tools } => {
                self.pending_provider_tools = Some(provider_tools.clone());
                Some("provider capabilities updated".to_owned())
            }
            _ => None,
        };
        if let Some(status) = status {
            self.status = Some(status);
        }
        Vec::new()
    }

    fn apply_live(&mut self, event: &LiveAgentEvent) -> Vec<ChatUpdate> {
        match event {
            LiveAgentEvent::TextDelta { text } => {
                if let Some(index) = self.streaming_assistant {
                    if let Some(TranscriptItem::AssistantDelta { text: current }) =
                        self.transcript.get_mut(index)
                    {
                        current.push_str(text);
                    }
                } else {
                    self.transcript
                        .push(TranscriptItem::AssistantDelta { text: text.clone() });
                    self.streaming_assistant = Some(self.transcript.len() - 1);
                }
            }
            LiveAgentEvent::ThinkingDelta { text } => {
                if let Some(index) = self.streaming_thinking {
                    if let Some(TranscriptItem::ThinkingDelta { text: current }) =
                        self.transcript.get_mut(index)
                    {
                        current.push_str(text);
                    }
                } else {
                    self.transcript
                        .push(TranscriptItem::ThinkingDelta { text: text.clone() });
                    self.streaming_thinking = Some(self.transcript.len() - 1);
                }
            }
            LiveAgentEvent::StepStarted { step } => {
                self.status = Some(format!("model step {step}"));
            }
            LiveAgentEvent::ToolStarted { name, .. } => {
                self.status = Some(format!("running {name}"));
            }
            LiveAgentEvent::ToolFinished { name, status, .. } => {
                self.status = Some(format!("{name}: {status:?}"));
            }
            LiveAgentEvent::Error { message } => {
                self.clear_live_assistant();
                self.transcript.push(TranscriptItem::Error {
                    message: message.clone(),
                });
                self.status = Some(message.clone());
                self.running_turn = None;
            }
        }
        vec![ChatUpdate::Live(event.clone())]
    }
}

/// One observable update produced while reducing a core event.
#[derive(Clone, Debug, PartialEq)]
pub enum ChatUpdate {
    /// Live agent event.
    Live(LiveAgentEvent),
    /// Durable turn completion.
    TurnEnded {
        /// Why the turn ended.
        reason: TurnEndReason,
    },
    /// An interaction request became pending.
    InteractionRequested {
        /// Interaction kind.
        kind: String,
    },
}

#[cfg(test)]
mod tests {
    use alpha_schema::{
        EventEnvelope, LiveAgentEvent, MessageId, Seq, SessionEvent, SessionEventNotification,
        SessionId, Timestamp, ToolStatus, TurnId,
    };
    use serde_json::json;

    use super::{ChatState, TranscriptItem};

    #[test]
    fn live_text_is_replaced_by_the_durable_message() {
        let session_id = SessionId::new();
        let mut state = ChatState {
            session_id: Some(session_id),
            ..ChatState::default()
        };
        state.apply(SessionEventNotification::Live {
            session_id,
            turn_id: Some("turn".to_owned()),
            stream_id: Some("stream".to_owned()),
            step_id: None,
            event: LiveAgentEvent::TextDelta {
                text: "hello".to_owned(),
            },
        });
        assert_eq!(
            state.transcript,
            vec![TranscriptItem::AssistantDelta {
                text: "hello".to_owned()
            }]
        );

        state.apply(SessionEventNotification::Durable {
            session_id,
            event: EventEnvelope::at(
                Seq::new(1),
                Timestamp::from_millis(1),
                SessionEvent::AssistantMessage {
                    id: MessageId::new(),
                    blocks: vec![alpha_schema::AssistantContentBlock::Text {
                        text: "hello".to_owned(),
                    }],
                    text: String::new(),
                },
            ),
        });
        assert_eq!(
            state.transcript,
            vec![TranscriptItem::AssistantMessage {
                blocks: vec![alpha_schema::AssistantContentBlock::Text {
                    text: "hello".to_owned()
                }]
            }]
        );
    }

    #[test]
    fn tool_calls_keep_structured_input_and_output() {
        let session_id = SessionId::new();
        let call_id = alpha_schema::ToolCallId::new();
        let mut state = ChatState {
            session_id: Some(session_id),
            ..ChatState::default()
        };
        state.apply(SessionEventNotification::Durable {
            session_id,
            event: EventEnvelope::at(
                Seq::new(1),
                Timestamp::from_millis(1),
                SessionEvent::ToolCall {
                    id: call_id,
                    message_id: MessageId::new(),
                    name: "shell".to_owned(),
                    input: json!({ "command": "ls" }),
                },
            ),
        });
        state.apply(SessionEventNotification::Durable {
            session_id,
            event: EventEnvelope::at(
                Seq::new(2),
                Timestamp::from_millis(2),
                SessionEvent::ToolResult {
                    id: call_id,
                    status: ToolStatus::Completed,
                    content: "src".to_owned(),
                },
            ),
        });
        assert_eq!(
            state.transcript,
            vec![
                TranscriptItem::ToolCall {
                    id: call_id,
                    name: "shell".to_owned(),
                    input: json!({ "command": "ls" }),
                },
                TranscriptItem::ToolResult {
                    id: call_id,
                    name: "shell".to_owned(),
                    status: ToolStatus::Completed,
                    content: "src".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn queued_steers_are_visible_until_they_settle() {
        let session_id = SessionId::new();
        let mut state = ChatState {
            session_id: Some(session_id),
            ..ChatState::default()
        };
        let injected_id = MessageId::new();
        let turn_id = TurnId::new();
        state.apply(SessionEventNotification::Durable {
            session_id,
            event: EventEnvelope::at(
                Seq::new(1),
                Timestamp::from_millis(1),
                SessionEvent::SteerQueued {
                    steer_id: injected_id,
                    turn_id,
                    text: "adjust direction".to_owned(),
                },
            ),
        });
        assert_eq!(
            state.transcript,
            vec![TranscriptItem::QueuedSteer {
                id: injected_id,
                text: "adjust direction".to_owned(),
            }]
        );

        state.apply(SessionEventNotification::Durable {
            session_id,
            event: EventEnvelope::at(
                Seq::new(2),
                Timestamp::from_millis(2),
                SessionEvent::SteerInjected {
                    steer_id: injected_id,
                    turn_id,
                    message_id: MessageId::new(),
                    text: "adjust direction".to_owned(),
                },
            ),
        });
        assert_eq!(
            state.transcript,
            vec![TranscriptItem::User {
                text: "adjust direction".to_owned(),
            }]
        );

        let discarded_id = MessageId::new();
        state.apply(SessionEventNotification::Durable {
            session_id,
            event: EventEnvelope::at(
                Seq::new(3),
                Timestamp::from_millis(3),
                SessionEvent::SteerQueued {
                    steer_id: discarded_id,
                    turn_id,
                    text: "undo this".to_owned(),
                },
            ),
        });
        state.discard_steers(&[discarded_id]);
        assert!(state.transcript.iter().all(|item| !matches!(
            item,
            TranscriptItem::QueuedSteer { id, .. } if *id == discarded_id
        )));
    }
}
