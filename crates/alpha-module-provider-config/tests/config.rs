use std::sync::Arc;

use alpha_module_provider_config::{LocalConfigFile, ProviderConfigStore, TomlProviderConfigStore};
use alpha_provider_api::{ProviderInstance, ProviderInstanceRegistry};
use serde_json::json;

fn instance(id: &str) -> ProviderInstance {
    ProviderInstance {
        id: id.to_owned(),
        kind: "openai-chat-completions".to_owned(),
        config_version: 1,
        config: json!({ "base_url": "https://api.test/v1" }),
        auth_ref: Some(format!("provider/{id}/api-key")),
        model_overrides: json!({}),
        disabled: false,
    }
}

#[test]
fn toml_store_round_trips_instances() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let registry = Arc::new(ProviderInstanceRegistry::new());
    let store = TomlProviderConfigStore::new(path, Arc::new(LocalConfigFile), registry.clone());

    store.upsert(instance("test")).expect("upsert");
    store
        .set_default_instance(Some("test".to_owned()))
        .expect("default");
    assert!(
        store
            .set_default_model("test", "deepseek-v4.1-flash")
            .expect("default model")
    );
    assert_eq!(store.list().len(), 1);
    assert_eq!(store.default_instance().as_deref(), Some("test"));
    assert_eq!(
        registry
            .get("test")
            .expect("runtime instance")
            .model_overrides["default"],
        json!("deepseek-v4.1-flash")
    );

    let reloaded = Arc::new(ProviderInstanceRegistry::new());
    let store = TomlProviderConfigStore::new(
        store.path().to_path_buf(),
        Arc::new(LocalConfigFile),
        reloaded.clone(),
    );
    store.reload().expect("reload");
    let persisted = reloaded.get("test").expect("instance");
    assert_eq!(persisted.id, "test");
    assert_eq!(
        persisted.model_overrides["default"],
        json!("deepseek-v4.1-flash")
    );
    assert!(store.remove("test").expect("remove"));
    assert!(reloaded.get("test").is_none());
}

#[test]
fn toml_store_omits_null_config_fields() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let registry = Arc::new(ProviderInstanceRegistry::new());
    let store = TomlProviderConfigStore::new(path, Arc::new(LocalConfigFile), registry);
    let mut instance = instance("opencode-go");
    instance.kind = "opencode-go".to_owned();
    instance.config = json!({
        "base_url": "https://opencode.ai/zen/go/v1",
        "session_id": null,
    });

    store.upsert(instance).expect("upsert");
    let reloaded = Arc::new(ProviderInstanceRegistry::new());
    let store = TomlProviderConfigStore::new(
        store.path().to_path_buf(),
        Arc::new(LocalConfigFile),
        reloaded.clone(),
    );
    store.reload().expect("reload");
    let persisted = reloaded.get("opencode-go").expect("persisted instance");
    assert!(persisted.config.get("session_id").is_none());
}
