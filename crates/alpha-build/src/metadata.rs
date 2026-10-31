//! `[package.metadata.alpha]` shapes.

use serde::Deserialize;

/// Metadata API version understood by this build helper.
pub const SUPPORTED_API: u32 = 1;

/// What a package contributes to a composition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageKind {
    /// A module: provides capabilities and activates at one scope.
    Module,
    /// A composition-only crate that depends on modules.
    Bundle,
    /// A contract crate: provides capabilities but has nothing to activate.
    Contract,
    /// A frontend crate: contributes command-line commands.
    Front,
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

/// Scope a module activates at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModuleScope {
    /// One activation for the whole core.
    Global,
    /// One activation per workspace Location.
    Location,
    /// One activation per session actor.
    Agent,
}

/// One command contributed by a frontend crate.
#[derive(Clone, Debug, Deserialize)]
pub struct FrontCommandMetadata {
    /// Subcommand name.
    pub name: String,
    /// One-line help text.
    pub about: String,
    /// Function in the frontend crate with signature
    /// `fn(FrontRequest) -> Result<(), FrontError>`.
    pub entry: String,
    /// Whether the command takes the standard option set and a prompt.
    #[serde(default)]
    pub takes_prompt: bool,
}

/// Package metadata.
#[derive(Clone, Debug, Deserialize)]
pub struct ModuleMetadata {
    /// Metadata API version.
    pub api: u32,
    /// Module, bundle, contract, or front.
    pub kind: PackageKind,
    /// Stable module key. Absent for frontend crates.
    #[serde(default)]
    pub key: String,
    /// Activation scope. Required for `kind = "module"`.
    #[serde(default)]
    pub scope: Option<ModuleScope>,
    /// Module type exported at the crate root. Required for `kind = "module"`.
    #[serde(default)]
    pub module: Option<String>,
    /// Config type exported at the crate root. Required for `kind = "module"`.
    #[serde(default)]
    pub config: Option<String>,
    /// Config keys that must be present before activation.
    #[serde(default)]
    pub requires_config: Vec<String>,
    /// Provided capabilities.
    #[serde(default)]
    pub provides: Vec<Capability>,
    /// Required capability keys.
    #[serde(default)]
    pub requires: Vec<String>,
    /// Static registry entry names.
    #[serde(default)]
    pub registry_entries: Vec<String>,
    /// Commands contributed by a frontend crate.
    #[serde(default)]
    pub commands: Vec<FrontCommandMetadata>,
}
