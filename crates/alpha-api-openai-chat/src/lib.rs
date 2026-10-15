//! OpenAI Chat Completions API adapter.

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::sync::Arc;

use alpha_kernel::{ActivateError, Ctx, Module};
use alpha_module_http::{HttpClient, HttpRequest, HttpResponseStream};
use alpha_module_loop::{
    ChatMessage, CompletionChunk, ProviderError, ToolCallDelta, ToolDefinition,
};
use alpha_provider_api::{AdapterCapabilities, AdapterRequest, ApiAdapter, ApiAdapterRegistry};
use futures::StreamExt;
use futures::stream::BoxStream;
use serde::Deserialize;
use serde_json::{Value, json};

/// Stable adapter key.
pub const ADAPTER_KEY: &str = "openai-chat-completions";

/// Stable module key used by the generated composition table.
pub const __ALPHA_MODULE__: &str = "api.openai-chat";

/// OpenAI Chat Completions adapter.
pub struct OpenAiChatAdapter;

impl ApiAdapter for OpenAiChatAdapter {
    fn key(&self) -> &'static str {
        ADAPTER_KEY
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            streaming: true,
            tool_calls: true,
            provider_tools: false,
        }
    }

    fn stream(
        &self,
        client: Arc<dyn HttpClient>,
        request: AdapterRequest,
    ) -> BoxStream<'static, Result<CompletionChunk, ProviderError>> {
        let body = match request_body(&request) {
            Ok(body) => body,
            Err(error) => return Box::pin(futures::stream::iter(vec![Err(error)])),
        };
        let endpoint = format!(
            "{}/chat/completions",
            request.base_url.trim_end_matches('/')
        );
        let api_key = request.api_key.clone();
        let headers = request.headers.clone();
        let initialize = async move {
            let mut http_request =
                HttpRequest::post(endpoint, body).with_header("content-type", "application/json");
            if let Some(api_key) = api_key {
                http_request =
                    http_request.with_header("authorization", format!("Bearer {api_key}"));
            }
            for (name, value) in headers {
                http_request = http_request.with_header(name, value);
            }
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
            Ok(response)
        };
        Box::pin(
            futures::stream::once(initialize).flat_map(|result| match result {
                Ok(response) => Box::pin(futures::stream::unfold(
                    StreamState::new(response),
                    next_stream_chunk,
                ))
                    as BoxStream<'static, Result<CompletionChunk, ProviderError>>,
                Err(error) => Box::pin(futures::stream::iter(vec![Err(error)]))
                    as BoxStream<'static, Result<CompletionChunk, ProviderError>>,
            }),
        )
    }
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

fn request_body(request: &AdapterRequest) -> Result<Vec<u8>, ProviderError> {
    let messages: Vec<Value> = request.messages.iter().map(wire_message).collect();
    let tools: Vec<Value> = request.tools.iter().map(wire_tool).collect();
    let mut payload = serde_json::Map::new();
    payload.insert("model".to_owned(), json!(request.model));
    payload.insert("messages".to_owned(), Value::Array(messages));
    if !tools.is_empty() {
        payload.insert("tools".to_owned(), Value::Array(tools));
    }
    if let Some(effort) = request.reasoning.api_effort() {
        payload.insert("reasoning_effort".to_owned(), json!(effort));
    }
    payload.insert("stream".to_owned(), json!(true));
    if let Value::Object(options) = &request.options {
        for (key, value) in options {
            payload.insert(key.clone(), value.clone());
        }
    }
    serde_json::to_vec(&Value::Object(payload))
        .map_err(|error| ProviderError::Request(error.to_string()))
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
        ChatMessage::AssistantContent(blocks) => assistant_content_message(blocks),
        ChatMessage::ToolResult { call_id, content } => json!({
            "role": "tool",
            "tool_call_id": call_id,
            "content": content,
        }),
    }
}

fn assistant_content_message(blocks: &[alpha_schema::AssistantContentBlock]) -> Value {
    let text = blocks
        .iter()
        .filter_map(|block| match block {
            alpha_schema::AssistantContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    let reasoning = blocks
        .iter()
        .filter_map(|block| match block {
            alpha_schema::AssistantContentBlock::Thinking { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    let calls = blocks
        .iter()
        .filter_map(|block| match block {
            alpha_schema::AssistantContentBlock::LocalToolCall {
                id,
                name,
                arguments,
            } => Some(json!({
                "id": id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": serde_json::to_string(arguments)
                        .unwrap_or_else(|_| "null".to_owned()),
                }
            })),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut message = serde_json::Map::new();
    message.insert("role".to_owned(), json!("assistant"));
    message.insert(
        "content".to_owned(),
        if text.is_empty() {
            Value::Null
        } else {
            json!(text)
        },
    );
    if !reasoning.is_empty() {
        message.insert("reasoning_content".to_owned(), json!(reasoning));
    }
    if !calls.is_empty() {
        message.insert("tool_calls".to_owned(), Value::Array(calls));
    }
    Value::Object(message)
}

fn wire_tool(tool: &ToolDefinition) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.input_schema,
        }
    })
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
        if let Some(reasoning) = [
            choice.delta.reasoning_content,
            choice.delta.reasoning,
            choice.delta.reasoning_text,
        ]
        .into_iter()
        .flatten()
        .find(|reasoning| !reasoning.is_empty())
        {
            pending.push_back(Ok(CompletionChunk::Thinking {
                text: reasoning,
                signature: None,
            }));
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
    reasoning_content: Option<String>,
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    reasoning_text: Option<String>,
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

fn snippet(body: &[u8]) -> String {
    String::from_utf8_lossy(body).chars().take(400).collect()
}

/// Registers the OpenAI Chat adapter.
pub struct OpenAiChatApiModule;

impl Module for OpenAiChatApiModule {
    type Requirements = ();
    type Config = ();

    const KEY: &'static str = "api.openai-chat";

    fn activate(&self, ctx: &Ctx, _config: &Self::Config) -> Result<(), ActivateError> {
        let registry = ctx
            .require::<ApiAdapterRegistry>()
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        let adapter: Arc<dyn ApiAdapter> = Arc::new(OpenAiChatAdapter);
        registry
            .register(adapter)
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        let cleanup = registry.clone();
        ctx.scope()
            .effect(move || {
                cleanup.unregister(ADAPTER_KEY);
            })
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        Ok(())
    }
}
