//! Pure composition checks over declared modules.

use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error;

use crate::metadata::{CapabilityKind, SUPPORTED_API};

/// One module discovered in the dependency graph.
#[derive(Clone, Debug)]
pub struct ModuleInfo {
    /// Package name.
    pub package: String,
    /// Stable module key.
    pub key: String,
    /// Extern path used by generated code.
    pub extern_path: String,
    /// Metadata API version.
    pub api: u32,
    /// Provided capability keys with kinds.
    pub provides: Vec<(String, CapabilityKind)>,
    /// Required capability keys.
    pub requires: Vec<String>,
    /// Static registry entry names.
    pub registry_entries: Vec<String>,
    /// Config keys that must be present before activation.
    pub requires_config: Vec<String>,
}

/// Composition errors.
#[derive(Debug, Error)]
pub enum BuildError {
    /// A module declares an unsupported metadata API.
    #[error("module `{module}` declares alpha api {found}; this build supports api {supported}")]
    UnsupportedApi {
        /// Module key.
        module: String,
        /// Declared version.
        found: u32,
        /// Supported version.
        supported: u32,
    },
    /// Two modules provide the same singleton capability.
    #[error(
        "singleton capability `{capability}` is provided by {providers}; \
         keep one provider and remove the others from the dependency graph"
    )]
    SingletonConflict {
        /// Capability key.
        capability: String,
        /// Comma-separated provider module keys.
        providers: String,
    },
    /// A required capability has no provider.
    #[error("module `{module}` requires `{capability}`, but no enabled module provides it")]
    MissingRequirement {
        /// Requiring module key.
        module: String,
        /// Missing capability key.
        capability: String,
    },
    /// Two modules register the same static entry name.
    #[error("registry entry `{entry}` is provided by both `{first}` and `{second}`")]
    RegistryConflict {
        /// Entry name.
        entry: String,
        /// First module key.
        first: String,
        /// Second module key.
        second: String,
    },
    /// The dependency graph has no modules.
    #[error("no alpha modules found in the dependency graph; add module crates to Cargo.toml")]
    NoModules,
}

/// Validate a module set.
///
/// Two modules may not provide the same singleton capability: a replacement is
/// a dependency swap, not a coexistence, so the composition fails instead of
/// picking a winner.
pub fn check(modules: &[ModuleInfo]) -> Result<(), BuildError> {
    if modules.is_empty() {
        return Err(BuildError::NoModules);
    }
    for module in modules {
        if module.api != SUPPORTED_API {
            return Err(BuildError::UnsupportedApi {
                module: module.key.clone(),
                found: module.api,
                supported: SUPPORTED_API,
            });
        }
    }

    let mut singleton_providers: BTreeMap<&str, Vec<&ModuleInfo>> = BTreeMap::new();
    for module in modules {
        for (key, kind) in &module.provides {
            if *kind == CapabilityKind::Singleton {
                singleton_providers.entry(key).or_default().push(module);
            }
        }
    }
    let provided_keys: BTreeSet<&str> = modules
        .iter()
        .flat_map(|module| module.provides.iter().map(|(key, _)| key.as_str()))
        .collect();

    for (capability, providers) in &singleton_providers {
        if providers.len() > 1 {
            return Err(BuildError::SingletonConflict {
                capability: (*capability).to_owned(),
                providers: provider_names(providers),
            });
        }
    }

    for module in modules {
        for required in &module.requires {
            if !provided_keys.contains(required.as_str()) {
                return Err(BuildError::MissingRequirement {
                    module: module.key.clone(),
                    capability: required.clone(),
                });
            }
        }
    }

    let mut entries: BTreeMap<&str, &str> = BTreeMap::new();
    for module in modules {
        for entry in &module.registry_entries {
            if let Some(first) = entries.insert(entry, &module.key) {
                return Err(BuildError::RegistryConflict {
                    entry: entry.clone(),
                    first: first.to_owned(),
                    second: module.key.clone(),
                });
            }
        }
    }

    Ok(())
}

fn provider_names(providers: &[&ModuleInfo]) -> String {
    providers
        .iter()
        .map(|module| module.key.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}
