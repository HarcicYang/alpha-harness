use std::sync::Arc;
use std::sync::Mutex;

use alpha_kernel::Ctx;
use alpha_module_http::{HttpClient, HttpError, HttpRequest, HttpResponse};
use alpha_module_loop::{
    ChatMessage, CompletionChunk, CompletionRequest, ModelProvider, ModelRoute, ModelRouteOverride,
};
use alpha_provider_api::{
    AdapterCapabilities, AdapterProvider, AdapterRequest, ApiAdapter, AuthMethodDescriptor,
    DynamicProvider, ModelDescriptor, ProviderBuildContext, ProviderDescriptor, ProviderInstance,
    ProviderInstanceRegistry, ProviderKind, ProviderKindError, ProviderKindRegistry,
    ReasoningLevel, default_model_id, discover_models, effective_model_catalog,
};
use alpha_schema::AssistantContentBlock;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use serde_json::json;

struct TestKind;

struct CatalogHttp {
    request: Mutex<Option<HttpRequest>>,
}

struct CaptureAdapter {
    request: Mutex<Option<AdapterRequest>>,
}

struct StreamingAdapter {
    chunks: Mutex<Option<Vec<Result<CompletionChunk, alpha_module_loop::ProviderError>>>>,
}

impl ApiAdapter for CaptureAdapter {
    fn key(&self) -> &'static str {
        "capture"
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            streaming: true,
            tool_calls: true,
            provider_tools: false,
        }
    }

    fn stream(
        &self,
        _client: Arc<dyn HttpClient>,
        request: AdapterRequest,
    ) -> BoxStream<'static, Result<CompletionChunk, alpha_module_loop::ProviderError>> {
        *self.request.lock().expect("request lock") = Some(request);
        Box::pin(futures::stream::empty())
    }
}

