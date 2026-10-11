//! Default agent driver.

use std::collections::BTreeMap;
use std::sync::Arc;

use alpha_kernel::{Ctx, Event, ParallelEvent};
use alpha_module_session::{
    MessageRecord, SessionError, SessionEvent, SessionState, SessionWriter, ToolStatus,
    TurnEndReason,
};
use alpha_module_tools::{ToolContext, ToolError, ToolRegistry};
use alpha_schema::{MessageId, ToolCallId, TurnId};
use futures::StreamExt;
use futures::future::BoxFuture;
use serde_json::Value;
use thiserror::Error;

use crate::provider::{
    ChatMessage, CompletionRequest, CompletionResponse, ModelProvider, ProviderError, ToolCall,
    ToolDefinition,
};

const DEFAULT_MAX_STEPS: usize = 24;

/// One steer message waiting for a safe injection boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SteerMessage {
    /// Turn receiving the steer.
    pub turn_id: TurnId,
    /// Steer text.
    pub text: String,
}

/// Queue drained by the loop at step boundaries.
pub trait SteerQueue: Send + Sync + 'static {
    /// Drain messages waiting for injection.
    fn drain(&self) -> Vec<SteerMessage>;
}

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
    /// The turn exceeded the model step limit.
    #[error("turn exceeded {0} model steps")]
    StepLimit(usize),
}

/// Outcome of one completed turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnOutcome {
    /// Identity of the assistant message.
    pub assistant_message: MessageId,
    /// Assistant text.
    pub assistant_text: String,
}

/// Default driver: one model request over the recorded history.
#[derive(Clone)]
pub struct AgentLoop {
    provider: Arc<dyn ModelProvider>,
    model: String,
    tools: Option<Arc<ToolRegistry>>,
    context_preparer: Option<Arc<dyn ContextPreparer>>,
    steer_queue: Option<Arc<dyn SteerQueue>>,
    max_steps: usize,
}

impl AgentLoop {
    /// Create a loop around one provider.
    pub fn new(provider: Arc<dyn ModelProvider>, model: impl Into<String>) -> Self {
        Self {
            provider,
            model: model.into(),
            tools: None,
            context_preparer: None,
            steer_queue: None,
            max_steps: DEFAULT_MAX_STEPS,
        }
    }

    /// Override the maximum number of model steps in one turn.
    pub fn with_max_steps(mut self, max_steps: usize) -> Self {
        self.max_steps = max_steps.max(1);
        self
    }

    /// Attach a tool registry.
    pub fn with_tools(mut self, tools: Arc<ToolRegistry>) -> Self {
        self.tools = Some(tools);
        self
    }

    /// Attach a context-preparation pipeline.
    pub fn with_context_preparer(mut self, preparer: Arc<dyn ContextPreparer>) -> Self {
        self.context_preparer = Some(preparer);
        self
    }

    /// Attach a steer queue checked at model step boundaries.
    pub fn with_steer_queue(mut self, queue: Arc<dyn SteerQueue>) -> Self {
        self.steer_queue = Some(queue);
        self
    }

    /// The configured model identifier.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Run one turn and record it in the session log.
    ///
    /// The turn runs model steps until the model stops requesting tools or the
    /// step limit is reached. `state` is the projection before this turn.
    pub async fn run_turn(
        &self,
        ctx: &Ctx,
        writer: &mut SessionWriter,
        state: &SessionState,
        prompt: &str,
    ) -> Result<TurnOutcome, LoopError> {
        self.run_turn_impl(ctx, writer, state, prompt).await
    }

