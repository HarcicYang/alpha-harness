//! Cross-platform credential storage.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Arc;

use alpha_kernel::{ActivateError, CapabilityScope, CapabilitySpec, Ctx, Module};
use keyring_core::{CredentialStore as KeyringCredentialStore, Entry, Error as KeyringError};
use serde::Deserialize;

pub use alpha_credential_api::{CredentialBackendStatus, CredentialError, CredentialStatus, CredentialStore};

/// Stable module key used by the generated composition table.
pub const __ALPHA_MODULE__: &str = "credential";

const SERVICE: &str = "alpha-harness";

/// Platform keyring-backed store.
pub struct PlatformCredentialStore {
    store: Option<Arc<KeyringCredentialStore>>,
    status: CredentialStatus,
}

impl PlatformCredentialStore {
    /// Open the platform backend without failing core startup.
    pub fn open() -> Self {
        match platform_store() {
            Ok((backend, name)) => Self {
                store: Some(backend),
                status: CredentialStatus {
                    backend: name.to_owned(),
                    status: CredentialBackendStatus::Available,
                    detail: None,
                },
            },
            Err(error) => Self {
                store: None,
                status: CredentialStatus {
                    backend: platform_name().to_owned(),
                    status: CredentialBackendStatus::Unavailable,
                    detail: Some(error),
                },
            },
        }
    }

    /// Construct a store over an injected keyring backend for tests.
    pub fn with_backend(backend: Arc<KeyringCredentialStore>, backend_name: &str) -> Self {
        Self {
            store: Some(backend),
            status: CredentialStatus {
                backend: backend_name.to_owned(),
                status: CredentialBackendStatus::Available,
                detail: None,
            },
        }
    }

    fn entry(&self, auth_ref: &str) -> Result<Entry, CredentialError> {
        validate_ref(auth_ref)?;
        let store = self.store.as_ref().ok_or_else(|| {
            CredentialError::Unavailable(self.status.detail.clone().unwrap_or_default())
        })?;
        store
            .build(SERVICE, auth_ref, None)
            .map_err(map_keyring_error)
    }
}

impl CredentialStore for PlatformCredentialStore {
    fn status(&self) -> CredentialStatus {
        self.status.clone()
    }

    fn read(&self, auth_ref: &str) -> Result<Vec<u8>, CredentialError> {
        match self.entry(auth_ref)?.get_secret() {
            Ok(secret) => Ok(secret),
            Err(KeyringError::NoEntry) => Err(CredentialError::NotFound(auth_ref.to_owned())),
            Err(error) => Err(map_keyring_error(error)),
        }
    }

    fn write(&self, auth_ref: &str, secret: &[u8]) -> Result<(), CredentialError> {
        self.entry(auth_ref)?
            .set_secret(secret)
            .map_err(map_keyring_error)
    }

    fn delete(&self, auth_ref: &str) -> Result<(), CredentialError> {
        match self.entry(auth_ref)?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(error) => Err(map_keyring_error(error)),
        }
    }

    fn contains(&self, auth_ref: &str) -> Result<bool, CredentialError> {
        match self.entry(auth_ref)?.get_secret() {
            Ok(_) => Ok(true),
            Err(KeyringError::NoEntry) => Ok(false),
            Err(error) => Err(map_keyring_error(error)),
        }
    }
}

impl std::fmt::Debug for PlatformCredentialStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PlatformCredentialStore")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

/// Configuration for credential storage.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct CredentialConfig {}

/// Registers the credential store service.
pub struct CredentialModule;

impl Module for CredentialModule {
    type Requirements = ();
    type Config = CredentialConfig;

    const KEY: &'static str = "credential";
    const CAPABILITIES: &'static [CapabilitySpec] = &[CapabilitySpec::singleton(
        "credential.store",
        CapabilityScope::Global,
    )];

    fn activate(&self, ctx: &Ctx, _config: &Self::Config) -> Result<(), ActivateError> {
        let store: Arc<dyn CredentialStore> = Arc::new(PlatformCredentialStore::open());
        ctx.provide::<dyn CredentialStore>(store)
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        Ok(())
    }
}

fn validate_ref(auth_ref: &str) -> Result<(), CredentialError> {
    if auth_ref.trim().is_empty() {
        return Err(CredentialError::InvalidReference(
            "auth reference is empty".to_owned(),
        ));
    }
    Ok(())
}

fn map_keyring_error(error: KeyringError) -> CredentialError {
    match error {
        KeyringError::NoEntry => CredentialError::NotFound("credential".to_owned()),
        KeyringError::NoStorageAccess(error) | KeyringError::PlatformFailure(error) => {
            CredentialError::Unavailable(error.to_string())
        }
        other => CredentialError::Backend(other.to_string()),
    }
}

#[cfg(target_os = "linux")]
fn platform_store() -> Result<(Arc<KeyringCredentialStore>, &'static str), String> {
    zbus_secret_service_keyring_store::Store::new()
        .map(|store| (store as Arc<KeyringCredentialStore>, "secret-service"))
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "macos")]
fn platform_store() -> Result<(Arc<KeyringCredentialStore>, &'static str), String> {
    apple_native_keyring_store::keychain::Store::new()
        .map(|store| (store as Arc<KeyringCredentialStore>, "keychain"))
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
fn platform_store() -> Result<(Arc<KeyringCredentialStore>, &'static str), String> {
    windows_native_keyring_store::Store::new()
        .map(|store| (store as Arc<KeyringCredentialStore>, "credential-manager"))
        .map_err(|error| error.to_string())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn platform_store() -> Result<(Arc<KeyringCredentialStore>, &'static str), String> {
    Err("unsupported credential platform".to_owned())
}

fn platform_name() -> &'static str {
    if cfg!(target_os = "linux") {
        "secret-service"
    } else if cfg!(target_os = "macos") {
        "keychain"
    } else if cfg!(target_os = "windows") {
        "credential-manager"
    } else {
        "unsupported"
    }
}

/// In-memory backend used by tests and embedding.
#[derive(Default)]
pub struct MemoryCredentialStore {
    secrets: std::sync::Mutex<HashMap<String, Vec<u8>>>,
}

impl CredentialStore for MemoryCredentialStore {
    fn status(&self) -> CredentialStatus {
        CredentialStatus {
            backend: "memory".to_owned(),
            status: CredentialBackendStatus::Available,
            detail: None,
        }
    }

    fn read(&self, auth_ref: &str) -> Result<Vec<u8>, CredentialError> {
        validate_ref(auth_ref)?;
        self.secrets
            .lock()
            .expect("credential lock poisoned")
            .get(auth_ref)
            .cloned()
            .ok_or_else(|| CredentialError::NotFound(auth_ref.to_owned()))
    }

    fn write(&self, auth_ref: &str, secret: &[u8]) -> Result<(), CredentialError> {
        validate_ref(auth_ref)?;
        self.secrets
            .lock()
            .expect("credential lock poisoned")
            .insert(auth_ref.to_owned(), secret.to_vec());
        Ok(())
    }

    fn delete(&self, auth_ref: &str) -> Result<(), CredentialError> {
        validate_ref(auth_ref)?;
        self.secrets
            .lock()
            .expect("credential lock poisoned")
            .remove(auth_ref);
        Ok(())
    }

    fn contains(&self, auth_ref: &str) -> Result<bool, CredentialError> {
        validate_ref(auth_ref)?;
        Ok(self
            .secrets
            .lock()
            .expect("credential lock poisoned")
            .contains_key(auth_ref))
    }
}
