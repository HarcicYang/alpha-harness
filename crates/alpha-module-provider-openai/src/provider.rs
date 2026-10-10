//! OpenAI chat-completions provider.

use std::sync::Arc;

use alpha_module_http::{HttpClient, HttpRequest};
use alpha_module_loop::{CompletionRequest, CompletionResponse, ModelProvider, ProviderError};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};

/// Provider name registered in the loop provider registry.
pub const PROVIDER_NAME: &str = "openai-compatible";

/// OpenAI-compatible provider.
pub struct OpenAiProvider {
    client: Arc<dyn HttpClient>,
    base_url: String,
    api_key: String,
}

impl OpenAiProvider {
    /// Create a provider over an HTTP seam.
    pub fn new(
        client: Arc<dyn HttpClient>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            client,
            base_url: base_url.into(),
            api_key: api_key.into(),
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }
}

impl ModelProvider for OpenAiProvider {
    fn complete(
        &self,
        request: CompletionRequest,
    ) -> BoxFuture<'_, Result<CompletionResponse, ProviderError>> {
        Box::pin(async move {
            let payload = ChatRequest {
                model: request.model,
                messages: request
                    .messages
                    .iter()
                    .map(|message| ChatMessage {
                        role: message.role(),
                        content: message.text(),
                    })
                    .collect(),
                stream: false,
            };
            let body = serde_json::to_vec(&payload)
                .map_err(|error| ProviderError::Request(error.to_string()))?;
            let http_request = HttpRequest::post(self.endpoint(), body)
                .with_header("content-type", "application/json")
                .with_header("authorization", format!("Bearer {}", self.api_key));
            let response = self
                .client
                .request(http_request)
                .await
                .map_err(|error| ProviderError::Request(error.to_string()))?;

            if !response.is_success() {
                return Err(ProviderError::Request(format!(
                    "status {}: {}",
                    response.status,
                    snippet(&response.body)
                )));
            }

            let parsed: ChatResponse = serde_json::from_slice(&response.body)
                .map_err(|error| ProviderError::Request(format!("invalid response: {error}")))?;
            let text = parsed
                .choices
                .into_iter()
                .next()
                .and_then(|choice| choice.message.content)
                .unwrap_or_default();
            Ok(CompletionResponse { text })
        })
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: String,
    messages: Vec<ChatMessage<'a>>,
    stream: bool,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: Option<String>,
}

fn snippet(body: &[u8]) -> String {
    String::from_utf8_lossy(body).chars().take(400).collect()
}
