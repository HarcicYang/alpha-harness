use std::sync::Arc;
use std::sync::Mutex;

use alpha_module_http::{HttpClient, HttpError, HttpRequest, HttpResponse};
use alpha_module_loop::ModelProvider;
use alpha_provider_api::{
    AuthMethodDescriptor, ModelDescriptor, ProviderBuildContext, ProviderDescriptor,
    ProviderInstance, ProviderKind, ProviderKindError, ReasoningLevel, default_model_id,
    discover_models, effective_model_catalog,
};
use futures::future::BoxFuture;
use serde_json::json;

struct TestKind;

struct CatalogHttp {
    request: Mutex<Option<HttpRequest>>,
}

impl HttpClient for CatalogHttp {
    fn request(&self, request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, HttpError>> {
        *self.request.lock().expect("request lock") = Some(request);
        Box::pin(async {
            Ok(HttpResponse {
                status: 200,
                headers: Default::default(),
                body: json!({
                    "data": [
                        { "id": "large", "display_name": "Large Dynamic" },
                        { "id": "new-model", "name": "New Model" }
                    ]
                })
                .to_string()
                .into_bytes(),
            })
        })
    }
}

impl ProviderKind for TestKind {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            kind: "test".to_owned(),
            name: "Test".to_owned(),
            config_schema: json!({}),
            default_config: json!({}),
            auth_methods: vec![AuthMethodDescriptor {
                key: "api_key".to_owned(),
                label: "API key".to_owned(),
                interactive: true,
            }],
            models: vec![
                ModelDescriptor {
                    id: "small".to_owned(),
                    name: "Small".to_owned(),
                    adapter: "test".to_owned(),
                    context_window: 1_000,
                    max_tokens: 100,
                    reasoning: false,
                    reasoning_levels: vec![ReasoningLevel::Auto],
                    provider_tools: Vec::new(),
                },
                ModelDescriptor {
                    id: "large".to_owned(),
                    name: "Large".to_owned(),
                    adapter: "test".to_owned(),
                    context_window: 2_000,
                    max_tokens: 200,
                    reasoning: true,
                    reasoning_levels: ReasoningLevel::ALL.to_vec(),
                    provider_tools: Vec::new(),
                },
            ],
            adapters: Vec::new(),
        }
    }

    fn config_version(&self) -> u32 {
        1
    }

    fn build(
        &self,
        _instance: &ProviderInstance,
        _context: &ProviderBuildContext,
    ) -> Result<Arc<dyn ModelProvider>, ProviderKindError> {
        Err(ProviderKindError::Build("not used".to_owned()))
    }
}

#[test]
fn user_overrides_change_catalog_metadata_and_default() {
    let instance = ProviderInstance {
        id: "test".to_owned(),
        kind: "test".to_owned(),
        config_version: 1,
        config: json!({}),
        auth_ref: None,
        model_overrides: json!({
            "default": "large",
            "large": {
                "name": "Large custom",
                "context_window": 999_999,
                "thinking_levels": ["auto", "off", "high"],
                "provider_tools": ["web_search"]
            }
        }),
        disabled: false,
    };
    let kind = TestKind;
    assert_eq!(default_model_id(&instance, &kind).as_deref(), Some("large"));
    let catalog = effective_model_catalog(&instance, &kind);
    let large = catalog
        .iter()
        .find(|model| model.id == "large")
        .expect("large model");
    assert_eq!(large.name, "Large custom");
    assert_eq!(large.context_window, 999_999);
    assert_eq!(
        large.reasoning_levels,
        vec![
            ReasoningLevel::Auto,
            ReasoningLevel::Off,
            ReasoningLevel::High
        ]
    );
    assert_eq!(large.provider_tools, vec!["web_search".to_owned()]);
}

#[test]
fn dynamic_catalog_prioritizes_provider_models() {
    let http = Arc::new(CatalogHttp {
        request: Mutex::new(None),
    });
    let instance = ProviderInstance {
        id: "test".to_owned(),
        kind: "test".to_owned(),
        config_version: 1,
        config: json!({ "base_url": "https://api.test/v1" }),
        auth_ref: None,
        model_overrides: json!({}),
        disabled: false,
    };
    let models = futures::executor::block_on(discover_models(
        &instance,
        &TestKind,
        http.clone(),
        Some(b"secret".to_vec()),
    ))
    .expect("dynamic models");
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "large");
    assert_eq!(models[0].name, "Large Dynamic");
    assert_eq!(models[1].id, "new-model");

    let request = http
        .request
        .lock()
        .expect("request lock")
        .clone()
        .expect("request");
    assert_eq!(request.url, "https://api.test/v1/models");
    assert_eq!(
        request.headers.get("authorization").map(String::as_str),
        Some("Bearer secret")
    );
}