    async fn run_turn_impl(
        &self,
        ctx: &Ctx,
        writer: &mut SessionWriter,
        state: &SessionState,
        prompt: &str,
    ) -> Result<TurnOutcome, LoopError> {
        let session_id = writer.header().id;
        let mut effective_state = state.clone();
        if let Some(preparer) = &self.context_preparer {
            let _ = preparer
                .prepare(
                    writer,
                    &mut effective_state,
                    self.provider.as_ref(),
                    &self.model,
                    prompt,
                )
                .await;
        }
        writer.append(SessionEvent::TurnStarted)?;
        let user_id = MessageId::new();
        writer.append(SessionEvent::UserMessage {
            id: user_id,
            text: prompt.to_owned(),
        })?;

        let mut messages = HistoryBuilder::from_state(&effective_state);
        messages.push(ChatMessage::User(prompt.to_owned()));

        let tools = self.tool_definitions();
        let mut last_text = String::new();
        let mut last_message_id: Option<MessageId> = None;

        for step in 0..self.max_steps {
            self.inject_steers(writer, &mut messages)?;
            ctx.parallel(AgentStepStarted { step: step + 1 }).await;
            let request = CompletionRequest {
                model: self.model.clone(),
                messages: messages.clone(),
                tools: tools.clone(),
            };
            let response = match self.stream_response(ctx, request).await {
                Ok(response) => response,
                Err(failure) => {
                    if !failure.text.is_empty() {
                        writer.append(SessionEvent::AssistantMessage {
                            id: MessageId::new(),
                            text: failure.text.clone(),
                        })?;
                    }
                    writer.append(SessionEvent::TurnEnded {
                        reason: TurnEndReason::Failed,
                    })?;
                    ctx.parallel(AgentError {
                        message: failure.error.to_string(),
                    })
                    .await;
                    return Err(LoopError::Provider(failure.error));
                }
            };

            let message_id = MessageId::new();
            let text = response.text;
            let calls = response.tool_calls;
            if !text.is_empty() {
                writer.append(SessionEvent::AssistantMessage {
                    id: message_id,
                    text: text.clone(),
                })?;
                last_text = text.clone();
                last_message_id = Some(message_id);
            }

            if calls.is_empty() {
                if !text.is_empty() {
                    messages.push(ChatMessage::Assistant(text));
                }
                let assistant_id = match last_message_id {
                    Some(id) => id,
                    None => {
                        let id = MessageId::new();
                        writer.append(SessionEvent::AssistantMessage {
                            id,
                            text: "(no content)".to_owned(),
                        })?;
                        last_text = "(no content)".to_owned();
                        id
                    }
                };
                writer.append(SessionEvent::TurnEnded {
                    reason: TurnEndReason::Completed,
                })?;
                ctx.parallel(AgentTurnFinished {
                    assistant_message: assistant_id,
                })
                .await;
                return Ok(TurnOutcome {
                    assistant_message: assistant_id,
                    assistant_text: last_text,
                });
            }

            messages.push(ChatMessage::AssistantToolCalls {
                text,
                calls: calls.clone(),
            });
            for call in calls {
                self.execute_tool(ctx, writer, session_id, message_id, &mut messages, call)
                    .await?;
            }
        }

        // Wrap up without tools so the user still gets a summary.
        let request = CompletionRequest {
            model: self.model.clone(),
            messages: messages.clone(),
            tools: Vec::new(),
        };
        match self.stream_response(ctx, request).await {
            Ok(response) if !response.text.is_empty() => {
                let assistant_id = MessageId::new();
                writer.append(SessionEvent::AssistantMessage {
                    id: assistant_id,
                    text: response.text.clone(),
                })?;
                writer.append(SessionEvent::TurnEnded {
                    reason: TurnEndReason::Completed,
                })?;
                ctx.parallel(AgentTurnFinished {
                    assistant_message: assistant_id,
                })
                .await;
                Ok(TurnOutcome {
                    assistant_message: assistant_id,
                    assistant_text: response.text,
                })
            }
            Err(failure) => {
                if !failure.text.is_empty() {
                    writer.append(SessionEvent::AssistantMessage {
                        id: MessageId::new(),
                        text: failure.text,
                    })?;
                }
                writer.append(SessionEvent::TurnEnded {
                    reason: TurnEndReason::Failed,
                })?;
                ctx.parallel(AgentError {
                    message: failure.error.to_string(),
                })
                .await;
                Err(LoopError::Provider(failure.error))
            }
            Ok(_) => {
                writer.append(SessionEvent::TurnEnded {
                    reason: TurnEndReason::Failed,
                })?;
                ctx.parallel(AgentError {
                    message: format!("turn exceeded {} model steps", self.max_steps),
                })
                .await;
                Err(LoopError::StepLimit(self.max_steps))
            }
        }
    }

