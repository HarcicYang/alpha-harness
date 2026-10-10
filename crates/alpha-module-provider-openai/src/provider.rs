//! OpenAI chat-completions provider.

use std::sync::Arc;

use alpha_module_http::{HttpClient, HttpRequest};
use alpha_module_loop::{
    ChatMessage, CompletionRequest, CompletionResponse, ModelProvider, ProviderError, ToolCall,
};
use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};

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
            let messages: Vec<Value> = request.messages.iter().map(wire_message).collect();
            let tools: Vec<Value> = request.tools.iter().map(wire_tool).collect();
            let mut payload = serde_json::Map::new();
            payload.insert("model".to_owned(), json!(request.model));
            payload.insert("messages".to_owned(), Value::Array(messages));
            if !tools.is_empty() {
                payload.insert("tools".to_owned(), Value::Array(tools));
            }
            payload.insert("stream".to_owned(), json!(false));
            let payload = Value::Object(payload);
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
            let message = parsed
                .choices
                .into_iter()
                .next()
                .map(|choice| choice.message)
                .unwrap_or_default();
            let mut tool_calls = Vec::new();
            for call in message.tool_calls.unwrap_or_default() {
                let arguments = if call.function.arguments.trim().is_empty() {
                    Value::Null
                } else {
                    serde_json::from_str(&call.function.arguments).map_err(|error| {
                        ProviderError::Request(format!("invalid tool arguments: {error}"))
                    })?
                };
                tool_calls.push(ToolCall {
                    id: call.id,
                    name: call.function.name,
                    arguments,
                });
            }
            Ok(CompletionResponse {
                text: message.content.unwrap_or_default(),
                tool_calls,
            })
        })
    }
}

fn wire_message(message: &ChatMessage) -> Value {
    match message {
        ChatMessage::User(text) => json!({ "role": "user", "content": text }),
        ChatMessage::Assistant(text) => json!({ "role": "assistant", "content": text }),
        ChatMessage::AssistantToolCalls { text, calls } => json!({
            "role": "assistant",
            "content": if text.is_empty() { Value::Null } else { json!(text) },
            "tool_calls": calls
                .iter()
                .map(|call| json!({
                    "id": call.id,
                    "type": "function",
                    "function": {
                        "name": call.name,
                        "arguments": serde_json::to_string(&call.arguments)
                            .unwrap_or_else(|_| "null".to_owned()),
                    }
                }))
                .collect::<Vec<_>>(),
        }),
        ChatMessage::ToolResult { call_id, content } => json!({
            "role": "tool",
            "tool_call_id": call_id,
            "content": content,
        }),
    }
}

fn wire_tool(tool: &alpha_module_loop::ToolDefinition) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.input_schema,
        }
    })
}

#[derive(Default, Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Default, Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<WireToolCall>>,
}

#[derive(Deserialize)]
struct WireToolCall {
    id: String,
    function: WireFunction,
}

#[derive(Deserialize)]
struct WireFunction {
    name: String,
    #[serde(default)]
    arguments: String,
}

fn snippet(body: &[u8]) -> String {
    String::from_utf8_lossy(body).chars().take(400).collect()
}
