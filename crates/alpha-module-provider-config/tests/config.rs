use std::sync::Arc;

use alpha_kernel::{Ctx, Module};
use alpha_module_provider_config::{
    LocalConfigFile, ProviderConfigConfig, ProviderConfigModule, ProviderConfigStore,
    TomlProviderConfigStore,
};
use alpha_provider_api::{ProviderApiModule, ProviderInstance, ProviderInstanceRegistry};
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
    assert_eq!(store.list().len(), 1);
    assert_eq!(store.default_instance().as_deref(), Some("test"));

    let reloaded = Arc::new(ProviderInstanceRegistry::new());
    let store = TomlProviderConfigStore::new(
        store.path().to_path_buf(),
        Arc::new(LocalConfigFile),
        reloaded.clone(),
    );
    store.reload().expect("reload");
    assert_eq!(reloaded.get("test").expect("instance").id, "test");
    assert!(store.remove("test").expect("remove"));
    assert!(reloaded.get("test").is_none());
}

#[test]
fn module_loads_the_config_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ctx = Ctx::root();
    ProviderApiModule.activate(&ctx, &()).expect("provider api");
    ProviderConfigModule
        .activate(
            &ctx,
            &ProviderConfigConfig {
                path: Some(dir.path().join("config.toml")),
            },
        )
        .expect("provider config");
    assert!(ctx.get::<dyn ProviderConfigStore>().is_some());
}
