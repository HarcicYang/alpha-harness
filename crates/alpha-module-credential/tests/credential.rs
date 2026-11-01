use std::sync::Arc;

use alpha_kernel::{Ctx, Module};
use alpha_module_credential::{
    CredentialConfig, CredentialError, CredentialModule, CredentialStore, MemoryCredentialStore,
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
fn backend_status_is_serializable() {
    let store: Arc<dyn CredentialStore> = Arc::new(MemoryCredentialStore::default());
    let status = store.status();
    let json = serde_json::to_value(status).expect("serialize");
    assert_eq!(json["backend"], "memory");
}
