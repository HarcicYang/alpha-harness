//! User TOML persistence for provider instances.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use alpha_kernel::{ActivateError, CapabilityScope, CapabilitySpec, Ctx, Module};
use alpha_provider_api::{ProviderInstance, ProviderInstanceRegistry};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Stable module key.
pub const __ALPHA_MODULE__: &str = "provider.config";

/// Provider configuration persistence failures.
#[derive(Debug, Error)]
pub enum ProviderConfigError {
    /// Config file IO failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Config TOML could not be parsed or serialized.
    #[error("provider config TOML error: {0}")]
    Toml(String),
    /// Provider instance conversion failed.
    #[error("provider instance conversion failed: {0}")]
    Instance(String),
}

/// File IO seam for the user configuration file.
pub trait ConfigFile: Send + Sync + 'static {
    /// Read a file if it exists.
    fn read(&self, path: &Path) -> Result<Option<String>, ProviderConfigError>;
    /// Atomically write a file.
    fn write_atomic(&self, path: &Path, contents: &str) -> Result<(), ProviderConfigError>;
}

/// Local filesystem implementation for user configuration.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalConfigFile;

impl ConfigFile for LocalConfigFile {
    fn read(&self, path: &Path) -> Result<Option<String>, ProviderConfigError> {
        match std::fs::read_to_string(path) {
            Ok(contents) => Ok(Some(contents)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn write_atomic(&self, path: &Path, contents: &str) -> Result<(), ProviderConfigError> {
        let parent = path.parent().ok_or_else(|| {
            ProviderConfigError::Instance(format!("config path `{}` has no parent", path.display()))
        })?;
        std::fs::create_dir_all(parent)?;
        let temp = parent.join(format!(
            ".{}.tmp-{}",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("config.toml"),
            std::process::id()
        ));
        std::fs::write(&temp, contents)?;
        std::fs::rename(&temp, path)?;
        Ok(())
    }
}

/// Provider config persistence contract.
pub trait ProviderConfigStore: Send + Sync + 'static {
    /// Reload the file into the runtime registry.
    fn reload(&self) -> Result<(), ProviderConfigError>;
    /// List all persisted instances.
    fn list(&self) -> Vec<ProviderInstance>;
    /// Get one persisted instance.
    fn get(&self, id: &str) -> Option<ProviderInstance>;
    /// Create or replace one instance.
    fn upsert(&self, instance: ProviderInstance) -> Result<(), ProviderConfigError>;
    /// Remove one instance.
    fn remove(&self, id: &str) -> Result<bool, ProviderConfigError>;
    /// The default provider instance id.
    fn default_instance(&self) -> Option<String>;
    /// Set the default provider instance id.
    fn set_default_instance(&self, id: Option<String>) -> Result<(), ProviderConfigError>;
}

/// TOML-backed provider config store.
pub struct TomlProviderConfigStore {
    path: PathBuf,
    file: Arc<dyn ConfigFile>,
    registry: Arc<ProviderInstanceRegistry>,
}

impl TomlProviderConfigStore {
    /// Create a store over one file and runtime registry.
    pub fn new(
        path: PathBuf,
        file: Arc<dyn ConfigFile>,
        registry: Arc<ProviderInstanceRegistry>,
    ) -> Self {
        Self {
            path,
            file,
            registry,
        }
    }

    /// The backing config path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read_file(&self) -> Result<ProviderFile, ProviderConfigError> {
        let Some(contents) = self.file.read(&self.path)? else {
            return Ok(ProviderFile::default());
        };
        toml::from_str(&contents).map_err(|error| ProviderConfigError::Toml(error.to_string()))
    }

    fn write_file(&self, file: &ProviderFile) -> Result<(), ProviderConfigError> {
        let contents = toml::to_string_pretty(file)
            .map_err(|error| ProviderConfigError::Toml(error.to_string()))?;
        self.file.write_atomic(&self.path, &contents)
    }
}

impl ProviderConfigStore for TomlProviderConfigStore {
    fn reload(&self) -> Result<(), ProviderConfigError> {
        let file = self.read_file()?;
        for (id, instance) in &file.providers.instances {
            let mut instance = instance.clone().into_provider_instance()?;
            instance.id = id.clone();
            self.registry.upsert(instance);
        }
        Ok(())
    }

    fn list(&self) -> Vec<ProviderInstance> {
        self.registry.list()
    }

    fn get(&self, id: &str) -> Option<ProviderInstance> {
        self.registry.get(id)
    }

