use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use alpha_kernel::{Ctx, Module};
use alpha_module_http::{HttpClient, HttpError, HttpRequest, HttpResponse};
use alpha_module_loop::{ChatMessage, CompletionRequest, ModelProvider, ProviderRegistry};
use alpha_module_provider_openai::{
    OpenAiConfig, OpenAiProvider, OpenAiProviderModule, PROVIDER_NAME,
};
use futures::future::BoxFuture;
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
            model: "gpt-test".to_owned(),
            messages: vec![ChatMessage::User("hi".to_owned())],
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
}

#[tokio::test]
async fn rejects_error_statuses_and_malformed_bodies() {
    let (_fake, http) = client_with(500, json!({ "error": { "message": "boom" } }));
    let provider = OpenAiProvider::new(http, "https://api.test/v1", "key");
    let error = provider
        .complete(CompletionRequest {
            model: "gpt-test".to_owned(),
            messages: vec![ChatMessage::User("hi".to_owned())],
        })
        .await
        .expect_err("status");
    assert!(error.to_string().contains("status 500"));

    let (_fake, http) = client_raw(HttpResponse {
        status: 200,
        headers: BTreeMap::new(),
        body: b"not json".to_vec(),
    });
    let provider = OpenAiProvider::new(http, "https://api.test/v1", "key");
    let error = provider
        .complete(CompletionRequest {
            model: "gpt-test".to_owned(),
            messages: vec![ChatMessage::User("hi".to_owned())],
        })
        .await
        .expect_err("malformed");
    assert!(error.to_string().contains("invalid response"));
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
                api_key_env: "ALPHA_TEST_UNUSED_KEY".to_owned(),
                api_key: Some("test-key".to_owned()),
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
                api_key_env: "ALPHA_TEST_DEFINITELY_MISSING_KEY".to_owned(),
                api_key: None,
            },
        )
        .expect_err("missing key");
    assert!(
        error
            .to_string()
            .contains("ALPHA_TEST_DEFINITELY_MISSING_KEY")
    );
}
