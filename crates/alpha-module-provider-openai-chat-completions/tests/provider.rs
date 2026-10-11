use std::collections::BTreeMap;
use std::sync::Arc;

use alpha_api_openai_chat::OpenAiChatApiModule;
use alpha_kernel::{Ctx, Module};
use alpha_module_http::{HttpClient, HttpError, HttpRequest, HttpResponse};
use alpha_module_provider_openai_chat_completions::{OpenAiChatProviderModule, PROVIDER_KIND};
use alpha_provider_api::{
    ApiAdapterRegistry, ProviderApiModule, ProviderBuildContext, ProviderInstance,
    ProviderKindRegistry,
};
use futures::future::BoxFuture;
use serde_json::json;

struct NoopHttp;

impl HttpClient for NoopHttp {
    fn request(&self, _request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, HttpError>> {
        Box::pin(async move {
            Ok(HttpResponse {
                status: 200,
                headers: BTreeMap::new(),
                body: Vec::new(),
            })
        })
    }
}

#[test]
fn registers_and_builds_a_provider_instance() {
    let ctx = Ctx::root();
    ProviderApiModule.activate(&ctx, &()).expect("provider api");
    OpenAiChatApiModule
        .activate(&ctx, &())
        .expect("openai adapter");
    OpenAiChatProviderModule
        .activate(&ctx, &())
        .expect("provider kind");

    let registry = ctx
        .require::<ProviderKindRegistry>()
        .expect("provider registry");
    let kind = registry.get(PROVIDER_KIND).expect("provider kind");
    assert_eq!(kind.descriptor().kind, PROVIDER_KIND);

    let adapter_registry = ctx
        .require::<ApiAdapterRegistry>()
        .expect("adapter registry");
    let instance = ProviderInstance {
        id: "test".to_owned(),
        kind: PROVIDER_KIND.to_owned(),
        config_version: 1,
        config: json!({ "base_url": "https://api.test/v1" }),
        auth_ref: Some("provider/test/api-key".to_owned()),
        model_overrides: json!({}),
    };
    let _provider = kind
        .build(
            &instance,
            &ProviderBuildContext {
                adapters: adapter_registry,
                http: Arc::new(NoopHttp),
                credential: Some(b"test-key".to_vec()),
            },
        )
        .expect("build provider");
}
