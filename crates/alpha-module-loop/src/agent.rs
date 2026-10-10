//! Default agent driver.

use std::sync::Arc;

use alpha_module_session::{
    MessageRecord, SessionError, SessionEvent, SessionState, SessionWriter, TurnEndReason,
};
use alpha_schema::MessageId;
use thiserror::Error;

use crate::provider::{ChatMessage, CompletionRequest, ModelProvider, ProviderError};

/// Loop failure.
#[derive(Debug, Error)]
pub enum LoopError {
    /// Session storage failure.
    #[error(transparent)]
    Session(#[from] SessionError),
    /// Provider failure.
    #[error(transparent)]
    Provider(#[from] ProviderError),
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
}

impl AgentLoop {
    /// Create a loop around one provider.
    pub fn new(provider: Arc<dyn ModelProvider>, model: impl Into<String>) -> Self {
        Self {
            provider,
            model: model.into(),
        }
    }

    /// The configured model identifier.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Run one turn and record it in the session log.
    ///
    /// `state` is the projection before this turn. On provider failure the
    /// turn is closed as failed and the error is returned.
    pub async fn run_turn(
        &self,
        writer: &mut SessionWriter,
        state: &SessionState,
        prompt: &str,
    ) -> Result<TurnOutcome, LoopError> {
        writer.append(SessionEvent::TurnStarted)?;
        let user_id = MessageId::new();
        writer.append(SessionEvent::UserMessage {
            id: user_id,
            text: prompt.to_owned(),
        })?;

        let mut messages: Vec<ChatMessage> = state
            .messages
            .iter()
            .map(|record| match record {
                MessageRecord::User { text, .. } => ChatMessage::User(text.clone()),
                MessageRecord::Assistant { text, .. } => ChatMessage::Assistant(text.clone()),
            })
            .collect();
        messages.push(ChatMessage::User(prompt.to_owned()));

        let request = CompletionRequest {
            model: self.model.clone(),
            messages,
        };

        match self.provider.complete(request).await {
            Ok(response) => {
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
            Err(error) => {
                writer.append(SessionEvent::TurnEnded {
                    reason: TurnEndReason::Failed,
                })?;
                Err(LoopError::Provider(error))
            }
        }
    }
}
