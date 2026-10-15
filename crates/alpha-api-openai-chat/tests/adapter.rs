use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use alpha_api_openai_chat::{ADAPTER_KEY, OpenAiChatApiModule};
use alpha_kernel::{Ctx, Module};
use alpha_module_http::{HttpClient, HttpError, HttpRequest, HttpResponse, HttpResponseStream};
use alpha_module_loop::{ChatMessage, CompletionChunk};
use alpha_provider_api::{
    AdapterRequest, ApiAdapter, ApiAdapterRegistry, ProviderApiModule, ReasoningLevel,
};
use alpha_schema::AssistantContentBlock;
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
                model: "test-model".to_owned(),
                reasoning: Default::default(),
                messages: vec![ChatMessage::User("hi".to_owned())],
                tools: Vec::new(),
                provider_tools: Vec::new(),
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

#[test]
fn module_registers_the_adapter() {
    let ctx = Ctx::root();
    ProviderApiModule.activate(&ctx, &()).expect("provider api");
    OpenAiChatApiModule
        .activate(&ctx, &())
        .expect("openai chat adapter");
    let registry = ctx
        .require::<ApiAdapterRegistry>()
        .expect("adapter registry");
    assert!(registry.get(ADAPTER_KEY).is_some());
    ctx.dispose();
    assert!(registry.get(ADAPTER_KEY).is_none());
}
