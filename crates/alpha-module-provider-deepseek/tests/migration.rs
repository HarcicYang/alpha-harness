use alpha_module_provider_deepseek::{DEFAULT_BASE_URL, DeepSeekProviderKind};
use alpha_provider_api::ProviderKind;
use serde_json::json;

#[test]
fn migrates_legacy_deepseek_endpoint_to_the_anthropic_compatible_base() {
    let config = json!({
        "base_url": "https://api.deepseek.com/v1",
        "headers": {},
        "options": {},
        "enable_web_search": true
    });
    let migrated = DeepSeekProviderKind
        .migrate_config(1, config)
        .expect("migration");
    assert_eq!(migrated["base_url"], DEFAULT_BASE_URL);
}
