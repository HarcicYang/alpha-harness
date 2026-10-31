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
         add an override in [package.metadata.alpha], for example `{capability}=<module>`"
    )]
    SingletonConflict {
        /// Capability key.
        capability: String,
        /// Comma-separated provider module keys.
        providers: String,
    },
    /// An override names a capability or module that does not match.
    #[error(
        "override `{capability}={chosen}` does not select a provider of that capability; \
         providers are {providers}"
    )]
    InvalidOverride {
        /// Capability key.
        capability: String,
        /// Requested provider.
        chosen: String,
        /// Comma-separated provider module keys.
        providers: String,
    },
    /// An override targets a capability that no module provides.
    #[error("override targets unknown capability `{capability}`")]
    UnknownOverride {
        /// Capability key.
        capability: String,
    },
    /// An override targets a registry capability.
    #[error("capability `{capability}` is a registry and cannot be overridden")]
    NotOverridable {
        /// Capability key.
        capability: String,
    },
    /// An override entry is malformed.
    #[error("override `{value}` must use the form `capability=module`")]
    MalformedOverride {
        /// Raw override entry.
        value: String,
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

/// Validate a module set and application overrides.
pub fn check(modules: &[ModuleInfo], overrides: &[String]) -> Result<(), BuildError> {
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

    let mut override_map: BTreeMap<String, String> = BTreeMap::new();
    for value in overrides {
        let Some((capability, module)) = value.split_once('=') else {
            return Err(BuildError::MalformedOverride {
                value: value.clone(),
            });
        };
        override_map.insert(capability.trim().to_owned(), module.trim().to_owned());
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

    for (capability, chosen) in &override_map {
        if !provided_keys.contains(capability.as_str()) {
            return Err(BuildError::UnknownOverride {
                capability: capability.clone(),
            });
        }
        let Some(providers) = singleton_providers.get(capability.as_str()) else {
            return Err(BuildError::NotOverridable {
                capability: capability.clone(),
            });
        };
        if !providers.iter().any(|module| &module.key == chosen) {
            return Err(BuildError::InvalidOverride {
                capability: capability.clone(),
                chosen: chosen.clone(),
                providers: provider_names(providers),
            });
        }
    }

    for (capability, providers) in &singleton_providers {
        if providers.len() > 1
            && override_map
                .get(*capability)
                .is_none_or(|chosen| !providers.iter().any(|module| &module.key == chosen))
        {
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
