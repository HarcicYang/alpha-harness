use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use alpha_protocol::Id;
use alpha_schema::{
    AssistantContentBlock, EventEnvelope, ImageAttachment, LiveAgentEvent, MessageId,
    PermissionMode, ReasoningLevel, Seq, SessionEvent, SessionEventNotification, SessionId,
    Timestamp, ToolCallId, ToolStatus, TurnEndReason, TurnId,
};
use serde_json::Value;

/// One structured transcript item independent of any renderer.
#[derive(Clone, Debug, PartialEq)]
pub enum TranscriptItem {
    /// A durable user message.
    User {
        /// Message text.
        text: String,
        /// Attached images.
        images: Vec<ImageAttachment>,
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
        /// Images returned with the result.
        images: Vec<ImageAttachment>,
    },
    /// Incremental output from a running tool.
    ToolOutput {
        /// Call identity.
        id: ToolCallId,
        /// Accumulated live output.
        text: String,
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

/// Kind of file change observed in the durable session log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileChangeKind {
    /// A file was created.
    Added,
    /// A file was updated.
    Modified,
}

/// One file changed by a completed local tool call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    /// Workspace-relative path.
    pub path: String,
    /// Change kind inferred from the tool call.
    pub kind: FileChangeKind,
    /// Lines introduced by the change.
    pub added_lines: usize,
    /// Lines removed by the change.
    pub removed_lines: usize,
    /// Original text when the tool exposed it.
    pub original: Option<String>,
    /// Updated text when the tool exposed it.
    pub updated: Option<String>,
}

/// Display state of one context activity entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityState {
    /// Work is still running or waiting for input.
    Running,
    /// Work completed successfully.
    Completed,
    /// Work failed or was interrupted.
    Failed,
}

/// One recent activity entry derived from durable events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityItem {
    /// Event timestamp.
    pub at: Timestamp,
    /// Short activity title.
    pub title: String,
    /// Supporting detail.
    pub detail: String,
    /// Current activity state.
    pub state: ActivityState,
}

#[derive(Clone, Debug)]
struct PendingFileChange {
    path: String,
    kind: FileChangeKind,
    added_lines: usize,
    removed_lines: usize,
    original: Option<String>,
    updated: Option<String>,
}

/// UI-independent chat and session state.
#[derive(Clone, Debug, Default)]
pub struct ChatState {
    /// Active session identity.
    pub session_id: Option<SessionId>,
    /// Workspace bound to the active session or draft.
    pub workspace: String,
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
    /// Estimated tokens currently visible to the model.
    pub context_tokens: usize,
    /// Estimated assistant output tokens recorded in this session.
    pub output_tokens: usize,
    /// Local tools frozen by the latest request header.
    pub local_tools: Vec<String>,
    /// Provider-native tools active for the current route.
    pub provider_tools: Vec<String>,
    /// File changes produced by completed local tool calls.
    pub file_changes: Vec<FileChange>,
    /// Recent activity derived from durable events.
    pub activity: Vec<ActivityItem>,
    /// Human-readable status line.
    pub status: Option<String>,
    /// Last durable sequence applied.
    pub last_seq: Option<Seq>,
    pub(crate) tool_names: BTreeMap<ToolCallId, String>,
    context_segments: BTreeMap<Seq, usize>,
    system_prompt_seq: Option<Seq>,
    request_header_seq: Option<Seq>,
    pending_file_changes: BTreeMap<ToolCallId, PendingFileChange>,
    tool_activity: BTreeMap<ToolCallId, usize>,
}

impl ChatState {
    /// Start a new unpersisted draft.
    pub fn new_session(&mut self) {
        self.reset_projection();
        self.status = Some("new session".to_owned());
    }

