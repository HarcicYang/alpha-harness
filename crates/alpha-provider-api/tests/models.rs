use std::sync::Arc;

use alpha_module_loop::ModelProvider;
use alpha_provider_api::{
    AuthMethodDescriptor, ModelDescriptor, ProviderBuildContext, ProviderDescriptor,
    ProviderInstance, ProviderKind, ProviderKindError, default_model_id, effective_model_catalog,
};
use serde_json::json;

struct TestKind;

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
                    provider_tools: Vec::new(),
                },
                ModelDescriptor {
                    id: "large".to_owned(),
                    name: "Large".to_owned(),
                    adapter: "test".to_owned(),
                    context_window: 2_000,
                    max_tokens: 200,
                    reasoning: true,
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
    assert_eq!(large.provider_tools, vec!["web_search".to_owned()]);
}
