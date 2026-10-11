use std::sync::Arc;

use alpha_kernel::{Ctx, Module};
use alpha_module_credential::{
    CredentialBackendStatus, CredentialConfig, CredentialError, CredentialModule, CredentialStore,
    MemoryCredentialStore, PlatformCredentialStore,
};

#[test]
fn memory_store_round_trips_and_deletes() {
    let store = MemoryCredentialStore::default();
    assert!(!store.contains("provider/test/api-key").expect("contains"));
    store
        .write("provider/test/api-key", b"secret")
        .expect("write");
    assert!(store.contains("provider/test/api-key").expect("contains"));
    assert_eq!(
        store.read("provider/test/api-key").expect("read"),
        b"secret"
    );
    store.delete("provider/test/api-key").expect("delete");
    assert!(matches!(
        store.read("provider/test/api-key"),
        Err(CredentialError::NotFound(_))
    ));
}

#[test]
fn invalid_auth_references_are_rejected() {
    let store = MemoryCredentialStore::default();
    assert!(matches!(
        store.write("", b"secret"),
        Err(CredentialError::InvalidReference(_))
    ));
}

#[test]
fn module_registers_and_disposes_the_store() {
    let ctx = Ctx::root();
    CredentialModule
        .activate(&ctx, &CredentialConfig::default())
        .expect("activate");
    assert!(ctx.get::<dyn CredentialStore>().is_some());
    ctx.dispose();
    assert!(ctx.get::<dyn CredentialStore>().is_none());
}

#[test]
fn platform_store_round_trips_when_available() {
    let store = PlatformCredentialStore::open();
    if store.status().status != CredentialBackendStatus::Available {
        return;
    }
    let auth_ref = format!("test/platform/{}", std::process::id());
    store.write(&auth_ref, b"platform-secret").expect("write");
    assert_eq!(store.read(&auth_ref).expect("read"), b"platform-secret");
    assert!(store.contains(&auth_ref).expect("contains"));
    store.delete(&auth_ref).expect("delete");
    assert!(!store.contains(&auth_ref).expect("contains"));
}

#[test]
fn backend_status_is_serializable() {
    let store: Arc<dyn CredentialStore> = Arc::new(MemoryCredentialStore::default());
    let status = store.status();
    let json = serde_json::to_value(status).expect("serialize");
    assert_eq!(json["backend"], "memory");
}
