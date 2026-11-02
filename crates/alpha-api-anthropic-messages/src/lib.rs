//! Anthropic Messages API adapter.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use alpha_http_api::{HttpClient, HttpRequest, HttpResponseStream};
use alpha_kernel::{ActivateError, Ctx, Module};
use alpha_module_loop::{
    ChatMessage, CompletionChunk, ProviderError, ToolCallDelta, ToolDefinition,
};
use alpha_provider_api::{
    AdapterCapabilities, AdapterRequest, ApiAdapter, ApiAdapterRegistry, ReasoningLevel,
};
use futures::StreamExt;
use futures::stream::BoxStream;
use serde::Deserialize;
use serde_json::{Value, json};

/// Stable adapter key.
pub const ADAPTER_KEY: &str = "anthropic-messages";

/// Stable module key used by the generated composition table.
pub const __ALPHA_MODULE__: &str = "api.anthropic-messages";

/// Anthropic Messages adapter.
pub struct AnthropicMessagesAdapter;

impl ApiAdapter for AnthropicMessagesAdapter {
    fn key(&self) -> &'static str {
        ADAPTER_KEY
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            streaming: true,
            tool_calls: true,
            provider_tools: true,
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
        let endpoint = messages_endpoint(&request.base_url);
        let api_key = request.api_key.clone();
        let headers = request.headers.clone();
        let initialize = async move {
            let mut http_request = HttpRequest::post(endpoint, body)
                .with_header("content-type", "application/json")
                .with_header("anthropic-version", "2023-06-01");
            if let Some(api_key) = api_key {
                http_request = http_request.with_header("x-api-key", api_key);
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
    body: BoxStream<'static, Result<Vec<u8>, alpha_http_api::HttpError>>,
    buffer: Vec<u8>,
    pending: VecDeque<Result<CompletionChunk, ProviderError>>,
    block_kinds: BTreeMap<usize, String>,
    usage: UsageState,
    done: bool,
}

/// Token counts reported across the Anthropic event stream.
#[derive(Clone, Copy, Debug, Default)]
struct UsageState {
    input_tokens: u64,
}

impl StreamState {
    fn new(response: HttpResponseStream) -> Self {
        Self {
            body: response.body,
            buffer: Vec::new(),
            pending: VecDeque::new(),
            block_kinds: BTreeMap::new(),
            usage: UsageState::default(),
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
                    &mut state.block_kinds,
                    &mut state.usage,
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
                if let Err(error) = drain_events(
                    &mut state.buffer,
                    &mut state.pending,
                    &mut state.block_kinds,
                    &mut state.usage,
                    &mut state.done,
                    true,
                ) {
                    state.pending.push_back(Err(error));
                }
            }
        }
    }
}

async fn collect_body(
    mut body: BoxStream<'static, Result<Vec<u8>, alpha_http_api::HttpError>>,
) -> Result<Vec<u8>, ProviderError> {
    let mut output = Vec::new();
    while let Some(chunk) = body.next().await {
        output
            .extend_from_slice(&chunk.map_err(|error| ProviderError::Request(error.to_string()))?);
    }
    Ok(output)
}

fn messages_endpoint(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/messages")
    } else {
        format!("{base}/v1/messages")
    }
}