    pub(crate) fn reset_projection(&mut self) {
        self.session_id = None;
        self.workspace.clear();
        self.transcript.clear();
        self.streaming_assistant = None;
        self.streaming_thinking = None;
        self.running_turn = None;
        self.interaction = None;
        self.pending_provider_tools = None;
        self.context_tokens = 0;
        self.output_tokens = 0;
        self.local_tools.clear();
        self.provider_tools.clear();
        self.file_changes.clear();
        self.activity.clear();
        self.status = None;
        self.last_seq = None;
        self.tool_names.clear();
        self.context_segments.clear();
        self.system_prompt_seq = None;
        self.request_header_seq = None;
        self.pending_file_changes.clear();
        self.tool_activity.clear();
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
        self.push_activity(
            Timestamp::now(),
            "Waiting for approval",
            interaction_detail(&kind, &prompt),
            ActivityState::Running,
        );
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
            TranscriptItem::User { text, .. }
            | TranscriptItem::QueuedSteer { text, .. }
            | TranscriptItem::AssistantDelta { text }
            | TranscriptItem::ThinkingDelta { text } => Some(text),
            TranscriptItem::AssistantMessage { blocks } => {
                blocks.iter().find_map(AssistantContentBlock::text)
            }
            TranscriptItem::ToolCall { .. }
            | TranscriptItem::ToolResult { .. }
            | TranscriptItem::ToolOutput { .. }
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
        self.project_context(envelope);
        let status = match &envelope.payload {
            SessionEvent::UserMessage { text, images, .. } => {
                self.transcript.push(TranscriptItem::User {
                    text: text.clone(),
                    images: images.clone(),
                });
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
                images,
            } => {
                self.transcript.retain(
                    |item| !matches!(item, TranscriptItem::ToolOutput { id: output_id, .. } if output_id == id),
                );
                let name = self
                    .tool_names
                    .remove(id)
                    .unwrap_or_else(|| "tool".to_owned());
                self.transcript.push(TranscriptItem::ToolResult {
                    id: *id,
                    name,
                    status: *status,
                    content: content.clone(),
                    images: images.clone(),
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
            SessionEvent::SteerInjected {
                steer_id,
                text,
                images,
                ..
            } => {
                self.transcript.retain(|item| {
                    !matches!(
                        item,
                        TranscriptItem::QueuedSteer { id, .. } if id == steer_id
                    )
                });
                self.transcript.push(TranscriptItem::User {
                    text: text.clone(),
                    images: images.clone(),
                });
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

    fn project_context(&mut self, envelope: &EventEnvelope<SessionEvent>) {
        match &envelope.payload {
            SessionEvent::SystemPromptSet { text } => {
                if let Some(previous) = self.system_prompt_seq.replace(envelope.seq) {
                    self.remove_context_segment(previous);
                }
                self.insert_context_segment(envelope.seq, estimate_tokens(text) + 4);
            }
            SessionEvent::ContextInjected { text, .. } => {
                self.insert_context_segment(envelope.seq, estimate_tokens(text) + 4);
            }
            SessionEvent::ContextCompacted {
                summary,
                from_seq,
                through_seq,
            } => {
                self.context_segments
                    .retain(|seq, _| *seq < *from_seq || *seq > *through_seq);
                self.recompute_context_tokens();
                self.insert_context_segment(*through_seq, estimate_tokens(summary) + 4);
                self.push_activity(
                    envelope.at,
                    "Context compacted",
                    format!("Sequences {from_seq} through {through_seq} were summarized."),
                    ActivityState::Completed,
                );
            }
            SessionEvent::TurnStarted => {
                self.push_activity(
                    envelope.at,
                    "Turn started",
                    format!("{} · {} effort", self.model, self.reasoning),
                    ActivityState::Running,
                );
            }
            SessionEvent::UserMessage { text, .. } => {
                self.insert_context_segment(envelope.seq, estimate_tokens(text) + 4);
            }
            SessionEvent::AssistantMessage { blocks, text, .. } => {
                let tokens = assistant_block_tokens(blocks, text);
                self.insert_context_segment(envelope.seq, tokens);
                self.output_tokens = self.output_tokens.saturating_add(tokens);
            }
            SessionEvent::ToolCall {
                id, name, input, ..
            } => {
                let tokens = estimate_tokens(name) + estimate_tokens(&input.to_string()) + 8;
                self.insert_context_segment(envelope.seq, tokens);
                if let Some(change) = pending_file_change(name, input) {
                    self.pending_file_changes.insert(*id, change);
                }
                self.push_activity(
                    envelope.at,
                    tool_activity_title(name),
                    tool_activity_detail(name, input),
                    ActivityState::Running,
                );
                let index = self.activity.len().saturating_sub(1);
                self.tool_activity.insert(*id, index);
            }
            SessionEvent::ToolResult {
                id,
                status,
                content,
                ..
            } => {
                self.insert_context_segment(envelope.seq, estimate_tokens(content) + 4);
                if let Some(index) = self.tool_activity.remove(id)
                    && let Some(activity) = self.activity.get_mut(index)
                {
                    activity.state = match status {
                        ToolStatus::Completed => ActivityState::Completed,
                        ToolStatus::Failed | ToolStatus::Interrupted => ActivityState::Failed,
                    };
                    activity.detail =
                        format!("{} · {}", activity.detail, compact_text(content, 96));
                }
                if *status == ToolStatus::Completed
                    && let Some(mut change) = self.pending_file_changes.remove(id)
                {
                    if change.kind == FileChangeKind::Modified {
                        let replacements = replacement_count(content);
                        change.added_lines = change.added_lines.saturating_mul(replacements);
                        change.removed_lines = change.removed_lines.saturating_mul(replacements);
                    }
                    self.record_file_change(change);
                }
            }
            SessionEvent::RequestHeader {
                tools,
                provider_tools,
                ..
            } => {
                self.local_tools = tools.clone();
                self.provider_tools = provider_tools.clone();
                if let Some(previous) = self.request_header_seq.replace(envelope.seq) {
                    self.remove_context_segment(previous);
                }
                let tools = tools
                    .iter()
                    .chain(provider_tools)
                    .map(|name| estimate_tokens(name) + 8)
                    .sum::<usize>();
                self.insert_context_segment(envelope.seq, tools);
            }
            SessionEvent::CapabilitiesSet { provider_tools } => {
                self.provider_tools = provider_tools.clone();
                self.push_activity(
                    envelope.at,
                    "Capabilities updated",
                    if provider_tools.is_empty() {
                        "Provider tools cleared.".to_owned()
                    } else {
                        provider_tools.join(", ")
                    },
                    ActivityState::Completed,
                );
            }
            SessionEvent::RouteChanged {
                provider,
                model,
                reasoning,
            } => {
                self.push_activity(
                    envelope.at,
                    "Model route updated",
                    format!("{provider} · {model} · {reasoning}"),
                    ActivityState::Completed,
                );
            }
            SessionEvent::ModeChanged { mode } => {
                self.push_activity(
                    envelope.at,
                    "Permission mode updated",
                    mode.to_string(),
                    ActivityState::Completed,
                );
            }
            SessionEvent::TurnEnded { reason } => {
                let state = match reason {
                    TurnEndReason::Completed => ActivityState::Completed,
                    TurnEndReason::Interrupted
                    | TurnEndReason::Failed
                    | TurnEndReason::Rejected => ActivityState::Failed,
                };
                self.push_activity(envelope.at, "Turn finished", format!("{reason:?}"), state);
            }
            SessionEvent::PermissionRequested { action, resources } => {
                self.push_activity(
                    envelope.at,
                    "Permission requested",
                    format!("{action} · {}", resources.join(", ")),
                    ActivityState::Running,
                );
            }
            SessionEvent::PermissionDecided {
                action,
                allowed,
                source,
                ..
            } => {
                self.push_activity(
                    envelope.at,
                    "Permission decided",
                    format!(
                        "{action} · {source} · {}",
                        if *allowed { "allowed" } else { "denied" }
                    ),
                    if *allowed {
                        ActivityState::Completed
                    } else {
                        ActivityState::Failed
                    },
                );
            }
            _ => {}
        }
    }

    fn insert_context_segment(&mut self, seq: Seq, tokens: usize) {
        let previous = self.context_segments.insert(seq, tokens).unwrap_or(0);
        self.context_tokens = self
            .context_tokens
            .saturating_sub(previous)
            .saturating_add(tokens);
    }

    fn remove_context_segment(&mut self, seq: Seq) {
        if let Some(tokens) = self.context_segments.remove(&seq) {
            self.context_tokens = self.context_tokens.saturating_sub(tokens);
        }
    }

    fn recompute_context_tokens(&mut self) {
        self.context_tokens = self.context_segments.values().sum();
    }

    fn record_file_change(&mut self, change: PendingFileChange) {
        if let Some(existing) = self
            .file_changes
            .iter_mut()
            .find(|existing| existing.path == change.path)
        {
            existing.kind = change.kind;
            existing.added_lines = existing.added_lines.saturating_add(change.added_lines);
            existing.removed_lines = existing.removed_lines.saturating_add(change.removed_lines);
            existing.updated = change.updated;
        } else {
            self.file_changes.push(FileChange {
                path: change.path,
                kind: change.kind,
                added_lines: change.added_lines,
                removed_lines: change.removed_lines,
                original: change.original,
                updated: change.updated,
            });
        }
    }

    fn push_activity(
        &mut self,
        at: Timestamp,
        title: impl Into<String>,
        detail: impl Into<String>,
        state: ActivityState,
    ) {
        const MAX_ACTIVITY: usize = 32;
        if self.activity.len() == MAX_ACTIVITY {
            self.activity.remove(0);
            for index in self.tool_activity.values_mut() {
                *index = index.saturating_sub(1);
            }
        }
        self.activity.push(ActivityItem {
            at,
            title: title.into(),
            detail: detail.into(),
            state,
        });
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
            LiveAgentEvent::ToolOutput { call_id, text } => {
                if let Some(TranscriptItem::ToolOutput {
                    text: current, ..
                }) = self.transcript.iter_mut().rev().find(
                    |item| matches!(item, TranscriptItem::ToolOutput { id, .. } if id == call_id),
                ) {
                    current.push_str(text);
                } else {
                    self.transcript.push(TranscriptItem::ToolOutput {
                        id: *call_id,
                        text: text.clone(),
                    });
                }
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

fn estimate_tokens(text: &str) -> usize {
    let mut ascii = 0usize;
    let mut non_ascii = 0usize;
    for character in text.chars() {
        if character.is_ascii() {
            ascii += 1;
        } else {
            non_ascii += 1;
        }
    }
    ascii.div_ceil(4) + non_ascii
}

fn assistant_block_tokens(blocks: &[AssistantContentBlock], fallback: &str) -> usize {
    blocks
        .iter()
        .map(|block| match block {
            AssistantContentBlock::Text { text } | AssistantContentBlock::Thinking { text, .. } => {
                estimate_tokens(text) + 4
            }
            AssistantContentBlock::LocalToolCall {
                name, arguments, ..
            }
            | AssistantContentBlock::ProviderToolCall {
                name,
                input: arguments,
                ..
            } => estimate_tokens(name) + estimate_tokens(&arguments.to_string()) + 8,
            AssistantContentBlock::ProviderToolResult { name, content, .. } => {
                estimate_tokens(name) + estimate_tokens(&content.to_string()) + 8
            }
            AssistantContentBlock::Citation {
                url,
                title,
                snippet,
            } => {
                url.as_deref().map(estimate_tokens).unwrap_or_default()
                    + title.as_deref().map(estimate_tokens).unwrap_or_default()
                    + snippet.as_deref().map(estimate_tokens).unwrap_or_default()
            }
        })
        .sum::<usize>()
        .max(estimate_tokens(fallback))
}

fn pending_file_change(name: &str, input: &Value) -> Option<PendingFileChange> {
    let path = input.get("path")?.as_str()?.to_owned();
    match name {
        "edit" => {
            let old = input.get("old")?.as_str()?.to_owned();
            let new = input.get("new")?.as_str()?.to_owned();
            Some(PendingFileChange {
                path,
                kind: FileChangeKind::Modified,
                added_lines: line_count(&new),
                removed_lines: line_count(&old),
                original: Some(old),
                updated: Some(new),
            })
        }
        "write" => {
            let content = input.get("content")?.as_str()?.to_owned();
            Some(PendingFileChange {
                path,
                kind: FileChangeKind::Added,
                added_lines: line_count(&content),
                removed_lines: 0,
                original: None,
                updated: Some(content),
            })
        }
        _ => None,
    }
}

fn line_count(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        text.lines().count()
    }
}

fn replacement_count(content: &str) -> usize {
    content
        .strip_prefix("Replaced ")
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|count| count.parse().ok())
        .unwrap_or(1)
}

fn tool_activity_title(name: &str) -> String {
    match name {
        "read" => "Read file",
        "write" => "Wrote file",
        "edit" => "Edited file",
        "shell" => "Ran command",
        "grep" => "Searched workspace",
        _ => "Called tool",
    }
    .to_owned()
}

fn tool_activity_detail(name: &str, input: &Value) -> String {
    let detail = match name {
        "read" | "write" | "edit" => input.get("path").and_then(Value::as_str).map(str::to_owned),
        "shell" => input
            .get("command")
            .and_then(Value::as_str)
            .map(str::to_owned),
        "grep" => input
            .get("query")
            .or_else(|| input.get("pattern"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        _ => None,
    }
    .unwrap_or_else(|| input.to_string());
    compact_text(&detail, 120)
}

fn interaction_detail(kind: &str, prompt: &Value) -> String {
    for key in ["command", "path", "action", "reason"] {
        if let Some(value) = prompt.get(key).and_then(Value::as_str) {
            return compact_text(value, 120);
        }
    }
    compact_text(&format!("{kind} · {prompt}"), 120)
}

fn compact_text(text: &str, limit: usize) -> String {
    let mut compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() > limit {
        compact = compact.chars().take(limit.saturating_sub(3)).collect();
        compact.push_str("...");
    }
    compact
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
        EventEnvelope, LiveAgentEvent, MessageId, ReasoningLevel, Seq, SessionEvent,
        SessionEventNotification, SessionId, Timestamp, ToolStatus, TurnId,
    };
    use serde_json::json;

    use super::{ActivityState, ChatState, FileChangeKind, TranscriptItem, estimate_tokens};

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
                    images: Vec::new(),
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
                    images: Vec::new(),
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
                    images: Vec::new(),
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
                    images: Vec::new(),
                },
            ),
        });
        assert_eq!(
            state.transcript,
            vec![TranscriptItem::User {
                text: "adjust direction".to_owned(),
                images: Vec::new(),
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
                    images: Vec::new(),
                },
            ),
        });
        state.discard_steers(&[discarded_id]);
        assert!(state.transcript.iter().all(|item| !matches!(
            item,
            TranscriptItem::QueuedSteer { id, .. } if *id == discarded_id
        )));
    }

    #[test]
    fn context_projection_tracks_capabilities_changes_and_activity() {
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
                SessionEvent::RequestHeader {
                    provider: "local".to_owned(),
                    model: "model".to_owned(),
                    reasoning: ReasoningLevel::Auto,
                    tools: vec!["read".to_owned(), "edit".to_owned()],
                    provider_tools: vec!["web_search".to_owned()],
                },
            ),
        });
        state.apply(SessionEventNotification::Durable {
            session_id,
            event: EventEnvelope::at(
                Seq::new(2),
                Timestamp::from_millis(2),
                SessionEvent::ToolCall {
                    id: call_id,
                    message_id: MessageId::new(),
                    name: "edit".to_owned(),
                    input: json!({
                        "path": "src/lib.rs",
                        "old": "first\nsecond",
                        "new": "first\nupdated\nthird"
                    }),
                },
            ),
        });
        state.apply(SessionEventNotification::Durable {
            session_id,
            event: EventEnvelope::at(
                Seq::new(3),
                Timestamp::from_millis(3),
                SessionEvent::ToolResult {
                    id: call_id,
                    status: ToolStatus::Completed,
                    content: "Replaced 1 occurrence(s) in src/lib.rs".to_owned(),
                    images: Vec::new(),
                },
            ),
        });

