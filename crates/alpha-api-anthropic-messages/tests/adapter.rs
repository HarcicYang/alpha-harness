use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use alpha_api_anthropic_messages::{ADAPTER_KEY, AnthropicMessagesApiModule};
use alpha_kernel::{Ctx, Module};
use alpha_module_http::{HttpClient, HttpError, HttpRequest, HttpResponse, HttpResponseStream};
use alpha_module_loop::{ChatMessage, CompletionChunk};
use alpha_provider_api::{AdapterRequest, ApiAdapter, ApiAdapterRegistry, ProviderApiModule};
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use serde_json::json;

struct StreamingHttp {
    request: Mutex<Option<HttpRequest>>,
    chunks: Vec<Vec<u8>>,
}

impl HttpClient for StreamingHttp {
    fn request(&self, _request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, HttpError>> {
        Box::pin(async move {
            Ok(HttpResponse {
                status: 200,
                headers: BTreeMap::new(),
                body: Vec::new(),
            })
        })
    }

    fn request_stream(
        &self,
        request: HttpRequest,
    ) -> BoxFuture<'_, Result<HttpResponseStream, HttpError>> {
        *self.request.lock().expect("request lock") = Some(request);
        let chunks = self.chunks.clone();
        Box::pin(async move {
            let body = futures::stream::iter(
                chunks
                    .into_iter()
                    .map(Ok)
                    .collect::<Vec<Result<Vec<u8>, HttpError>>>(),
            );
            Ok(HttpResponseStream {
                status: 200,
                headers: BTreeMap::new(),
                body: Box::pin(body) as BoxStream<'static, Result<Vec<u8>, HttpError>>,
            })
        })
    }
}

