//! Credential storage contract.
//!
//! The fixed shell and provider modules talk to this trait only, so the
//! storage implementation is a replaceable dependency rather than a fixed one.

#![forbid(unsafe_code)]

pub use alpha_schema::{CredentialBackendStatus, CredentialStatus};

/// Credential storage failures.
#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    /// No credential exists for the reference.
    #[error("credential `{0}` is not configured")]
    NotFound(String),
    /// The backend is unavailable.
    #[error("credential backend is unavailable: {0}")]
    Unavailable(String),
    /// The auth reference is invalid.
    #[error("invalid auth reference: {0}")]
    InvalidReference(String),
    /// The platform store rejected the operation.
    #[error("credential operation failed: {0}")]
    Backend(String),
}

/// One secret store keyed by stable auth references.
pub trait CredentialStore: Send + Sync + 'static {
    /// Return backend availability.
    fn status(&self) -> CredentialStatus;
    /// Read one secret.
    fn read(&self, auth_ref: &str) -> Result<Vec<u8>, CredentialError>;
    /// Create or replace one secret.
    fn write(&self, auth_ref: &str, secret: &[u8]) -> Result<(), CredentialError>;
    /// Delete one secret. Missing entries are treated as already deleted.
    fn delete(&self, auth_ref: &str) -> Result<(), CredentialError>;
    /// Whether one secret exists.
    fn contains(&self, auth_ref: &str) -> Result<bool, CredentialError>;
}
