//! Module contract and capability declarations.
//!
//! Modules declare their requirements, configuration, and provided
//! capabilities statically. Composition checks read the same declarations
//! through crate metadata at build time.

use std::fmt;

use serde::de::DeserializeOwned;

use crate::context::Ctx;

/// Where a capability lives in the scope chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityScope {
    /// One instance shared by the whole runtime.
    Global,
    /// One instance per project or workspace Location.
    Location,
    /// One instance per agent.
    Agent,
}

/// How many providers a capability accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityKind {
    /// Exactly one provider; a second provider is a composition error.
    Singleton,
    /// Multiple providers keyed by name.
    Registry,
}

/// Static declaration of one capability provided by a module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapabilitySpec {
    /// Stable capability key, for example `tool.bash` or `session.store`.
    pub key: &'static str,
    /// Singleton or registry.
    pub kind: CapabilityKind,
    /// Scope level that owns the capability.
    pub scope: CapabilityScope,
}

impl CapabilitySpec {
    /// Declare a singleton capability.
    pub const fn singleton(key: &'static str, scope: CapabilityScope) -> Self {
        Self {
            key,
            kind: CapabilityKind::Singleton,
            scope,
        }
    }

    /// Declare a registry capability.
    pub const fn registry(key: &'static str, scope: CapabilityScope) -> Self {
        Self {
            key,
            kind: CapabilityKind::Registry,
            scope,
        }
    }
}

/// Static module description used by composition checks and tooling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModuleDescriptor {
    /// Stable module key.
    pub key: &'static str,
    /// Capabilities provided by the module.
    pub capabilities: &'static [CapabilitySpec],
}

impl ModuleDescriptor {
    /// Build a descriptor from static parts.
    pub const fn new(key: &'static str, capabilities: &'static [CapabilitySpec]) -> Self {
        Self { key, capabilities }
    }

    /// Build a descriptor from a module type.
    pub fn of<M: Module>() -> Self {
        Self {
            key: M::KEY,
            capabilities: M::CAPABILITIES,
        }
    }
}

/// Failure while activating a module.
pub struct ActivateError {
    module: &'static str,
    source: Box<dyn std::error::Error + Send + Sync + 'static>,
}

impl ActivateError {
    /// Wrap an underlying error with the module key.
    pub fn new(
        module: &'static str,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            module,
            source: Box::new(source),
        }
    }

    /// Build an activation error from a plain message.
    pub fn message(module: &'static str, message: impl Into<String>) -> Self {
        Self {
            module,
            source: Box::from(message.into()),
        }
    }

    /// The module that failed to activate.
    pub fn module(&self) -> &'static str {
        self.module
    }
}

impl fmt::Debug for ActivateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ActivateError")
            .field("module", &self.module)
            .field("source", &self.source)
            .finish()
    }
}

impl fmt::Display for ActivateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "module `{}` failed to activate: {}",
            self.module, self.source
        )
    }
}

impl std::error::Error for ActivateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.source)
    }
}

/// The contract every module implements.
pub trait Module: Send + Sync + 'static {
    /// Capabilities this module requires, checked by composition tooling.
    type Requirements;

    /// Typed configuration, layered from runtime configuration files.
    type Config: DeserializeOwned + Default;

    /// Stable module key.
    const KEY: &'static str;

    /// Capabilities provided by this module.
    const CAPABILITIES: &'static [CapabilitySpec] = &[];

    /// Activate the module against a context.
    ///
    /// Every registration made here must be a reversible effect owned by the
    /// supplied context or one of its scopes.
    fn activate(&self, ctx: &Ctx, config: &Self::Config) -> Result<(), ActivateError>;
}