#[tokio::test]
async fn streams_text_and_tool_input_deltas() {
    let http = Arc::new(StreamingHttp {
        request: Mutex::new(None),
        chunks: vec![
            b"data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n".to_vec(),
            b"data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hel\"}}\n\n".to_vec(),
            b"data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n".to_vec(),
            b"data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_1\",\"name\":\"read\",\"input\":{}}}\n\n".to_vec(),
            b"data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\"}}\n\n".to_vec(),
            b"data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"a.txt\\\"}\"}}\n\n".to_vec(),
            b"data: {\"type\":\"message_stop\"}\n\n".to_vec(),
        ],
    });
    let adapter = alpha_api_anthropic_messages::AnthropicMessagesAdapter;
    let chunks: Vec<CompletionChunk> = adapter
        .stream(
            http.clone(),
            AdapterRequest {
                base_url: "https://api.anthropic.test".to_owned(),
                api_key: Some("secret".to_owned()),
                headers: BTreeMap::new(),
                model: "claude-test".to_owned(),
                messages: vec![ChatMessage::User("hi".to_owned())],
                tools: Vec::new(),
                provider_tools: vec!["web_search".to_owned()],
                options: json!({}),
            },
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect();

    assert_eq!(chunks[0], CompletionChunk::Text("hel".to_owned()));
    assert_eq!(chunks[1], CompletionChunk::Text("lo".to_owned()));
    assert!(matches!(
        &chunks[2],
        CompletionChunk::ToolCall(delta)
            if delta.id.as_deref() == Some("tool_1") && delta.name.as_deref() == Some("read")
    ));
    assert!(matches!(
        &chunks[3],
        CompletionChunk::ToolCall(delta) if delta.arguments == "{\"path\":"
    ));
    assert!(matches!(
        &chunks[4],
        CompletionChunk::ToolCall(delta) if delta.arguments == "\"a.txt\"}"
    ));

    let request = http
        .request
        .lock()
        .expect("request lock")
        .clone()
        .expect("request");
    assert_eq!(request.url, "https://api.anthropic.test/v1/messages");
    let payload: serde_json::Value = serde_json::from_slice(&request.body).expect("payload json");
    assert_eq!(payload["stream"], json!(true));
    assert_eq!(payload["tools"][0]["type"], json!("web_search_20250305"));
}

#[tokio::test]
async fn streams_provider_native_web_search_blocks() {
    let http = Arc::new(StreamingHttp {
        request: Mutex::new(None),
        chunks: vec![
            b"data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"server_tool_use\",\"id\":\"srvtoolu_1\",\"name\":\"web_search\",\"input\":{\"query\":\"rust\"}}}\n\n".to_vec(),
            b"data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"web_search_tool_result\",\"id\":\"srvtoolu_1\",\"content\":[{\"type\":\"web_search_result\",\"url\":\"https://example.com\",\"title\":\"Example\"}]}}\n\n".to_vec(),
            b"data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"citation_delta\",\"citation\":{\"url\":\"https://example.com\",\"title\":\"Example\",\"cited_text\":\"snippet\"}}}\n\n".to_vec(),
            b"data: {\"type\":\"message_stop\"}\n\n".to_vec(),
        ],
    });
    let adapter = alpha_api_anthropic_messages::AnthropicMessagesAdapter;
    let chunks: Vec<CompletionChunk> = adapter
        .stream(
            http,
            AdapterRequest {
                base_url: "https://api.anthropic.test".to_owned(),
                api_key: Some("secret".to_owned()),
                headers: BTreeMap::new(),
                model: "claude-test".to_owned(),
                messages: vec![ChatMessage::User("hi".to_owned())],
                tools: Vec::new(),
                provider_tools: vec!["web_search".to_owned()],
                options: json!({}),
            },
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect();

    assert!(matches!(
        &chunks[0],
        CompletionChunk::ProviderToolCall { id, name, input }
            if id == "srvtoolu_1" && name == "web_search" && input["query"] == "rust"
    ));
    assert!(matches!(
        &chunks[1],
        CompletionChunk::ProviderToolResult { id, name, content }
            if id == "srvtoolu_1" && name == "web_search" && content[0]["url"] == "https://example.com"
    ));
    assert!(matches!(
        &chunks[2],
        CompletionChunk::Citation { url, title, snippet }
            if url.as_deref() == Some("https://example.com")
                && title.as_deref() == Some("Example")
                && snippet.as_deref() == Some("snippet")
    ));
}

#[tokio::test]
async fn groups_consecutive_tool_results_into_one_user_message() {
    let http = Arc::new(StreamingHttp {
        request: Mutex::new(None),
        chunks: vec![b"data: {\"type\":\"message_stop\"}\n\n".to_vec()],
    });
    let adapter = alpha_api_anthropic_messages::AnthropicMessagesAdapter;
    adapter
        .stream(
            http.clone(),
            AdapterRequest {
                base_url: "https://api.anthropic.test".to_owned(),
                api_key: Some("secret".to_owned()),
                headers: BTreeMap::new(),
                model: "claude-test".to_owned(),
                messages: vec![
                    ChatMessage::User("inspect the workspace".to_owned()),
                    ChatMessage::AssistantToolCalls {
                        text: String::new(),
                        calls: vec![
                            alpha_module_loop::ToolCall {
                                id: "call-1".to_owned(),
                                name: "shell".to_owned(),
                                arguments: json!({ "command": "pwd" }),
                            },
                            alpha_module_loop::ToolCall {
                                id: "call-2".to_owned(),
                                name: "shell".to_owned(),
                                arguments: json!({ "command": "ls" }),
                            },
                        ],
                    },
                    ChatMessage::ToolResult {
                        call_id: "call-1".to_owned(),
                        content: "workspace".to_owned(),
                    },
                    ChatMessage::ToolResult {
                        call_id: "call-2".to_owned(),
                        content: "Cargo.toml".to_owned(),
                    },
                ],
                tools: Vec::new(),
                provider_tools: Vec::new(),
                options: json!({}),
            },
        )
        .collect::<Vec<_>>()
        .await;

    let request = http
        .request
        .lock()
        .expect("request lock")
        .clone()
        .expect("request");
    let payload: serde_json::Value = serde_json::from_slice(&request.body).expect("payload json");
    let messages = payload["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[1]["role"], json!("assistant"));
    assert_eq!(
        messages[1]["content"].as_array().expect("tool calls").len(),
        2
    );
    assert_eq!(messages[2]["role"], json!("user"));
    let results = messages[2]["content"].as_array().expect("tool results");
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["type"], json!("tool_result"));
    assert_eq!(results[0]["tool_use_id"], json!("call-1"));
    assert_eq!(results[1]["type"], json!("tool_result"));
    assert_eq!(results[1]["tool_use_id"], json!("call-2"));
}

#[test]
fn module_registers_the_adapter() {
    let ctx = Ctx::root();
    ProviderApiModule.activate(&ctx, &()).expect("provider api");
    AnthropicMessagesApiModule
        .activate(&ctx, &())
        .expect("anthropic adapter");
    let registry = ctx
        .require::<ApiAdapterRegistry>()
        .expect("adapter registry");
    assert!(registry.get(ADAPTER_KEY).is_some());
    ctx.dispose();
    assert!(registry.get(ADAPTER_KEY).is_none());
}