impl ApiAdapter for StreamingAdapter {
    fn key(&self) -> &'static str {
        "streaming"
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            streaming: true,
            tool_calls: false,
            provider_tools: false,
        }
    }

    fn stream(
        &self,
        _client: Arc<dyn HttpClient>,
        _request: AdapterRequest,
    ) -> BoxStream<'static, Result<CompletionChunk, alpha_module_loop::ProviderError>> {
        let chunks = self
            .chunks
            .lock()
            .expect("chunks lock")
            .take()
            .unwrap_or_default();
        Box::pin(futures::stream::iter(chunks))
    }
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
fn dynamic_provider_reports_effective_model_context_windows() {
    let instance = ProviderInstance {
        id: "test".to_owned(),
        kind: "test".to_owned(),
        config_version: 1,
        config: json!({}),
        auth_ref: None,
        model_overrides: json!({
            "large": {
                "context_window": 999_999
            }
        }),
        disabled: false,
    };
    let ctx = Ctx::root();
    let kinds = Arc::new(ProviderKindRegistry::new());
    kinds
        .register(Arc::new(TestKind))
        .expect("register provider kind");
    let instances = Arc::new(ProviderInstanceRegistry::new());
    instances.upsert(instance);
    ctx.provide(kinds).expect("provide kinds");
    ctx.provide(instances).expect("provide instances");
    let route = Arc::new(ModelRouteOverride::new(ModelRoute::new(
        "test",
        "large",
        ReasoningLevel::Auto,
    )));
    let provider = DynamicProvider::new_routed(ctx, "test", route);

    assert_eq!(provider.context_window("large"), Some(999_999));
    assert_eq!(provider.context_window("small"), Some(1_000));
    assert_eq!(provider.context_window("missing"), None);
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

#[test]
fn session_header_is_added_to_provider_requests() {
    let adapter = Arc::new(CaptureAdapter {
        request: Mutex::new(None),
    });
    let provider = Arc::new(
        AdapterProvider::new(
            adapter.clone(),
            Arc::new(CatalogHttp {
                request: Mutex::new(None),
            }),
            "https://api.test/v1",
            None,
            Default::default(),
            Vec::new(),
            json!({}),
        )
        .with_session_header("x-opencode-session"),
    );
    let _stream = provider.stream(CompletionRequest {
        session_id: "session-123".to_owned(),
        model: "deepseek-v4.1-flash".to_owned(),
        reasoning: ReasoningLevel::Auto,
        messages: vec![ChatMessage::User("hello".to_owned())],
        tools: Vec::new(),
        provider_tools: Vec::new(),
        max_output_tokens: None,
    });

    let request = adapter
        .request
        .lock()
        .expect("request lock")
        .clone()
        .expect("adapter request");
    assert_eq!(
        request
            .headers
            .get("x-opencode-session")
            .map(String::as_str),
        Some("session-123")
    );
}

#[test]
fn model_specific_adapter_receives_route_history_and_provider_tools() {
    let default_adapter = Arc::new(CaptureAdapter {
        request: Mutex::new(None),
    });
    let model_adapter = Arc::new(CaptureAdapter {
        request: Mutex::new(None),
    });
    let provider = Arc::new(
        AdapterProvider::new(
            default_adapter.clone(),
            Arc::new(CatalogHttp {
                request: Mutex::new(None),
            }),
            "https://api.test/v1",
            None,
            Default::default(),
            vec!["web_search".to_owned()],
            json!({}),
        )
        .with_model_adapters(std::collections::BTreeMap::from([(
            "routed-model".to_owned(),
            model_adapter.clone() as Arc<dyn ApiAdapter>,
        )])),
    );
    let _stream = provider.stream(CompletionRequest {
        session_id: "session-123".to_owned(),
        model: "routed-model".to_owned(),
        reasoning: ReasoningLevel::Max,
        messages: vec![
            ChatMessage::AssistantContent(vec![AssistantContentBlock::Thinking {
                text: "remembered reasoning".to_owned(),
                signature: Some("signature".to_owned()),
            }]),
            ChatMessage::User("next".to_owned()),
        ],
        tools: Vec::new(),
        provider_tools: vec!["web_search".to_owned()],
        max_output_tokens: None,
    });

    assert!(
        default_adapter
            .request
            .lock()
            .expect("request lock")
            .is_none()
    );
    let request = model_adapter
        .request
        .lock()
        .expect("request lock")
        .clone()
        .expect("model adapter request");
    assert_eq!(request.reasoning, ReasoningLevel::Max);
    assert_eq!(request.provider_tools, vec!["web_search".to_owned()]);
    assert!(request.messages.iter().any(|message| matches!(
        message,
        ChatMessage::AssistantContent(blocks)
            if blocks.iter().any(|block| matches!(
                block,
                AssistantContentBlock::Thinking { text, signature }
                    if text == "remembered reasoning"
                        && signature.as_deref() == Some("signature")
            ))
    )));
}

#[test]
fn complete_preserves_streamed_content_order() {
    let adapter = Arc::new(StreamingAdapter {
        chunks: Mutex::new(Some(vec![
            Ok(CompletionChunk::Thinking {
                text: "plan ".to_owned(),
                signature: None,
            }),
            Ok(CompletionChunk::Thinking {
                text: "more".to_owned(),
                signature: None,
            }),
            Ok(CompletionChunk::Text("answer".to_owned())),
            Ok(CompletionChunk::Thinking {
                text: "after".to_owned(),
                signature: None,
            }),
            Ok(CompletionChunk::Text(" again".to_owned())),
        ])),
    });
    let provider = AdapterProvider::new(
        adapter,
        Arc::new(CatalogHttp {
            request: Mutex::new(None),
        }),
        "https://api.test/v1",
        None,
        Default::default(),
        Vec::new(),
        json!({}),
    );
    let response = futures::executor::block_on(provider.complete(CompletionRequest {
        session_id: "session-123".to_owned(),
        model: "streaming-model".to_owned(),
        reasoning: ReasoningLevel::Auto,
        messages: vec![ChatMessage::User("hello".to_owned())],
        tools: Vec::new(),
        provider_tools: Vec::new(),
        max_output_tokens: None,
    }))
    .expect("completion");

    assert_eq!(response.text, "answer again");
    assert_eq!(
        response.blocks,
        vec![
            AssistantContentBlock::Thinking {
                text: "plan more".to_owned(),
                signature: None,
            },
            AssistantContentBlock::Text {
                text: "answer".to_owned(),
            },
            AssistantContentBlock::Thinking {
                text: "after".to_owned(),
                signature: None,
            },
            AssistantContentBlock::Text {
                text: " again".to_owned(),
            },
        ]
    );
}
