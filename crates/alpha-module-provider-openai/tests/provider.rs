use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use alpha_kernel::{Ctx, Module};
use alpha_module_http::{HttpClient, HttpError, HttpRequest, HttpResponse, HttpResponseStream};
use alpha_module_loop::{
    ChatMessage, CompletionChunk, CompletionRequest, ModelProvider, ProviderRegistry, ToolCall,
    ToolCallDelta, ToolDefinition,
};
use alpha_module_provider_openai::{
    OpenAiConfig, OpenAiProvider, OpenAiProviderModule, PROVIDER_NAME,
};
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use serde_json::{Value, json};

struct FakeHttp {
    requests: Mutex<Vec<HttpRequest>>,
    response: HttpResponse,
}

impl FakeHttp {
    fn new(status: u16, body: Value) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            response: HttpResponse {
                status,
                headers: BTreeMap::new(),
                body: serde_json::to_vec(&body).expect("body"),
            },
        }
    }
}

impl HttpClient for FakeHttp {
    fn request(&self, request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, HttpError>> {
        self.requests.lock().expect("request lock").push(request);
        let response = self.response.clone();
        Box::pin(async move { Ok(response) })
    }
}

struct ChunkedHttp {
    request: Mutex<Option<HttpRequest>>,
    chunks: Vec<Vec<u8>>,
}

impl HttpClient for ChunkedHttp {
    fn request(&self, request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, HttpError>> {
        *self.request.lock().expect("request lock") = Some(request);
        let body = self.chunks.concat();
        Box::pin(async move {
            Ok(HttpResponse {
                status: 200,
                headers: BTreeMap::new(),
                body,
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

fn client_with(status: u16, body: Value) -> (Arc<FakeHttp>, Arc<dyn HttpClient>) {
    client_raw(FakeHttp::new(status, body).response)
}

fn client_raw(response: HttpResponse) -> (Arc<FakeHttp>, Arc<dyn HttpClient>) {
    let fake = Arc::new(FakeHttp {
        requests: Mutex::new(Vec::new()),
        response,
    });
    let client: Arc<dyn HttpClient> = fake.clone();
    (fake, client)
}

fn completion_response(text: &str) -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: BTreeMap::new(),
        body: json!({ "choices": [{ "message": { "content": text } }] })
            .to_string()
            .into_bytes(),
    }
}

#[tokio::test]
async fn sends_chat_completion_requests_and_parses_text() {
    let (fake, http) = client_raw(completion_response("hello from model"));
    let provider = OpenAiProvider::new(http, "https://api.test/v1", "secret-key");
    let response = provider
        .complete(CompletionRequest {
            session_id: "test-session".to_owned(),
            model: "gpt-test".to_owned(),
            reasoning: Default::default(),
            messages: vec![ChatMessage::User("hi".to_owned())],
            tools: Vec::new(),
            provider_tools: Vec::new(),
            max_output_tokens: Some(100_000),
        })
        .await
        .expect("complete");
    assert_eq!(response.text, "hello from model");

    let requests = fake.requests.lock().expect("request lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url, "https://api.test/v1/chat/completions");
    assert_eq!(
        requests[0].headers.get("authorization").map(String::as_str),
        Some("Bearer secret-key")
    );
    let body: Value = serde_json::from_slice(&requests[0].body).expect("body json");
    assert_eq!(body["model"], json!("gpt-test"));
    assert_eq!(body["stream"], json!(false));
    assert_eq!(body["messages"][0]["role"], json!("user"));
    assert_eq!(body["messages"][0]["content"], json!("hi"));
    assert_eq!(body["max_tokens"], json!(100_000));
}

#[tokio::test]
async fn rejects_error_statuses_and_malformed_bodies() {
    let (_fake, http) = client_with(500, json!({ "error": { "message": "boom" } }));
    let provider = Arc::new(OpenAiProvider::new(http, "https://api.test/v1", "key"));
    let error = provider
        .complete(CompletionRequest {
            session_id: "test-session".to_owned(),
            model: "gpt-test".to_owned(),
            reasoning: Default::default(),
            messages: vec![ChatMessage::User("hi".to_owned())],
            tools: Vec::new(),
            provider_tools: Vec::new(),
            max_output_tokens: None,
        })
        .await
        .expect_err("status");
    assert!(error.to_string().contains("status 500"));

    let (_fake, http) = client_raw(HttpResponse {
        status: 200,
        headers: BTreeMap::new(),
        body: b"not json".to_vec(),
    });
    let provider = Arc::new(OpenAiProvider::new(http, "https://api.test/v1", "key"));
    let error = provider
        .complete(CompletionRequest {
            session_id: "test-session".to_owned(),
            model: "gpt-test".to_owned(),
            reasoning: Default::default(),
            messages: vec![ChatMessage::User("hi".to_owned())],
            tools: Vec::new(),
            provider_tools: Vec::new(),
            max_output_tokens: None,
        })
        .await
        .expect_err("malformed");
    assert!(error.to_string().contains("invalid response"));
}

#[tokio::test]
async fn maps_tool_definitions_and_parses_tool_calls() {
    let response = HttpResponse {
        status: 200,
        headers: BTreeMap::new(),
        body: json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "read",
                            "arguments": "{\"path\":\"a.txt\"}"
                        }
                    }]
                }
            }]
        })
        .to_string()
        .into_bytes(),
    };
    let (fake, http) = client_raw(response);
    let provider = Arc::new(OpenAiProvider::new(http, "https://api.test/v1", "key"));
    let completion = provider
        .complete(CompletionRequest {
            session_id: "test-session".to_owned(),
            model: "gpt-test".to_owned(),
            reasoning: Default::default(),
            messages: vec![
                ChatMessage::User("read a.txt".to_owned()),
                ChatMessage::AssistantToolCalls {
                    text: String::new(),
                    calls: vec![ToolCall {
                        id: "call_0".to_owned(),
                        name: "read".to_owned(),
                        arguments: json!({ "path": "a.txt" }),
                    }],
                },
                ChatMessage::ToolResult {
                    call_id: "call_0".to_owned(),
                    content: "previous".to_owned(),
                },
            ],
            tools: vec![ToolDefinition {
                name: "read".to_owned(),
                description: "Read a file".to_owned(),
                input_schema: json!({ "type": "object" }),
            }],
            provider_tools: Vec::new(),
            max_output_tokens: None,
        })
        .await
        .expect("complete");

    assert!(completion.text.is_empty());
    assert_eq!(
        completion.tool_calls,
        vec![ToolCall {
            id: "call_1".to_owned(),
            name: "read".to_owned(),
            arguments: json!({ "path": "a.txt" }),
        }]
    );

    let requests = fake.requests.lock().expect("request lock");
    let body: Value = serde_json::from_slice(&requests[0].body).expect("body json");
    assert_eq!(body["tools"][0]["function"]["name"], json!("read"));
    assert_eq!(body["messages"][1]["role"], json!("assistant"));
    assert_eq!(
        body["messages"][1]["tool_calls"][0]["function"]["name"],
        json!("read")
    );
    assert_eq!(body["messages"][2]["role"], json!("tool"));
    assert_eq!(body["messages"][2]["tool_call_id"], json!("call_0"));
}

