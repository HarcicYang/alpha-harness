use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use alpha_api_openai_responses::{ADAPTER_KEY, OpenAiResponsesApiModule};
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
async fn streams_responses_text_and_tool_arguments() {
    let http = Arc::new(StreamingHttp {
        request: Mutex::new(None),
        chunks: vec![
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"hel\"}\n\n".to_vec(),
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"lo\"}\n\n".to_vec(),
            b"data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"read\"}}\n\n".to_vec(),
            b"data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"delta\":\"{\\\"path\\\":\\\"a.txt\\\"}\"}\n\n".to_vec(),
            b"data: {\"type\":\"response.completed\"}\n\n".to_vec(),
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

    let request = http
        .request
        .lock()
        .expect("request lock")
        .clone()
        .expect("request");
    assert_eq!(request.url, "https://api.test/v1/responses");
    let payload: serde_json::Value = serde_json::from_slice(&request.body).expect("payload json");
    assert_eq!(payload["stream"], json!(true));
}

#[test]
fn module_registers_the_adapter() {
    let ctx = Ctx::root();
    ProviderApiModule.activate(&ctx, &()).expect("provider api");
    OpenAiResponsesApiModule
        .activate(&ctx, &())
        .expect("responses adapter");
    let registry = ctx
        .require::<ApiAdapterRegistry>()
        .expect("adapter registry");
    assert!(registry.get(ADAPTER_KEY).is_some());
    ctx.dispose();
    assert!(registry.get(ADAPTER_KEY).is_none());
}