    async fn stream_response(
        &self,
        ctx: &Ctx,
        request: CompletionRequest,
    ) -> Result<CompletionResponse, StreamFailure> {
        let mut stream = self.provider.stream(request);
        let mut text = String::new();
        let mut calls: BTreeMap<usize, ToolCallBuilder> = BTreeMap::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| StreamFailure {
                error,
                text: text.clone(),
            })?;
            match chunk {
                crate::provider::CompletionChunk::Text(delta) => {
                    if !delta.is_empty() {
                        text.push_str(&delta);
                        ctx.parallel(AgentTextDelta { text: delta }).await;
                    }
                }
                crate::provider::CompletionChunk::ToolCall(delta) => {
                    let call = calls.entry(delta.index).or_default();
                    if let Some(id) = delta.id {
                        call.id = Some(id);
                    }
                    if let Some(name) = delta.name {
                        call.name = Some(name);
                    }
                    call.arguments.push_str(&delta.arguments);
                }
            }
        }

        let mut tool_calls = Vec::with_capacity(calls.len());
        for (index, call) in calls {
            let id = call.id.unwrap_or_else(|| format!("call_{index}"));
            let name = call.name.ok_or_else(|| StreamFailure {
                error: ProviderError::Request(format!("stream tool call {index} has no name")),
                text: text.clone(),
            })?;
            let arguments = if call.arguments.trim().is_empty() {
                Value::Null
            } else {
                serde_json::from_str(&call.arguments).map_err(|error| StreamFailure {
                    error: ProviderError::Request(format!(
                        "invalid tool arguments for `{name}`: {error}"
                    )),
                    text: text.clone(),
                })?
            };
            tool_calls.push(ToolCall {
                id,
                name,
                arguments,
            });
        }

        Ok(CompletionResponse { text, tool_calls })
    }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        let Some(registry) = &self.tools else {
            return Vec::new();
        };
        registry
            .names()
            .into_iter()
            .filter_map(|name| registry.get(&name))
            .map(|tool| ToolDefinition {
                name: tool.name().to_owned(),
                description: tool.description(),
                input_schema: tool.input_schema(),
            })
            .collect()
    }

    fn inject_steers(
        &self,
        writer: &mut SessionWriter,
        messages: &mut Vec<ChatMessage>,
    ) -> Result<(), LoopError> {
        let Some(queue) = &self.steer_queue else {
            return Ok(());
        };
        for steer in queue.drain() {
            writer.append(SessionEvent::SteerQueued {
                turn_id: steer.turn_id,
                text: steer.text.clone(),
            })?;
            let message_id = MessageId::new();
            writer.append(SessionEvent::SteerInjected {
                turn_id: steer.turn_id,
                message_id,
                text: steer.text.clone(),
            })?;
            messages.push(ChatMessage::User(steer.text));
        }
        Ok(())
    }

    async fn execute_tool(
        &self,
        ctx: &Ctx,
        writer: &mut SessionWriter,
        session_id: alpha_schema::SessionId,
        message_id: MessageId,
        messages: &mut Vec<ChatMessage>,
        call: ToolCall,
    ) -> Result<(), LoopError> {
        let call_id = ToolCallId::new();
        writer.append(SessionEvent::ToolCall {
            id: call_id,
            message_id,
            name: call.name.clone(),
            input: call.arguments.clone(),
        })?;

        ctx.parallel(AgentToolStarted {
            name: call.name.clone(),
            input: call.arguments.clone(),
        })
        .await;

        let outcome = match &self.tools {
            Some(registry) => match registry.get(&call.name) {
                Some(tool) => {
                    let context = ToolContext::new(session_id, message_id, call_id);
                    tool.execute(call.arguments, context).await
                }
                None => Err(ToolError::failure(format!("unknown tool `{}`", call.name))),
            },
            None => Err(ToolError::failure("no tools are registered")),
        };

        let (status, content) = match outcome {
            Ok(response) => (ToolStatus::Completed, response.content),
            Err(ToolError::Failure(message)) => (ToolStatus::Failed, message),
            Err(ToolError::Operational(message)) => {
                writer.append(SessionEvent::TurnEnded {
                    reason: TurnEndReason::Failed,
                })?;
                ctx.parallel(AgentError {
                    message: message.clone(),
                })
                .await;
                return Err(LoopError::Tool(message));
            }
        };
        ctx.parallel(AgentToolFinished {
            name: call.name.clone(),
            status,
            content: content.clone(),
        })
        .await;
        writer.append(SessionEvent::ToolResult {
            id: call_id,
            status,
            content: content.clone(),
        })?;
        messages.push(ChatMessage::ToolResult {
            call_id: call.id,
            content,
        });
        Ok(())
    }
}

#[derive(Default)]
struct ToolCallBuilder {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

struct StreamFailure {
    error: ProviderError,
    text: String,
}

/// A model step is about to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentStepStarted {
    /// One-based model step.
    pub step: usize,
}

impl ParallelEvent for AgentStepStarted {}
impl Event for AgentStepStarted {}

/// Assistant text was received.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentTextDelta {
    /// Text fragment.
    pub text: String,
}

impl ParallelEvent for AgentTextDelta {}
impl Event for AgentTextDelta {}

/// A completed tool call is about to execute.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentToolStarted {
    /// Tool name.
    pub name: String,
    /// Decoded tool input.
    pub input: Value,
}