#[tokio::test]
async fn streams_text_deltas_and_requests_streaming_mode() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
        "data: [DONE]\n\n",
    );
    let (fake, http) = client_raw(HttpResponse {
        status: 200,
        headers: BTreeMap::new(),
        body: body.as_bytes().to_vec(),
    });
    let provider = Arc::new(OpenAiProvider::new(http, "https://api.test/v1", "key"));
    let chunks: Vec<Result<CompletionChunk, _>> = provider
        .stream(CompletionRequest {
            session_id: "test-session".to_owned(),
            model: "stream-model".to_owned(),
            reasoning: Default::default(),
            messages: vec![ChatMessage::User("hi".to_owned())],
            tools: Vec::new(),
            provider_tools: Vec::new(),
            max_output_tokens: None,
        })
        .collect()
        .await;
    let chunks: Vec<CompletionChunk> = chunks.into_iter().map(Result::unwrap).collect();

    assert_eq!(
        chunks,
        vec![
            CompletionChunk::Text("hel".to_owned()),
            CompletionChunk::Text("lo".to_owned()),
        ]
    );
    let requests = fake.requests.lock().expect("request lock");
    let body: Value = serde_json::from_slice(&requests[0].body).expect("body json");
    assert_eq!(body["stream"], json!(true));
}

