use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use alpha_http_api::{HttpClient, HttpError, HttpRequest, HttpResponse, HttpResponseStream};
use alpha_module_loop::{ChatMessage, CompletionChunk};
use alpha_provider_api::{AdapterRequest, ApiAdapter, ReasoningLevel};
use alpha_schema::{AssistantContentBlock, ImageAttachment};
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
async fn streams_openai_chat_text_deltas() {
    let http = Arc::new(StreamingHttp {
        request: Mutex::new(None),
        chunks: vec![
            b"data: {\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}\n\n".to_vec(),
            b"data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n".to_vec(),
            b"data: [DONE]\n\n".to_vec(),
        ],
    });
    let adapter = alpha_api_openai_chat::OpenAiChatAdapter;
    let chunks: Vec<CompletionChunk> = adapter
        .stream(
            http.clone(),
            AdapterRequest {
                base_url: "https://api.test/v1".to_owned(),
                api_key: Some("secret".to_owned()),
                headers: BTreeMap::new(),
                model: "group/模型-v1".to_owned(),
                reasoning: Default::default(),
                messages: vec![ChatMessage::User("hi".to_owned())],
                tools: Vec::new(),
                provider_tools: Vec::new(),
                max_output_tokens: Some(384_000),
                options: json!({}),
            },
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect();

    assert_eq!(
        chunks,
        vec![
            CompletionChunk::Text("hel".to_owned()),
            CompletionChunk::Text("lo".to_owned())
        ]
    );
    let request = http
        .request
        .lock()
        .expect("request lock")
        .clone()
        .expect("request");
    assert_eq!(request.url, "https://api.test/v1/chat/completions");
    assert_eq!(
        request.headers.get("authorization").map(String::as_str),
        Some("Bearer secret")
    );
    let payload: serde_json::Value = serde_json::from_slice(&request.body).expect("payload json");
    assert_eq!(payload["stream"], json!(true));
    assert_eq!(payload["max_tokens"], json!(384_000));
    // Model ids are opaque: slashes and non-ASCII characters reach the wire as-is.
    assert_eq!(payload["model"], json!("group/模型-v1"));
}

#[tokio::test]
async fn explicit_chat_options_override_the_catalog_output_limit() {
    let http = Arc::new(StreamingHttp {
        request: Mutex::new(None),
        chunks: vec![b"data: [DONE]\n\n".to_vec()],
    });
    alpha_api_openai_chat::OpenAiChatAdapter
        .stream(
            http.clone(),
            AdapterRequest {
                base_url: "https://api.test/v1".to_owned(),
                api_key: Some("secret".to_owned()),
                headers: BTreeMap::new(),
                model: "deepseek-v4-pro".to_owned(),
                reasoning: Default::default(),
                messages: vec![ChatMessage::User("hi".to_owned())],
                tools: Vec::new(),
                provider_tools: Vec::new(),
                max_output_tokens: Some(384_000),
                options: json!({ "max_tokens": 65_536 }),
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
    assert_eq!(payload["max_tokens"], json!(65_536));
}

#[tokio::test]
async fn sends_user_images_as_data_urls() {
    let http = Arc::new(StreamingHttp {
        request: Mutex::new(None),
        chunks: vec![b"data: [DONE]\n\n".to_vec()],
    });
    alpha_api_openai_chat::OpenAiChatAdapter
        .stream(
            http.clone(),
            AdapterRequest {
                base_url: "https://api.test/v1".to_owned(),
                api_key: Some("secret".to_owned()),
                headers: BTreeMap::new(),
                model: "vision-model".to_owned(),
                reasoning: Default::default(),
                messages: vec![ChatMessage::UserContent {
                    text: "inspect".to_owned(),
                    images: vec![ImageAttachment {
                        name: "pixel.png".to_owned(),
                        mime_type: "image/png".to_owned(),
                        data: "aGVsbG8=".to_owned(),
                    }],
                }],
                tools: Vec::new(),
                provider_tools: Vec::new(),
                max_output_tokens: None,
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
    assert_eq!(payload["messages"][0]["content"][0]["text"], "inspect");
    assert_eq!(
        payload["messages"][0]["content"][1]["image_url"]["url"],
        "data:image/png;base64,aGVsbG8="
    );
}

#[tokio::test]
async fn sends_tool_result_images_as_data_urls() {
    let http = Arc::new(StreamingHttp {
        request: Mutex::new(None),
        chunks: vec![b"data: [DONE]\n\n".to_vec()],
    });
    alpha_api_openai_chat::OpenAiChatAdapter
        .stream(
            http.clone(),
            AdapterRequest {
                base_url: "https://api.test/v1".to_owned(),
                api_key: Some("secret".to_owned()),
                headers: BTreeMap::new(),
                model: "vision-model".to_owned(),
                reasoning: Default::default(),
                messages: vec![ChatMessage::ToolResult {
                    call_id: "call-1".to_owned(),
                    content: "loaded".to_owned(),
                    images: vec![ImageAttachment {
                        name: "pixel.png".to_owned(),
                        mime_type: "image/png".to_owned(),
                        data: "aGVsbG8=".to_owned(),
                    }],
                }],
                tools: Vec::new(),
                provider_tools: Vec::new(),
                max_output_tokens: None,
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
    assert_eq!(payload["messages"][0]["content"][0]["text"], "loaded");
    assert_eq!(
        payload["messages"][0]["content"][1]["image_url"]["url"],
        "data:image/png;base64,aGVsbG8="
    );
}

#[tokio::test]
async fn maps_reasoning_effort_into_chat_requests() {
    let http = Arc::new(StreamingHttp {
        request: Mutex::new(None),
        chunks: vec![b"data: [DONE]\n\n".to_vec()],
    });
    alpha_api_openai_chat::OpenAiChatAdapter
        .stream(
            http.clone(),
            AdapterRequest {
                base_url: "https://api.test/v1".to_owned(),
                api_key: Some("secret".to_owned()),
                headers: BTreeMap::new(),
                model: "reasoning-model".to_owned(),
                reasoning: ReasoningLevel::High,
                messages: vec![ChatMessage::User("hi".to_owned())],
                tools: Vec::new(),
                provider_tools: Vec::new(),
                max_output_tokens: None,
                options: json!({ "reasoning_effort": "low" }),
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
    assert_eq!(payload["reasoning_effort"], json!("high"));
}

#[tokio::test]
async fn streams_and_replays_reasoning_content() {
    let http = Arc::new(StreamingHttp {
        request: Mutex::new(None),
        chunks: vec![
            b"data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"think \"}}]}\n\n".to_vec(),
            b"data: {\"choices\":[{\"delta\":{\"reasoning\":\"more\"}}]}\n\n".to_vec(),
            b"data: {\"choices\":[{\"delta\":{\"content\":\"answer\"}}]}\n\n".to_vec(),
            b"data: [DONE]\n\n".to_vec(),
        ],
    });
    let chunks: Vec<CompletionChunk> = alpha_api_openai_chat::OpenAiChatAdapter
        .stream(
            http.clone(),
            AdapterRequest {
                base_url: "https://api.test/v1".to_owned(),
                api_key: Some("secret".to_owned()),
                headers: BTreeMap::new(),
                model: "reasoning-model".to_owned(),
                reasoning: ReasoningLevel::High,
                messages: vec![ChatMessage::User("hi".to_owned())],
                tools: Vec::new(),
                provider_tools: Vec::new(),
                max_output_tokens: None,
                options: json!({}),
            },
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        chunks,
        vec![
            CompletionChunk::Thinking {
                text: "think ".to_owned(),
                signature: None,
            },
            CompletionChunk::Thinking {
                text: "more".to_owned(),
                signature: None,
            },
            CompletionChunk::Text("answer".to_owned()),
        ]
    );

    alpha_api_openai_chat::OpenAiChatAdapter
        .stream(
            http.clone(),
            AdapterRequest {
                base_url: "https://api.test/v1".to_owned(),
                api_key: Some("secret".to_owned()),
                headers: BTreeMap::new(),
                model: "reasoning-model".to_owned(),
                reasoning: ReasoningLevel::High,
                messages: vec![
                    ChatMessage::User("hi".to_owned()),
                    ChatMessage::AssistantContent(vec![
                        AssistantContentBlock::Thinking {
                            text: "remembered".to_owned(),
                            signature: None,
                        },
                        AssistantContentBlock::Text {
                            text: "visible".to_owned(),
                        },
                    ]),
                ],
                tools: Vec::new(),
                provider_tools: Vec::new(),
                max_output_tokens: None,
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
    assert_eq!(payload["messages"][1]["reasoning_content"], "remembered");
    assert_eq!(payload["messages"][1]["content"], "visible");
}
