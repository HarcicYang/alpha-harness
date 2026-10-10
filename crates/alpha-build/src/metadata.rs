//! `[package.metadata.alpha]` shapes.

use serde::Deserialize;

/// Metadata API version understood by this build helper.
pub const SUPPORTED_API: u32 = 1;

/// Whether a package is a module or a bundle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageKind {
    /// A module that provides capabilities.
    Module,
    /// A composition-only crate that depends on modules.
    Bundle,
}

/// Capability provider kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CapabilityKind {
    /// Exactly one provider is allowed.
    Singleton,
    /// Multiple providers are keyed by name.
    Registry,
}

/// Capability scope level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CapabilityScope {
    /// Shared by the whole runtime.
    Global,
    /// Per project or workspace Location.
    Location,
    /// Per agent.
    Agent,
}

/// One declared capability.
#[derive(Clone, Debug, Deserialize)]
pub struct Capability {
    /// Stable capability key.
    pub key: String,
    /// Singleton or registry.
    pub kind: CapabilityKind,
    /// Scope level.
    pub scope: CapabilityScope,
}

/// Module metadata.
#[derive(Clone, Debug, Deserialize)]
pub struct ModuleMetadata {
    /// Metadata API version.
    pub api: u32,
    /// Module or bundle.
    pub kind: PackageKind,
    /// Stable module key.
    pub key: String,
    /// Provided capabilities.
    #[serde(default)]
    pub provides: Vec<Capability>,
    /// Required capability keys.
    #[serde(default)]
    pub requires: Vec<String>,
    /// Static registry entry names.
    #[serde(default)]
    pub registry_entries: Vec<String>,
}

/// Application-level metadata.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AppMetadata {
    /// `capability=module` override declarations.
    #[serde(default)]
    pub overrides: Vec<String>,
}