impl ParallelEvent for AgentToolStarted {}
impl Event for AgentToolStarted {}

/// A tool call settled.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentToolFinished {
    /// Tool name.
    pub name: String,
    /// Tool status.
    pub status: ToolStatus,
    /// Model-visible result.
    pub content: String,
}

impl ParallelEvent for AgentToolFinished {}
impl Event for AgentToolFinished {}

/// One turn completed successfully.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentTurnFinished {
    /// Final assistant message.
    pub assistant_message: MessageId,
}

impl ParallelEvent for AgentTurnFinished {}
impl Event for AgentTurnFinished {}

/// One turn failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentError {
    /// Human-readable failure.
    pub message: String,
}

impl ParallelEvent for AgentError {}
impl Event for AgentError {}

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

/// Optional pre-turn context transformation.
///
/// Implementations may append durable context events and update the supplied
/// session projection. Returning an error leaves the original history intact.
pub trait ContextPreparer: Send + Sync + 'static {
    /// Prepare context before the driver records a new user message.
    fn prepare<'a>(
        &'a self,
        writer: &'a mut SessionWriter,
        state: &'a mut SessionState,
        provider: &'a dyn ModelProvider,
        model: &'a str,
        prompt: &'a str,
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
        writer: &'a mut SessionWriter,
        state: &'a SessionState,
        prompt: &'a str,
    ) -> BoxFuture<'a, Result<TurnOutcome, LoopError>>;
}

impl AgentDriver for AgentLoop {
    fn run_turn<'a>(
        &'a self,
        ctx: &'a Ctx,
        writer: &'a mut SessionWriter,
        state: &'a SessionState,
        prompt: &'a str,
    ) -> BoxFuture<'a, Result<TurnOutcome, LoopError>> {
        Box::pin(self.run_turn_impl(ctx, writer, state, prompt))
    }
}

/// Rebuilds provider-facing history from the durable projection.
#[derive(Default)]
struct HistoryBuilder {
    messages: Vec<ChatMessage>,
    pending_text: Option<(MessageId, String)>,
    pending_calls: Vec<ToolCall>,
    calls_for: Option<MessageId>,
}

impl HistoryBuilder {
    fn from_state(state: &SessionState) -> Vec<ChatMessage> {
        let mut builder = Self::default();
        if let Some(prompt) = &state.system_prompt {
            builder.messages.push(ChatMessage::System(prompt.clone()));
        }
        if let Some(compaction) = &state.compaction {
            builder.messages.push(ChatMessage::System(format!(
                "# 历史压缩摘要\n\n{}",
                compaction.summary
            )));
        }
        for record in &state.messages {
            if state
                .compaction
                .as_ref()
                .is_some_and(|compaction| record.seq() <= compaction.through_seq)
            {
                continue;
            }
            builder.push(record);
        }
        builder.finish()
    }

    fn push(&mut self, record: &MessageRecord) {
        match record {
            MessageRecord::User { text, .. } => {
                self.flush();
                self.messages.push(ChatMessage::User(text.clone()));
            }
            MessageRecord::Assistant { id, text, .. } => {
                self.flush();
                self.pending_text = Some((*id, text.clone()));
            }
            MessageRecord::ToolCall {
                message_id,
                id,
                name,
                input,
                ..
            } => {
                if self.calls_for != Some(*message_id) {
                    self.flush();
                    self.calls_for = Some(*message_id);
                }
                self.pending_calls.push(ToolCall {
                    id: id.to_string(),
                    name: name.clone(),
                    arguments: input.clone(),
                });
            }
            MessageRecord::ToolResult { id, content, .. } => {
                self.flush();
                self.messages.push(ChatMessage::ToolResult {
                    call_id: id.to_string(),
                    content: content.clone(),
                });
            }
        }
    }

    fn flush(&mut self) {
        if self.pending_calls.is_empty() {
            if let Some((_, text)) = self.pending_text.take() {
                self.messages.push(ChatMessage::Assistant(text));
            }
            return;
        }

        let text = match (self.pending_text.take(), self.calls_for.take()) {
            (Some((id, text)), Some(caller)) if id == caller => text,
            (Some((_, text)), _) => {
                self.messages.push(ChatMessage::Assistant(text));
                String::new()
            }
            _ => String::new(),
        };
        let calls = std::mem::take(&mut self.pending_calls);
        self.messages
            .push(ChatMessage::AssistantToolCalls { text, calls });
    }

    fn finish(mut self) -> Vec<ChatMessage> {
        self.flush();
        self.messages
    }
}
