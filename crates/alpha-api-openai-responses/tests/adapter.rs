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
async fn streams_responses_text_and_tool_arguments() {
    let http = Arc::new(StreamingHttp {
        request: Mutex::new(None),
        chunks: vec![
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"hel\"}\n\n".to_vec(),
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"lo\"}\n\n".to_vec(),
            b"data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"read\"}}\n\n".to_vec(),
            b"data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"delta\":\"{\\\"path\\\":\\\"a.txt\\\"}\"}\n\n".to_vec(),
            b"data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":2048,\"output_tokens\":64,\"input_tokens_details\":{\"cached_tokens\":1536}}}}\n\n".to_vec(),
        ],
    });
    let adapter = alpha_api_openai_responses::OpenAiResponsesAdapter;
    let chunks: Vec<CompletionChunk> = adapter
        .stream(
            http.clone(),
            AdapterRequest {
                base_url: "https://api.test/v1".to_owned(),
                api_key: Some("secret".to_owned()),
                headers: BTreeMap::new(),
                model: "test-model".to_owned(),
                reasoning: Default::default(),
                messages: vec![ChatMessage::User("hi".to_owned())],
                tools: Vec::new(),
                provider_tools: Vec::new(),
                max_output_tokens: Some(128_000),
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
            if delta.id.as_deref() == Some("call_1") && delta.name.as_deref() == Some("read")
    ));
    assert!(matches!(
        &chunks[3],
        CompletionChunk::ToolCall(delta)
            if delta.arguments == "{\"path\":\"a.txt\"}"
    ));
    assert_eq!(
        chunks[4],
        CompletionChunk::Usage {
            input_tokens: 2048,
            output_tokens: 64,
            cached_input_tokens: 1536
        }
    );

    let request = http
        .request
        .lock()
        .expect("request lock")
        .clone()
        .expect("request");
    assert_eq!(request.url, "https://api.test/v1/responses");
    let payload: serde_json::Value = serde_json::from_slice(&request.body).expect("payload json");
    assert_eq!(payload["stream"], json!(true));
    assert_eq!(payload["max_output_tokens"], json!(128_000));
}

#[tokio::test]
async fn sends_user_images_as_input_images() {
    let http = Arc::new(StreamingHttp {
        request: Mutex::new(None),
        chunks: vec![b"data: {\"type\":\"response.completed\"}\n\n".to_vec()],
    });
    alpha_api_openai_responses::OpenAiResponsesAdapter
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
    assert_eq!(payload["input"][0]["content"][0]["text"], "inspect");
    assert_eq!(
        payload["input"][0]["content"][1]["image_url"],
        "data:image/png;base64,aGVsbG8="
    );
}

#[tokio::test]
async fn sends_tool_result_images_as_input_images() {
    let http = Arc::new(StreamingHttp {
        request: Mutex::new(None),
        chunks: vec![b"data: {\"type\":\"response.completed\"}\n\n".to_vec()],
    });
    alpha_api_openai_responses::OpenAiResponsesAdapter
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
    assert_eq!(payload["input"][0]["output"][0]["text"], "loaded");
    assert_eq!(
        payload["input"][0]["output"][1]["image_url"],
        "data:image/png;base64,aGVsbG8="
    );
}

#[tokio::test]
async fn maps_reasoning_effort_into_response_requests() {
    let http = Arc::new(StreamingHttp {
        request: Mutex::new(None),
        chunks: vec![b"data: {\"type\":\"response.completed\"}\n\n".to_vec()],
    });
    alpha_api_openai_responses::OpenAiResponsesAdapter
        .stream(
            http.clone(),
            AdapterRequest {
                base_url: "https://api.test/v1".to_owned(),
                api_key: Some("secret".to_owned()),
                headers: BTreeMap::new(),
                model: "reasoning-model".to_owned(),
                reasoning: ReasoningLevel::Off,
                messages: vec![ChatMessage::User("hi".to_owned())],
                tools: Vec::new(),
                provider_tools: Vec::new(),
                max_output_tokens: None,
                options: json!({ "reasoning": { "effort": "low" } }),
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
    assert_eq!(payload["reasoning"]["effort"], json!("none"));
    assert_eq!(payload["include"], json!(["reasoning.encrypted_content"]));
}

#[tokio::test]
async fn streams_and_replays_reasoning_items() {
    let http = Arc::new(StreamingHttp {
        request: Mutex::new(None),
        chunks: vec![
            b"data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"think \"}\n\n"
                .to_vec(),
            b"data: {\"type\":\"response.reasoning_text.delta\",\"delta\":\"more\"}\n\n".to_vec(),
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"answer\"}\n\n".to_vec(),
            b"data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"reasoning\",\"encrypted_content\":\"sig\"}}\n\n".to_vec(),
            b"data: {\"type\":\"response.completed\"}\n\n".to_vec(),
        ],
    });
    let chunks: Vec<CompletionChunk> = alpha_api_openai_responses::OpenAiResponsesAdapter
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
            CompletionChunk::Thinking {
                text: String::new(),
                signature: Some("sig".to_owned()),
            },
        ]
    );

    alpha_api_openai_responses::OpenAiResponsesAdapter
        .stream(
            http.clone(),
            AdapterRequest {
                base_url: "https://api.test/v1".to_owned(),
                api_key: Some("secret".to_owned()),
                headers: BTreeMap::new(),
                model: "reasoning-model".to_owned(),
                reasoning: ReasoningLevel::High,
                messages: vec![ChatMessage::AssistantContent(vec![
                    AssistantContentBlock::Thinking {
                        text: "remembered".to_owned(),
                        signature: Some("sig".to_owned()),
                    },
                    AssistantContentBlock::Text {
                        text: "visible".to_owned(),
                    },
                ])],
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
    assert_eq!(payload["input"][0]["type"], "reasoning");
    assert_eq!(payload["input"][0]["encrypted_content"], "sig");
    assert_eq!(payload["input"][0]["summary"][0]["text"], "remembered");
    assert_eq!(payload["input"][1]["content"][0]["text"], "visible");
}