fn request_body(request: &AdapterRequest) -> Result<Vec<u8>, ProviderError> {
    let mut system = Vec::new();
    let mut messages = Vec::new();
    let mut tool_results = Vec::new();
    for message in &request.messages {
        match message {
            ChatMessage::System(text) => system.push(text.clone()),
            ChatMessage::User(text) => {
                flush_tool_results(&mut messages, &mut tool_results);
                messages.push(json!({
                    "role": "user",
                    "content": [{ "type": "text", "text": text }],
                }));
            }
            ChatMessage::UserContent { text, images } => {
                flush_tool_results(&mut messages, &mut tool_results);
                let mut content = Vec::new();
                if !text.is_empty() {
                    content.push(json!({ "type": "text", "text": text }));
                }
                content.extend(images.iter().map(|image| {
                    json!({
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": image.mime_type,
                            "data": image.data,
                        },
                    })
                }));
                messages.push(json!({ "role": "user", "content": content }));
            }
            ChatMessage::Assistant(text) => {
                flush_tool_results(&mut messages, &mut tool_results);
                messages.push(json!({
                    "role": "assistant",
                    "content": [{ "type": "text", "text": text }],
                }));
            }
            ChatMessage::AssistantToolCalls { text, calls } => {
                flush_tool_results(&mut messages, &mut tool_results);
                let mut content = Vec::new();
                if !text.is_empty() {
                    content.push(json!({ "type": "text", "text": text }));
                }
                for call in calls {
                    content.push(json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.name,
                        "input": call.arguments,
                    }));
                }
                messages.push(json!({ "role": "assistant", "content": content }));
            }
            ChatMessage::AssistantContent(blocks) => {
                flush_tool_results(&mut messages, &mut tool_results);
                messages.push(json!({
                    "role": "assistant",
                    "content": wire_assistant_blocks(blocks),
                }));
            }
            ChatMessage::ToolResult {
                call_id,
                content,
                images,
            } => {
                let content = if images.is_empty() {
                    json!(content)
                } else {
                    let mut content_blocks = Vec::new();
                    if !content.is_empty() {
                        content_blocks.push(json!({ "type": "text", "text": content }));
                    }
                    content_blocks.extend(images.iter().map(|image| {
                        json!({
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": image.mime_type,
                                "data": image.data,
                            },
                        })
                    }));
                    Value::Array(content_blocks)
                };
                tool_results.push(json!({
                    "type": "tool_result",
                    "tool_use_id": call_id,
                    "content": content,
                }));
            }
        }
    }
    flush_tool_results(&mut messages, &mut tool_results);
    let tools = request
        .tools
        .iter()
        .map(wire_tool)
        .chain(
            request
                .provider_tools
                .iter()
                .filter(|tool| tool.as_str() == "web_search")
                .map(|_| {
                    json!({
                        "type": "web_search_20250305",
                        "name": "web_search",
                    })
                }),
        )
        .collect::<Vec<_>>();
    let mut payload = serde_json::Map::new();
    if let Value::Object(options) = &request.options {
        for (key, value) in options {
            payload.insert(key.clone(), value.clone());
        }
    }
    payload.insert("model".to_owned(), json!(request.model));
    payload.insert("messages".to_owned(), Value::Array(messages));
    payload.insert("stream".to_owned(), json!(true));
    if let Some(max_output_tokens) = request.max_output_tokens {
        payload
            .entry("max_tokens".to_owned())
            .or_insert_with(|| json!(max_output_tokens));
    }
    match request.reasoning {
        ReasoningLevel::Auto => {}
        ReasoningLevel::Off => {
            payload.insert("thinking".to_owned(), json!({ "type": "disabled" }));
        }
        reasoning => {
            if let Some(budget_tokens) = reasoning.budget_tokens() {
                payload.insert(
                    "thinking".to_owned(),
                    json!({ "type": "enabled", "budget_tokens": budget_tokens }),
                );
            }
        }
    }
    if !system.is_empty() {
        payload.insert("system".to_owned(), json!(system.join("\n\n")));
    }
    if !tools.is_empty() {
        payload.insert("tools".to_owned(), Value::Array(tools));
    }
    serde_json::to_vec(&Value::Object(payload))
        .map_err(|error| ProviderError::Request(error.to_string()))
}

fn wire_assistant_blocks(blocks: &[alpha_schema::AssistantContentBlock]) -> Vec<Value> {
    blocks
        .iter()
        .filter_map(|block| match block {
            alpha_schema::AssistantContentBlock::Text { text } => {
                Some(json!({ "type": "text", "text": text }))
            }
            alpha_schema::AssistantContentBlock::Thinking { text, signature } => {
                match signature
                    .as_deref()
                    .filter(|signature| !signature.is_empty())
                {
                    Some(signature) => Some(json!({
                        "type": "thinking",
                        "thinking": text,
                        "signature": signature,
                    })),
                    None => Some(json!({ "type": "text", "text": text })),
                }
            }
            alpha_schema::AssistantContentBlock::LocalToolCall {
                id,
                name,
                arguments,
            } => Some(json!({
                "type": "tool_use",
                "id": id,
                "name": name,
                "input": arguments,
            })),
            alpha_schema::AssistantContentBlock::ProviderToolCall { id, name, input } => {
                Some(json!({
                    "type": "server_tool_use",
                    "id": id,
                    "name": name,
                    "input": input,
                }))
            }
            alpha_schema::AssistantContentBlock::ProviderToolResult { id, name, content } => {
                Some(json!({
                    "type": if name == "web_search" {
                        "web_search_tool_result"
                    } else {
                        "tool_result"
                    },
                    "tool_use_id": id,
                    "content": content,
                }))
            }
            alpha_schema::AssistantContentBlock::Citation { .. } => None,
        })
        .collect()
}