#[tokio::test]
async fn accumulates_tool_call_deltas_split_across_network_chunks() {
    let chunks = vec![
        b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",".to_vec(),
        b"\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"pa\"}}]}}]}\n\n".to_vec(),
        b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,".to_vec(),
        b"\"function\":{\"arguments\":\"th\\\":\\\"a.txt\\\"}\"}}]}}]}\n\n".to_vec(),
        b"data: [DONE]\n\n".to_vec(),
    ];
    let raw = Arc::new(ChunkedHttp {
        request: Mutex::new(None),
        chunks,
    });
    let http: Arc<dyn HttpClient> = raw.clone();
    let provider = Arc::new(OpenAiProvider::new(http, "https://api.test/v1", "key"));
    let chunks: Vec<CompletionChunk> = provider
        .stream(CompletionRequest {
            session_id: "test-session".to_owned(),
            model: "stream-model".to_owned(),
            reasoning: Default::default(),
            messages: vec![ChatMessage::User("read".to_owned())],
            tools: Vec::new(),
            provider_tools: Vec::new(),
            max_output_tokens: None,
        })
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect();

    assert_eq!(
        chunks,
        vec![
            CompletionChunk::ToolCall(ToolCallDelta {
                index: 0,
                id: Some("call_1".to_owned()),
                name: Some("read".to_owned()),
                arguments: "{\"pa".to_owned(),
            }),
            CompletionChunk::ToolCall(ToolCallDelta {
                index: 0,
                id: None,
                name: None,
                arguments: "th\":\"a.txt\"}".to_owned(),
            }),
        ]
    );
    assert!(raw.request.lock().expect("request lock").is_some());
}

#[tokio::test]
async fn rejects_malformed_stream_events() {
    let (_fake, http) = client_raw(HttpResponse {
        status: 200,
        headers: BTreeMap::new(),
        body: b"data: {not-json}\n\n".to_vec(),
    });
    let provider = Arc::new(OpenAiProvider::new(http, "https://api.test/v1", "key"));
    let error = provider
        .stream(CompletionRequest {
            session_id: "test-session".to_owned(),
            model: "stream-model".to_owned(),
            reasoning: Default::default(),
            messages: vec![ChatMessage::User("hi".to_owned())],
            tools: Vec::new(),
            provider_tools: Vec::new(),
            max_output_tokens: None,
        })
        .next()
        .await
        .expect("chunk")
        .expect_err("invalid stream event");
    assert!(error.to_string().contains("invalid stream event"));
}

#[test]
fn module_registers_and_unregisters_the_provider() {
    let registry = Arc::new(ProviderRegistry::new());
    let ctx = Ctx::root();
    ctx.provide::<ProviderRegistry>(registry.clone())
        .expect("registry");
    let (_fake, http) = client_raw(completion_response("ok"));
    ctx.provide::<dyn HttpClient>(http).expect("http");

    OpenAiProviderModule
        .activate(
            &ctx,
            &OpenAiConfig {
                base_url: "https://api.test/v1".to_owned(),
                api_key: "test-key".to_owned(),
                max_output_tokens: None,
            },
        )
        .expect("activate");
    assert!(registry.get(PROVIDER_NAME).is_some());

    ctx.dispose();
    assert!(registry.get(PROVIDER_NAME).is_none());
}

#[test]
fn module_rejects_missing_api_keys() {
    let ctx = Ctx::root();
    ctx.provide::<ProviderRegistry>(Arc::new(ProviderRegistry::new()))
        .expect("registry");
    let (_fake, http) = client_raw(completion_response("ok"));
    ctx.provide::<dyn HttpClient>(http).expect("http");

    let error = OpenAiProviderModule
        .activate(
            &ctx,
            &OpenAiConfig {
                base_url: "https://api.test/v1".to_owned(),
                api_key: String::new(),
                max_output_tokens: None,
            },
        )
        .expect_err("missing key");
    assert!(error.to_string().contains("configure `api_key`"));
}
