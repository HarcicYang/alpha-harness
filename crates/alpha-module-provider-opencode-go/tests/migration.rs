use alpha_module_provider_opencode_go::{DEFAULT_BASE_URL, OpenCodeGoProviderKind};
use alpha_provider_api::{ProviderKind, ReasoningLevel};
use serde_json::json;

#[test]
fn migrates_legacy_base_url_and_exposes_current_catalog() {
    let kind = OpenCodeGoProviderKind;
    let config = kind
        .migrate_config(
            1,
            json!({
                "base_url": "https://opencode.ai/zen/v1",
                "adapter": "anthropic-messages",
                "headers": {},
                "options": {},
                "session_id": null
            }),
        )
        .expect("migration");
    assert_eq!(config["base_url"], json!(DEFAULT_BASE_URL));
    assert_eq!(kind.config_version(), 2);

    let models = kind.descriptor().models;
    assert_eq!(models[0].id, "deepseek-v4.1-flash");
    assert_eq!(
        models[0].reasoning_levels,
        vec![
            ReasoningLevel::Auto,
            ReasoningLevel::Low,
            ReasoningLevel::High,
            ReasoningLevel::Max
        ]
    );
}