    fn upsert(&self, instance: ProviderInstance) -> Result<(), ProviderConfigError> {
        let mut file = self.read_file()?;
        file.providers.instances.insert(
            instance.id.clone(),
            ProviderInstanceToml::from_instance(instance.clone())?,
        );
        self.write_file(&file)?;
        self.registry.upsert(instance);
        Ok(())
    }

    fn remove(&self, id: &str) -> Result<bool, ProviderConfigError> {
        let mut file = self.read_file()?;
        let removed = file.providers.instances.remove(id).is_some();
        if removed {
            self.write_file(&file)?;
            self.registry.remove(id);
        }
        Ok(removed)
    }

    fn default_instance(&self) -> Option<String> {
        self.read_file()
            .ok()
            .and_then(|file| file.providers.default_instance)
    }

    fn set_default_instance(&self, id: Option<String>) -> Result<(), ProviderConfigError> {
        let mut file = self.read_file()?;
        file.providers.default_instance = id;
        self.write_file(&file)
    }
}

/// Provider config module configuration.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ProviderConfigConfig {
    /// Explicit user config path. Defaults to the XDG config location.
    pub path: Option<PathBuf>,
}

/// Registers the provider config store.
pub struct ProviderConfigModule;

impl Module for ProviderConfigModule {
    type Requirements = ();
    type Config = ProviderConfigConfig;

    const KEY: &'static str = "provider.config";
    const CAPABILITIES: &'static [CapabilitySpec] = &[CapabilitySpec::singleton(
        "provider.config_store",
        CapabilityScope::Global,
    )];

    fn activate(&self, ctx: &Ctx, config: &Self::Config) -> Result<(), ActivateError> {
        let registry = ctx
            .require::<ProviderInstanceRegistry>()
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        let file: Arc<dyn ConfigFile> = match ctx.get::<dyn ConfigFile>() {
            Some(file) => file,
            None => Arc::new(LocalConfigFile),
        };
        let path = config.path.clone().unwrap_or_else(default_config_path);
        let store = Arc::new(TomlProviderConfigStore::new(path, file, registry));
        store
            .reload()
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        ctx.provide::<dyn ProviderConfigStore>(store)
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        Ok(())
    }
}

fn default_config_path() -> PathBuf {
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(path)
            .join("alpha-harness")
            .join("config.toml");
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("alpha-harness")
        .join("config.toml")
}

#[derive(Default, Serialize, Deserialize)]
struct ProviderFile {
    #[serde(default)]
    providers: ProvidersSection,
}

#[derive(Default, Serialize, Deserialize)]
struct ProvidersSection {
    #[serde(skip_serializing_if = "Option::is_none")]
    default_instance: Option<String>,
    #[serde(default)]
    instances: BTreeMap<String, ProviderInstanceToml>,
}

#[derive(Clone, Serialize, Deserialize)]
struct ProviderInstanceToml {
    kind: String,
    config_version: u32,
    #[serde(default = "default_toml_table")]
    config: toml::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    auth_ref: Option<String>,
    #[serde(default = "default_toml_table")]
    model_overrides: toml::Value,
    #[serde(default)]
    disabled: bool,
}

fn default_toml_table() -> toml::Value {
    toml::Value::Table(toml::Table::new())
}

impl ProviderInstanceToml {
    fn from_instance(instance: ProviderInstance) -> Result<Self, ProviderConfigError> {
        Ok(Self {
            kind: instance.kind,
            config_version: instance.config_version,
            config: toml::Value::try_from(instance.config)
                .map_err(|error| ProviderConfigError::Instance(error.to_string()))?,
            auth_ref: instance.auth_ref,
            model_overrides: toml::Value::try_from(instance.model_overrides)
                .map_err(|error| ProviderConfigError::Instance(error.to_string()))?,
            disabled: instance.disabled,
        })
    }

    fn into_provider_instance(self) -> Result<ProviderInstance, ProviderConfigError> {
        Ok(ProviderInstance {
            id: String::new(),
            kind: self.kind,
            config_version: self.config_version,
            config: serde_json::to_value(self.config)
                .map_err(|error| ProviderConfigError::Instance(error.to_string()))?,
            auth_ref: self.auth_ref,
            model_overrides: serde_json::to_value(self.model_overrides)
                .map_err(|error| ProviderConfigError::Instance(error.to_string()))?,
            disabled: self.disabled,
        })
    }
}
