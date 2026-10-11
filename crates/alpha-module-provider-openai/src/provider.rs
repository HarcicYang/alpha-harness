//! OpenAI chat-completions provider.

use std::collections::VecDeque;
use std::sync::Arc;

use alpha_module_http::{HttpClient, HttpRequest, HttpResponseStream};
use alpha_module_loop::{
    ChatMessage, CompletionChunk, CompletionRequest, CompletionResponse, ModelProvider,
    ProviderError, ToolCall, ToolCallDelta,
};
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
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

    fn request_body(
        &self,
        request: &CompletionRequest,
        stream: bool,
    ) -> Result<Vec<u8>, ProviderError> {
        let messages: Vec<Value> = request.messages.iter().map(wire_message).collect();
        let tools: Vec<Value> = request.tools.iter().map(wire_tool).collect();
        let mut payload = serde_json::Map::new();
        payload.insert("model".to_owned(), json!(request.model));
        payload.insert("messages".to_owned(), Value::Array(messages));
        if !tools.is_empty() {
            payload.insert("tools".to_owned(), Value::Array(tools));
        }
        payload.insert("stream".to_owned(), json!(stream));
        serde_json::to_vec(&Value::Object(payload))
            .map_err(|error| ProviderError::Request(error.to_string()))
    }
}

impl ModelProvider for OpenAiProvider {
    fn complete(
        &self,
        request: CompletionRequest,
    ) -> BoxFuture<'_, Result<CompletionResponse, ProviderError>> {
        Box::pin(async move {
            let body = self.request_body(&request, false)?;
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
                blocks: Vec::new(),
            })
        })
    }

    fn stream(
        self: Arc<Self>,
        request: CompletionRequest,
    ) -> BoxStream<'static, Result<CompletionChunk, ProviderError>> {
        let body = match self.request_body(&request, true) {
            Ok(body) => body,
            Err(error) => return Box::pin(futures::stream::iter(vec![Err(error)])),
        };
        let client = self.client.clone();
        let endpoint = self.endpoint();
        let api_key = self.api_key.clone();
        let initialize = async move {
            let http_request = HttpRequest::post(endpoint, body)
                .with_header("content-type", "application/json")
                .with_header("authorization", format!("Bearer {api_key}"));
            let response = client
                .request_stream(http_request)
                .await
                .map_err(|error| ProviderError::Request(error.to_string()))?;
            if !(200..300).contains(&response.status) {
                let body = collect_body(response.body).await?;
                return Err(ProviderError::Request(format!(
                    "status {}: {}",
                    response.status,
                    snippet(&body)
                )));
            }
            let is_event_stream = response
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
                .is_some_and(|(_, value)| value.to_ascii_lowercase().contains("text/event-stream"));
            if is_event_stream {
                Ok(StreamInit::Events(response))
            } else {
                let body = collect_body(response.body).await?;
                if looks_like_sse(&body) {
                    return Ok(StreamInit::Buffered(body));
                }
                let parsed: ChatResponse = serde_json::from_slice(&body).map_err(|error| {
                    ProviderError::Request(format!("invalid response: {error}"))
                })?;
                let message = parsed
                    .choices
                    .into_iter()
                    .next()
                    .map(|choice| choice.message)
                    .unwrap_or_default();
                Ok(StreamInit::Chunks(completion_chunks(message)?))
            }
        };

        Box::pin(
            futures::stream::once(initialize).flat_map(|result| match result {
                Ok(StreamInit::Events(response)) => Box::pin(futures::stream::unfold(
                    StreamState::new(response),
                    next_stream_chunk,
                ))
                    as BoxStream<'static, Result<CompletionChunk, ProviderError>>,
                Ok(StreamInit::Buffered(body)) => Box::pin(futures::stream::unfold(
                    StreamState::from_bytes(body),
                    next_stream_chunk,
                ))
                    as BoxStream<'static, Result<CompletionChunk, ProviderError>>,
                Ok(StreamInit::Chunks(chunks)) => Box::pin(futures::stream::iter(chunks))
                    as BoxStream<'static, Result<CompletionChunk, ProviderError>>,
                Err(error) => Box::pin(futures::stream::iter(vec![Err(error)]))
                    as BoxStream<'static, Result<CompletionChunk, ProviderError>>,
            }),
        )
    }
}

enum StreamInit {
    Events(HttpResponseStream),
    Buffered(Vec<u8>),
    Chunks(Vec<Result<CompletionChunk, ProviderError>>),
}

struct StreamState {
    body: BoxStream<'static, Result<Vec<u8>, alpha_module_http::HttpError>>,
    buffer: Vec<u8>,
    pending: VecDeque<Result<CompletionChunk, ProviderError>>,
    done: bool,
}

impl StreamState {
    fn new(response: HttpResponseStream) -> Self {
        Self {
            body: response.body,
            buffer: Vec::new(),
            pending: VecDeque::new(),
            done: false,
        }
    }

    fn from_bytes(bytes: Vec<u8>) -> Self {
        Self {
            body: Box::pin(futures::stream::once(async move { Ok(bytes) })),
            buffer: Vec::new(),
            pending: VecDeque::new(),
            done: false,
        }
    }
}

fn looks_like_sse(body: &[u8]) -> bool {
    let body = trim_ascii(body);
    body.starts_with(b"data:") || body.windows(6).any(|window| window == b"\ndata:")
}

