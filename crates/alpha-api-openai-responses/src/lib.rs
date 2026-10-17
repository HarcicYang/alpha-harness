//! OpenAI Responses API adapter.

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
pub const ADAPTER_KEY: &str = "openai-responses";

/// Stable module key used by the generated composition table.
pub const __ALPHA_MODULE__: &str = "api.openai-responses";

/// OpenAI Responses adapter.
pub struct OpenAiResponsesAdapter;

impl ApiAdapter for OpenAiResponsesAdapter {
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
        let endpoint = format!("{}/responses", request.base_url.trim_end_matches('/'));
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
    let input: Vec<Value> = request.messages.iter().flat_map(wire_message).collect();
    let tools: Vec<Value> = request.tools.iter().map(wire_tool).collect();
    let mut payload = serde_json::Map::new();
    if let Value::Object(options) = &request.options {
        for (key, value) in options {
            payload.insert(key.clone(), value.clone());
        }
    }
    payload.insert("model".to_owned(), json!(request.model));
    payload.insert("input".to_owned(), Value::Array(input));
    payload.insert("stream".to_owned(), json!(true));
    payload.insert("store".to_owned(), json!(false));
    payload.insert("include".to_owned(), json!(["reasoning.encrypted_content"]));
    if !tools.is_empty() {
        payload.insert("tools".to_owned(), Value::Array(tools));
    }
    if let Some(effort) = request.reasoning.api_effort() {
        payload.insert("reasoning".to_owned(), json!({ "effort": effort }));
    }
    serde_json::to_vec(&Value::Object(payload))
        .map_err(|error| ProviderError::Request(error.to_string()))
}

fn wire_message(message: &ChatMessage) -> Vec<Value> {
    match message {
        ChatMessage::System(text) => vec![json!({
            "role": "system",
            "content": [{ "type": "input_text", "text": text }],
        })],
        ChatMessage::User(text) => vec![json!({
            "role": "user",
            "content": [{ "type": "input_text", "text": text }],
        })],
        ChatMessage::Assistant(text) => vec![json!({
            "type": "message",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": text, "annotations": [] }],
        })],
        ChatMessage::AssistantToolCalls { text, calls } => {
            let mut items = Vec::new();
            if !text.is_empty() {
                items.push(json!({
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": text, "annotations": [] }],
                }));
            }
            for call in calls {
                items.push(json!({
                    "type": "function_call",
                    "call_id": call.id,
                    "name": call.name,
                    "arguments": serde_json::to_string(&call.arguments)
                        .unwrap_or_else(|_| "null".to_owned()),
                }));
            }
            items
        }
        ChatMessage::AssistantContent(blocks) => assistant_content_items(blocks),
        ChatMessage::ToolResult { call_id, content } => vec![json!({
            "type": "function_call_output",
            "call_id": call_id,
            "output": content,
        })],
    }
}

fn assistant_content_items(blocks: &[alpha_schema::AssistantContentBlock]) -> Vec<Value> {
    let mut items = Vec::new();
    for block in blocks {
        match block {
            alpha_schema::AssistantContentBlock::Text { text } => items.push(json!({
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": text, "annotations": [] }],
            })),
            alpha_schema::AssistantContentBlock::Thinking { text, signature } => {
                let mut item = serde_json::Map::new();
                item.insert("type".to_owned(), json!("reasoning"));
                if !text.is_empty() {
                    item.insert(
                        "summary".to_owned(),
                        json!([{ "type": "summary_text", "text": text }]),
                    );
                }
                if let Some(signature) = signature {
                    item.insert("encrypted_content".to_owned(), json!(signature));
                }
                items.push(Value::Object(item));
            }
            alpha_schema::AssistantContentBlock::LocalToolCall {
                id,
                name,
                arguments,
            } => items.push(json!({
                "type": "function_call",
                "call_id": id,
                "name": name,
                "arguments": serde_json::to_string(arguments)
                    .unwrap_or_else(|_| "null".to_owned()),
            })),
            _ => {}
        }
    }
    items
}

fn wire_tool(tool: &ToolDefinition) -> Value {
    json!({
        "type": "function",
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.input_schema,
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
    let event: StreamEvent = serde_json::from_slice(data)
        .map_err(|error| ProviderError::Request(format!("invalid stream event: {error}")))?;
    match event.kind.as_str() {
        "response.output_text.delta" => {
            if let Some(delta) = event.delta {
                if !delta.is_empty() {
                    pending.push_back(Ok(CompletionChunk::Text(delta)));
                }
            }
        }
        "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
            if let Some(delta) = event.delta
                && !delta.is_empty()
            {
                pending.push_back(Ok(CompletionChunk::Thinking {
                    text: delta,
                    signature: None,
                }));
            }
        }
        "response.output_item.added" => {
            if let Some(item) = event.item {
                if item.kind.as_deref() == Some("function_call") {
                    pending.push_back(Ok(CompletionChunk::ToolCall(ToolCallDelta {
                        index: event.output_index.unwrap_or_default(),
                        id: item.call_id.or(item.id),
                        name: item.name,
                        arguments: item.arguments.unwrap_or_default(),
                    })));
                }
            }
        }
        "response.function_call_arguments.delta" => {
            if let Some(delta) = event.delta {
                pending.push_back(Ok(CompletionChunk::ToolCall(ToolCallDelta {
                    index: event.output_index.unwrap_or_default(),
                    id: None,
                    name: None,
                    arguments: delta,
                })));
            }
        }
        "response.output_item.done" => {
            if let Some(item) = event.item
                && item.kind.as_deref() == Some("reasoning")
                && let Some(signature) = item.encrypted_content
                && !signature.is_empty()
            {
                pending.push_back(Ok(CompletionChunk::Thinking {
                    text: String::new(),
                    signature: Some(signature),
                }));
            }
        }
        "response.completed" => {
            *done = true;
        }
        "error" => {
            return Err(ProviderError::Request(format!(
                "stream error: {}",
                event
                    .error
                    .unwrap_or_else(|| json!({ "message": "unknown" }))
            )));
        }
        _ => {}
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
struct StreamEvent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    delta: Option<String>,
    #[serde(default)]
    output_index: Option<usize>,
    #[serde(default)]
    item: Option<StreamItem>,
    #[serde(default)]
    error: Option<Value>,
}

#[derive(Default, Deserialize)]
struct StreamItem {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    call_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
    #[serde(default)]
    encrypted_content: Option<String>,
}

fn snippet(body: &[u8]) -> String {
    String::from_utf8_lossy(body).chars().take(400).collect()
}

/// Registers the OpenAI Responses adapter.
pub struct OpenAiResponsesApiModule;

impl Module for OpenAiResponsesApiModule {
    type Requirements = ();
    type Config = ();

    const KEY: &'static str = "api.openai-responses";

    fn activate(&self, ctx: &Ctx, _config: &Self::Config) -> Result<(), ActivateError> {
        let registry = ctx
            .require::<ApiAdapterRegistry>()
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        let adapter: Arc<dyn ApiAdapter> = Arc::new(OpenAiResponsesAdapter);
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
