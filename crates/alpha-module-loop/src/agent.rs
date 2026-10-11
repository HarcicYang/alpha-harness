//! Default agent driver.

use std::sync::Arc;

use alpha_module_session::{
    MessageRecord, SessionError, SessionEvent, SessionState, SessionWriter, ToolStatus,
    TurnEndReason,
};
use alpha_module_tools::{ToolContext, ToolError, ToolRegistry};
use alpha_schema::{MessageId, ToolCallId};
use futures::future::BoxFuture;
use serde_json::Value;
use thiserror::Error;

use crate::provider::{
    ChatMessage, CompletionRequest, ModelProvider, ProviderError, ToolCall, ToolDefinition,
};

const DEFAULT_MAX_STEPS: usize = 24;

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
    observer: Option<Arc<dyn TurnObserver>>,
    max_steps: usize,
}

impl AgentLoop {
    /// Create a loop around one provider.
    pub fn new(provider: Arc<dyn ModelProvider>, model: impl Into<String>) -> Self {
        Self {
            provider,
            model: model.into(),
            tools: None,
            observer: None,
            max_steps: DEFAULT_MAX_STEPS,
        }
    }

    /// Override the maximum number of model steps in one turn.
    pub fn with_max_steps(mut self, max_steps: usize) -> Self {
        self.max_steps = max_steps.max(1);
        self
    }

    /// Attach a progress observer.
    pub fn with_observer(mut self, observer: Arc<dyn TurnObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Attach a tool registry.
    pub fn with_tools(mut self, tools: Arc<ToolRegistry>) -> Self {
        self.tools = Some(tools);
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
        writer: &mut SessionWriter,
        state: &SessionState,
        prompt: &str,
    ) -> Result<TurnOutcome, LoopError> {
        self.run_turn_impl(writer, state, prompt).await
    }

    async fn run_turn_impl(
        &self,
        writer: &mut SessionWriter,
        state: &SessionState,
        prompt: &str,
    ) -> Result<TurnOutcome, LoopError> {
        let session_id = writer.header().id;
        writer.append(SessionEvent::TurnStarted)?;
        let user_id = MessageId::new();
        writer.append(SessionEvent::UserMessage {
            id: user_id,
            text: prompt.to_owned(),
        })?;

        let mut messages = HistoryBuilder::from_state(state);
        messages.push(ChatMessage::User(prompt.to_owned()));

        let tools = self.tool_definitions();
        let mut last_text = String::new();
        let mut last_message_id: Option<MessageId> = None;

        for step in 0..self.max_steps {
            if let Some(observer) = &self.observer {
                observer.step_started(step + 1);
            }
            let request = CompletionRequest {
                model: self.model.clone(),
                messages: messages.clone(),
                tools: tools.clone(),
            };
            let response = match self.provider.complete(request).await {
                Ok(response) => response,
                Err(error) => {
                    writer.append(SessionEvent::TurnEnded {
                        reason: TurnEndReason::Failed,
                    })?;
                    return Err(LoopError::Provider(error));
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
                self.execute_tool(writer, session_id, message_id, &mut messages, call)
                    .await?;
            }
        }

        // Wrap up without tools so the user still gets a summary.
        let request = CompletionRequest {
            model: self.model.clone(),
            messages: messages.clone(),
            tools: Vec::new(),
        };
        match self.provider.complete(request).await {
            Ok(response) if !response.text.is_empty() => {
                let assistant_id = MessageId::new();
                writer.append(SessionEvent::AssistantMessage {
                    id: assistant_id,
                    text: response.text.clone(),
                })?;
                writer.append(SessionEvent::TurnEnded {
                    reason: TurnEndReason::Completed,
                })?;
                Ok(TurnOutcome {
                    assistant_message: assistant_id,
                    assistant_text: response.text,
                })
            }
            _ => {
                writer.append(SessionEvent::TurnEnded {
                    reason: TurnEndReason::Failed,
                })?;
                Err(LoopError::StepLimit(self.max_steps))
            }
        }
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

    async fn execute_tool(
        &self,
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

        if let Some(observer) = &self.observer {
            observer.tool_started(&call.name, &call.arguments);
        }

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
                return Err(LoopError::Tool(message));
            }
        };
        if let Some(observer) = &self.observer {
            observer.tool_finished(&call.name, status, &content);
        }
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

/// Progress observer for one turn. Implementations must not block.
pub trait TurnObserver: Send + Sync + 'static {
    /// A model step is about to start.
    fn step_started(&self, step: usize);
    /// A tool call is about to execute.
    fn tool_started(&self, name: &str, input: &Value);
    /// A tool call settled.
    fn tool_finished(&self, name: &str, status: ToolStatus, content: &str);
}

/// Replaceable driver contract for one agent turn.
///
/// Consumers depend on this trait, so a composition can mount a different
/// driver under the same service key.
pub trait AgentDriver: Send + Sync + 'static {
    /// Run one turn and record it in the session log.
    fn run_turn<'a>(
        &'a self,
        writer: &'a mut SessionWriter,
        state: &'a SessionState,
        prompt: &'a str,
    ) -> BoxFuture<'a, Result<TurnOutcome, LoopError>>;
}

impl AgentDriver for AgentLoop {
    fn run_turn<'a>(
        &'a self,
        writer: &'a mut SessionWriter,
        state: &'a SessionState,
        prompt: &'a str,
    ) -> BoxFuture<'a, Result<TurnOutcome, LoopError>> {
        Box::pin(self.run_turn_impl(writer, state, prompt))
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
        for record in &state.messages {
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
            MessageRecord::Assistant { id, text } => {
                self.flush();
                self.pending_text = Some((*id, text.clone()));
            }
            MessageRecord::ToolCall {
                message_id,
                id,
                name,
                input,
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