async fn next_stream_chunk(
    mut state: StreamState,
) -> Option<(Result<CompletionChunk, ProviderError>, StreamState)> {
    loop {
        if let Some(chunk) = state.pending.pop_front() {
            return Some((chunk, state));
        }
        if state.done {
            return None;
        }
        match state.body.next().await {
            Some(Ok(bytes)) => {
                state.buffer.extend_from_slice(&bytes);
                if let Err(error) = drain_events(
                    &mut state.buffer,
                    &mut state.pending,
                    &mut state.done,
                    false,
                ) {
                    state.pending.push_back(Err(error));
                    state.done = true;
                }
            }
            Some(Err(error)) => {
                state.done = true;
                state
                    .pending
                    .push_back(Err(ProviderError::Request(error.to_string())));
            }
            None => {
                state.done = true;
                if let Err(error) =
                    drain_events(&mut state.buffer, &mut state.pending, &mut state.done, true)
                {
                    state.pending.push_back(Err(error));
                }
            }
        }
    }
}

async fn collect_body(
    mut body: BoxStream<'static, Result<Vec<u8>, alpha_module_http::HttpError>>,
) -> Result<Vec<u8>, ProviderError> {
    let mut output = Vec::new();
    while let Some(chunk) = body.next().await {
        output
            .extend_from_slice(&chunk.map_err(|error| ProviderError::Request(error.to_string()))?);
    }
    Ok(output)
}

fn completion_chunks(
    message: ResponseMessage,
) -> Result<Vec<Result<CompletionChunk, ProviderError>>, ProviderError> {
    let mut chunks = Vec::new();
    if let Some(text) = message.content {
        if !text.is_empty() {
            chunks.push(Ok(CompletionChunk::Text(text)));
        }
    }
    for (index, call) in message
        .tool_calls
        .unwrap_or_default()
        .into_iter()
        .enumerate()
    {
        let arguments = if call.function.arguments.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&call.function.arguments).map_err(|error| {
                ProviderError::Request(format!("invalid tool arguments: {error}"))
            })?
        };
        chunks.push(Ok(CompletionChunk::ToolCall(ToolCallDelta {
            index,
            id: Some(call.id),
            name: Some(call.function.name),
            arguments: serde_json::to_string(&arguments).unwrap_or_else(|_| "null".to_owned()),
        })));
    }
    Ok(chunks)
}

fn drain_events(
    buffer: &mut Vec<u8>,
    pending: &mut VecDeque<Result<CompletionChunk, ProviderError>>,
    done: &mut bool,
    finish: bool,
) -> Result<(), ProviderError> {
    loop {
        let Some(index) = buffer.iter().position(|byte| *byte == b'\n') else {
            break;
        };
        let mut line: Vec<u8> = buffer.drain(..=index).collect();
        line.pop();
        process_event_line(&line, pending, done)?;
        if *done {
            buffer.clear();
            return Ok(());
        }
    }
    if finish && !buffer.is_empty() {
        let line = std::mem::take(buffer);
        process_event_line(&line, pending, done)?;
    }
    Ok(())
}

fn process_event_line(
    line: &[u8],
    pending: &mut VecDeque<Result<CompletionChunk, ProviderError>>,
    done: &mut bool,
) -> Result<(), ProviderError> {
    let line = trim_ascii(line);
    if line.is_empty() || line.starts_with(b":") {
        return Ok(());
    }
    let Some(data) = line.strip_prefix(b"data:") else {
        return Ok(());
    };
    let data = trim_ascii(data.strip_prefix(b" ").unwrap_or(data));
    if data == b"[DONE]" {
        *done = true;
        return Ok(());
    }
    let response: StreamResponse = serde_json::from_slice(data)
        .map_err(|error| ProviderError::Request(format!("invalid stream event: {error}")))?;
    if let Some(error) = response.error {
        return Err(ProviderError::Request(format!("stream error: {error}")));
    }
    for choice in response.choices {
        if let Some(text) = choice.delta.content {
            if !text.is_empty() {
                pending.push_back(Ok(CompletionChunk::Text(text)));
            }
        }
        for call in choice.delta.tool_calls.unwrap_or_default() {
            pending.push_back(Ok(CompletionChunk::ToolCall(ToolCallDelta {
                index: call.index,
                id: call.id,
                name: call
                    .function
                    .as_ref()
                    .and_then(|function| function.name.clone()),
                arguments: call
                    .function
                    .and_then(|function| function.arguments)
                    .unwrap_or_default(),
            })));
        }
    }
    Ok(())
}

fn trim_ascii(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(|byte| byte.is_ascii_whitespace()) {
        value = &value[1..];
    }
    while value.last().is_some_and(|byte| byte.is_ascii_whitespace()) {
        value = &value[..value.len() - 1];
    }
    value
}

fn wire_message(message: &ChatMessage) -> Value {
    match message {
        ChatMessage::System(text) => json!({ "role": "system", "content": text }),
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

#[derive(Default, Deserialize)]
struct StreamResponse {
    #[serde(default)]
    choices: Vec<StreamChoice>,
    #[serde(default)]
    error: Option<Value>,
}

#[derive(Default, Deserialize)]
struct StreamChoice {
    #[serde(default)]
    delta: StreamDelta,
}

#[derive(Default, Deserialize)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<StreamToolCall>>,
}

#[derive(Default, Deserialize)]
struct StreamToolCall {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<StreamFunction>,
}

#[derive(Default, Deserialize)]
struct StreamFunction {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
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