fn flush_tool_results(messages: &mut Vec<Value>, tool_results: &mut Vec<Value>) {
    if !tool_results.is_empty() {
        messages.push(json!({
            "role": "user",
            "content": std::mem::take(tool_results),
        }));
    }
}

fn wire_tool(tool: &ToolDefinition) -> Value {
    json!({
        "name": tool.name,
        "description": tool.description,
        "input_schema": tool.input_schema,
    })
}

fn drain_events(
    buffer: &mut Vec<u8>,
    pending: &mut VecDeque<Result<CompletionChunk, ProviderError>>,
    block_kinds: &mut BTreeMap<usize, String>,
    usage: &mut UsageState,
    done: &mut bool,
    finish: bool,
) -> Result<(), ProviderError> {
    while let Some(index) = buffer.iter().position(|byte| *byte == b'\n') {
        let mut line: Vec<u8> = buffer.drain(..=index).collect();
        line.pop();
        process_event_line(&line, pending, block_kinds, usage, done)?;
        if *done {
            buffer.clear();
            return Ok(());
        }
    }
    if finish && !buffer.is_empty() {
        let line = std::mem::take(buffer);
        process_event_line(&line, pending, block_kinds, usage, done)?;
    }
    Ok(())
}

fn process_event_line(
    line: &[u8],
    pending: &mut VecDeque<Result<CompletionChunk, ProviderError>>,
    block_kinds: &mut BTreeMap<usize, String>,
    usage: &mut UsageState,
    done: &mut bool,
) -> Result<(), ProviderError> {
    let line = trim_ascii(line);
    if line.is_empty() || line.starts_with(b":") || line.starts_with(b"event:") {
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
    let event: StreamEvent = serde_json::from_slice(data).map_err(|error| {
        ProviderError::Request(format!(
            "invalid stream event: {error}: {}",
            String::from_utf8_lossy(data)
        ))
    })?;
    match event.kind.as_str() {
        // Prompt tokens arrive once, at the start of the message.
        "message_start" => {
            if let Some(reported) = event.message.and_then(|message| message.usage) {
                usage.input_tokens = reported.input_tokens;
                pending.push_back(Ok(CompletionChunk::Usage {
                    input_tokens: reported.input_tokens,
                    output_tokens: reported.output_tokens,
                    cached_input_tokens: reported.cache_read_input_tokens,
                }));
            }
        }
        // Completion tokens are cumulative, so every delta supersedes the last.
        "message_delta" => {
            if let Some(reported) = event.usage {
                // Some compatible endpoints repeat the prompt size here instead
                // of in `message_start`.
                if reported.input_tokens > 0 {
                    usage.input_tokens = reported.input_tokens;
                }
                pending.push_back(Ok(CompletionChunk::Usage {
                    input_tokens: usage.input_tokens,
                    output_tokens: reported.output_tokens,
                    cached_input_tokens: reported.cache_read_input_tokens,
                }));
            }
        }
        "content_block_start" => {
            if let Some(block) = event.content_block {
                let index = event.index.unwrap_or_default();
                block_kinds.insert(index, block.kind.clone());
                match block.kind.as_str() {
                    "tool_use" => {
                        let arguments = block
                            .input
                            .as_ref()
                            .filter(|value| {
                                !value.is_null()
                                    && value.as_object().is_none_or(|object| !object.is_empty())
                            })
                            .map(Value::to_string)
                            .unwrap_or_default();
                        pending.push_back(Ok(CompletionChunk::ToolCall(ToolCallDelta {
                            index,
                            id: block.id,
                            name: block.name,
                            arguments,
                        })));
                    }
                    "server_tool_use" => {
                        pending.push_back(Ok(CompletionChunk::ProviderToolCall {
                            id: block.id.unwrap_or_else(|| format!("provider-{index}")),
                            name: block.name.unwrap_or_else(|| "provider_tool".to_owned()),
                            input: block.input.unwrap_or_else(|| json!({})),
                        }));
                    }
                    "web_search_tool_result" => {
                        pending.push_back(Ok(CompletionChunk::ProviderToolResult {
                            id: block.id.unwrap_or_else(|| format!("search-{index}")),
                            name: "web_search".to_owned(),
                            content: block.content.unwrap_or_else(|| json!({})),
                        }));
                    }
                    "thinking" => {
                        pending.push_back(Ok(CompletionChunk::Thinking {
                            text: block.thinking.unwrap_or_default(),
                            signature: block.signature,
                        }));
                    }
                    _ => {}
                }
            }
        }
        "content_block_delta" => {
            if let Some(delta) = event.delta {
                match delta.kind.as_str() {
                    "text_delta" => {
                        if let Some(text) = delta.text {
                            pending.push_back(Ok(CompletionChunk::Text(text)));
                        }
                    }
                    "input_json_delta" => {
                        let index = event.index.unwrap_or_default();
                        if block_kinds
                            .get(&index)
                            .is_some_and(|kind| kind == "tool_use")
                            && let Some(partial) = delta.partial_json
                        {
                            pending.push_back(Ok(CompletionChunk::ToolCall(ToolCallDelta {
                                index,
                                id: None,
                                name: None,
                                arguments: partial,
                            })));
                        }
                    }
                    "thinking_delta" => {
                        if let Some(text) = delta.thinking {
                            pending.push_back(Ok(CompletionChunk::Thinking {
                                text,
                                signature: None,
                            }));
                        }
                    }
                    "signature_delta" => {
                        if let Some(signature) = delta.signature {
                            pending.push_back(Ok(CompletionChunk::Thinking {
                                text: String::new(),
                                signature: Some(signature),
                            }));
                        }
                    }
                    "citation_delta" => {
                        if let Some(citation) = delta.citation {
                            pending.push_back(Ok(CompletionChunk::Citation {
                                url: citation
                                    .get("url")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                                title: citation
                                    .get("title")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                                snippet: citation
                                    .get("cited_text")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                            }));
                        }
                    }
                    _ => {}
                }
            }
        }
        "message_stop" => {
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
    index: Option<usize>,
    #[serde(default)]
    message: Option<StreamMessage>,
    #[serde(default)]
    usage: Option<StreamUsage>,
    #[serde(default)]
    content_block: Option<StreamContentBlock>,
    #[serde(default)]
    delta: Option<StreamDelta>,
    #[serde(default)]
    error: Option<Value>,
}

#[derive(Default, Deserialize)]
struct StreamMessage {
    #[serde(default)]
    usage: Option<StreamUsage>,
}

/// Token counts reported by the Messages API.
#[derive(Default, Deserialize)]
struct StreamUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

#[derive(Default, Deserialize)]
struct StreamContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Option<Value>,
    #[serde(default)]
    content: Option<Value>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    signature: Option<String>,
}

#[derive(Default, Deserialize)]
struct StreamDelta {
    #[serde(rename = "type")]
    #[serde(default)]
    kind: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    partial_json: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    signature: Option<String>,
    #[serde(default)]
    citation: Option<Value>,
}

fn snippet(body: &[u8]) -> String {
    String::from_utf8_lossy(body).chars().take(400).collect()
}

/// Registers the Anthropic Messages adapter.
pub struct AnthropicMessagesApiModule;

impl Module for AnthropicMessagesApiModule {
    type Requirements = ();
    type Config = ();

    const KEY: &'static str = "api.anthropic-messages";

    fn activate(&self, ctx: &Ctx, _config: &Self::Config) -> Result<(), ActivateError> {
        let registry = ctx
            .require::<ApiAdapterRegistry>()
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        let adapter: Arc<dyn ApiAdapter> = Arc::new(AnthropicMessagesAdapter);
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
