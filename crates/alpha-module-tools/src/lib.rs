//! Built-in tools: read, grep, write, edit, and shell.

#![forbid(unsafe_code)]

pub mod contract;
mod execution;
pub mod file_tools;
pub mod grep_tool;
pub mod registry;
pub mod shell_tool;

use std::sync::Arc;

use alpha_kernel::{ActivateError, CapabilityScope, CapabilitySpec, Ctx, Module};
use alpha_module_fs::{FileSystem, FileSystemSet};
use alpha_module_permission::PermissionService;
use alpha_module_process::ProcessRunnerSet;
use serde::Deserialize;

/// Stable module key used by the generated composition table.
pub const __ALPHA_MODULE__: &str = "tools";

pub use contract::{Tool, ToolContext, ToolError, ToolResponse};
pub use file_tools::{EditTool, ReadTool, WriteTool};
pub use grep_tool::GrepTool;
pub use registry::{RegistryError, ToolRegistry};
pub use shell_tool::{ShellKind, ShellTool, discover_shells};

/// Configuration for the built-in tools.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ToolsConfig {
    /// Default shell used when a call does not choose one.
    #[serde(default)]
    pub default_shell: Option<String>,
}

/// Registers the built-in tool set.
pub struct ToolsModule;

impl Module for ToolsModule {
    type Requirements = ();
    type Config = ToolsConfig;

    const KEY: &'static str = "tools";
    const CAPABILITIES: &'static [CapabilitySpec] =
        &[CapabilitySpec::registry("tool", CapabilityScope::Location)];

    fn activate(&self, ctx: &Ctx, config: &Self::Config) -> Result<(), ActivateError> {
        let filesystems = ctx
            .require::<FileSystemSet>()
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        let processes = ctx
            .require::<ProcessRunnerSet>()
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        let permission = ctx
            .require::<dyn PermissionService>()
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        let filesystem: Arc<dyn FileSystem> = Arc::new(execution::ModeAwareFileSystem::new(
            filesystems,
            permission.clone(),
        ));

        let registry = Arc::new(ToolRegistry::new());
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(ReadTool::new(filesystem.clone(), permission.clone())),
            Arc::new(WriteTool::new(filesystem.clone(), permission.clone())),
            Arc::new(EditTool::new(filesystem.clone(), permission.clone())),
            Arc::new(GrepTool::new(filesystem, permission.clone())),
            Arc::new(ShellTool::new(
                processes,
                permission,
                config.default_shell.clone(),
            )),
        ];
        for tool in tools {
            registry
                .register(tool)
                .map_err(|error| ActivateError::new(Self::KEY, error))?;
        }
        ctx.provide::<ToolRegistry>(registry)
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        Ok(())
    }
}