        assert_eq!(state.local_tools, ["read", "edit"]);
        assert_eq!(state.provider_tools, ["web_search"]);
        assert!(state.context_tokens > 0);
        assert_eq!(state.file_changes.len(), 1);
        assert_eq!(state.file_changes[0].path, "src/lib.rs");
        assert_eq!(state.file_changes[0].kind, FileChangeKind::Modified);
        assert_eq!(state.file_changes[0].added_lines, 3);
        assert_eq!(state.file_changes[0].removed_lines, 2);
        assert_eq!(
            state.activity.last().map(|item| item.state),
            Some(ActivityState::Completed)
        );
    }

    #[test]
    fn failed_file_tools_do_not_record_changes() {
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
                    name: "write".to_owned(),
                    input: json!({ "path": "src/new.rs", "content": "fn main() {}" }),
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
                    status: ToolStatus::Failed,
                    content: "permission denied".to_owned(),
                    images: Vec::new(),
                },
            ),
        });

        assert!(state.file_changes.is_empty());
        assert_eq!(
            state.activity.last().map(|item| item.state),
            Some(ActivityState::Failed)
        );
    }

    #[test]
    fn context_compaction_replaces_tracked_prefix_tokens() {
        let session_id = SessionId::new();
        let mut state = ChatState {
            session_id: Some(session_id),
            ..ChatState::default()
        };
        for (seq, text) in [
            (1, "a long first message that should be compacted"),
            (2, "another long message that should be compacted"),
        ] {
            state.apply(SessionEventNotification::Durable {
                session_id,
                event: EventEnvelope::at(
                    Seq::new(seq),
                    Timestamp::from_millis(seq as i64),
                    SessionEvent::UserMessage {
                        id: MessageId::new(),
                        text: text.to_owned(),
                        images: Vec::new(),
                    },
                ),
            });
        }
        let before = state.context_tokens;
        state.apply(SessionEventNotification::Durable {
            session_id,
            event: EventEnvelope::at(
                Seq::new(3),
                Timestamp::from_millis(3),
                SessionEvent::ContextCompacted {
                    summary: "short summary".to_owned(),
                    from_seq: Seq::new(1),
                    through_seq: Seq::new(2),
                },
            ),
        });

        assert!(state.context_tokens < before);
        assert_eq!(state.context_tokens, estimate_tokens("short summary") + 4);
    }
}
