//! Contracts between the fixed shell and a composed harness.
//!
//! The shell parses arguments, serves the core protocol, and dispatches front
//! commands. It never names a module or a frontend crate: everything it needs
//! arrives through the tables in [`Host`], which are generated at build time
//! from the composition's Cargo dependency graph.

#![forbid(unsafe_code)]

use std::fmt;
use std::path::PathBuf;

use alpha_kernel::{ActivateError, Ctx};
use alpha_protocol::Router;
use alpha_schema::{PermissionMode, SessionId};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Scope a module activates at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleScope {
    /// One activation for the whole core.
    Global,
    /// One activation per workspace Location.
    Location,
    /// One activation per session actor.
    Agent,
}

/// Configuration handed to one module activation.
///
/// Values the shell knows before any module runs (`root`, `cwd`, `mode`,
/// `provider`, `model`, `path`) are merged into the same JSON document. A
/// module config deserializes the fields it declares and ignores the rest.
#[derive(Clone, Debug, Default)]
pub struct ModuleInput {
    values: Value,
}

impl ModuleInput {
    /// Build an input from a resolved JSON document.
    pub fn new(values: Value) -> Self {
        Self { values }
    }

    /// Deserialize this module's config type.
    pub fn config<T: DeserializeOwned>(&self, key: &'static str) -> Result<T, ActivateError> {
        serde_json::from_value(self.values.clone())
            .map_err(|error| ActivateError::message(key, error.to_string()))
    }

    /// Whether every listed key is present and not empty.
    pub fn provides_keys(&self, keys: &[&str]) -> bool {
        keys.iter().all(|key| {
            self.values
                .get(*key)
                .is_some_and(|value| !value.is_null() && value.as_str() != Some(""))
        })
    }

    /// The raw document, for modules that merge their own values.
    pub fn values(&self) -> &Value {
        &self.values
    }
}

/// One module activation entry generated from the composition.
pub struct ModuleEntry {
    /// Stable module key, for example `fs.local`.
    pub key: &'static str,
    /// Scope this module activates at.
    pub scope: ModuleScope,
    /// Config keys that must be present before the module activates.
    pub requires_config: &'static [&'static str],
    /// Activate the module against a context.
    pub activate: fn(&Ctx, &ModuleInput) -> Result<(), ActivateError>,
}

/// Arguments one front command receives from the shell.
#[derive(Clone, Debug, Default)]
pub struct FrontRequest {
    /// Workspace root. The shell resolves the current directory when unset.
    pub workspace: PathBuf,
    /// Permission mode.
    pub mode: PermissionMode,
    /// Provider instance or provider kind.
    pub provider: String,
    /// Model identifier.
    pub model: String,
    /// Session to resume.
    pub resume: Option<SessionId>,
    /// Prompt text; empty for interactive fronts.
    pub prompt: Vec<String>,
}

impl FrontRequest {
    /// Joined prompt text.
    pub fn prompt_text(&self) -> String {
        self.prompt.join(" ")
    }
}

/// Failure reported by a front.
#[derive(Clone, Debug)]
pub struct FrontError {
    message: String,
}

impl FrontError {
    /// Build a front error from a message.
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for FrontError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for FrontError {}

/// One command contributed by a frontend crate.
pub struct FrontEntry {
    /// Subcommand name, for example `tui`.
    pub command: &'static str,
    /// One-line help text.
    pub about: &'static str,
    /// Whether the command takes the standard option set and a prompt.
    pub takes_prompt: bool,
    /// Run the front.
    pub run: fn(FrontRequest) -> Result<(), FrontError>,
}

/// RPC methods contributed by a module.
///
/// The shell builds the base router (ping, session log methods) and asks every
/// registered extension to add its own methods, so a protocol surface belongs
/// to the module that owns it.
pub trait RpcExtension: Send + Sync {
    /// Register this extension's methods.
    fn extend(&self, router: Router) -> Router;
}

/// The composed harness the shell runs.
pub struct Host {
    /// Module activations, already sorted by dependency.
    pub modules: &'static [ModuleEntry],
    /// Front commands.
    pub fronts: &'static [FrontEntry],
}

impl Host {
    /// Modules activating at one scope, in dependency order.
    pub fn modules_at(&self, scope: ModuleScope) -> impl Iterator<Item = &'static ModuleEntry> {
        self.modules
            .iter()
            .filter(move |module| module.scope == scope)
    }
}

/// Values the shell injects into every module input.
#[derive(Clone, Debug, Default)]
pub struct RuntimeValues {
    /// Workspace root for the Location being activated.
    pub workspace: Option<PathBuf>,
    /// Fixed working directory for the Location being activated.
    pub cwd: Option<PathBuf>,
    /// Permission mode for the session being activated.
    pub mode: Option<PermissionMode>,
    /// Provider route for the session being activated.
    pub provider: Option<String>,
    /// Model route for the session being activated.
    pub model: Option<String>,
    /// Provider configuration path override.
    pub provider_config_path: Option<PathBuf>,
}

impl RuntimeValues {
    /// Render the injection document shared by every module.
    pub fn module_input(&self) -> ModuleInput {
        let mut document = serde_json::Map::new();
        if let Some(workspace) = &self.workspace {
            document.insert("root".to_owned(), path_value(workspace));
        }
        if let Some(cwd) = &self.cwd {
            document.insert("cwd".to_owned(), path_value(cwd));
        }
        if let Some(mode) = self.mode {
            document.insert("mode".to_owned(), Value::String(mode.to_string()));
        }
        if let Some(provider) = &self.provider {
            document.insert("provider".to_owned(), Value::String(provider.clone()));
        }
        if let Some(model) = &self.model {
            document.insert("model".to_owned(), Value::String(model.clone()));
        }
        if let Some(path) = &self.provider_config_path {
            document.insert("path".to_owned(), path_value(path));
        }
        ModuleInput::new(Value::Object(document))
    }
}

fn path_value(path: &std::path::Path) -> Value {
    Value::String(path.to_string_lossy().into_owned())
}
