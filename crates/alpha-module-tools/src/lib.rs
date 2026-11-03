//! Built-in tools: read, grep, write, edit, and shell.

#![forbid(unsafe_code)]

pub mod contract;
mod execution;
pub mod file_tools;
pub mod grep_tool;
pub mod image_tool;
pub mod registry;
pub mod shell_tool;

use std::sync::Arc;

use alpha_kernel::{ActivateError, CapabilityScope, CapabilitySpec, Ctx, Module};
use alpha_module_fs::{FileSystem, FileSystemSet};
use alpha_module_process::ProcessRunnerSet;
use alpha_permission_api::{PermissionService, UnrestrictedPermission};
use alpha_prompt_api::{PromptError, PromptPriority, PromptRequest, PromptSectionProvider};
use serde::Deserialize;

/// Stable module key used by the generated composition table.
pub const __ALPHA_MODULE__: &str = "tools";

pub use contract::{Tool, ToolContext, ToolError, ToolResponse};
pub use file_tools::{EditTool, ReadTool, WriteTool};
pub use grep_tool::GrepTool;
pub use image_tool::ViewImageTool;
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
            .get::<dyn PermissionService>()
            .unwrap_or_else(|| Arc::new(UnrestrictedPermission));
        let filesystem: Arc<dyn FileSystem> = Arc::new(execution::ModeAwareFileSystem::new(
            filesystems,
            permission.clone(),
        ));

        let registry = Arc::new(ToolRegistry::new());
        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(ReadTool::new(filesystem.clone(), permission.clone())),
            Arc::new(WriteTool::new(filesystem.clone(), permission.clone())),
            Arc::new(EditTool::new(filesystem.clone(), permission.clone())),
            Arc::new(GrepTool::new(filesystem.clone(), permission.clone())),
            Arc::new(ViewImageTool::new(filesystem, permission.clone())),
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
        // Tool-use policy belongs to the crate that owns the tool surface.
        alpha_prompt_api::section_registry(ctx)
            .and_then(|registry| registry.register(ctx, Arc::new(ToolRulesSection)))
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        Ok(())
    }
}

/// Tool-use policy.
pub struct ToolRulesSection;

impl PromptSectionProvider for ToolRulesSection {
    fn key(&self) -> &'static str {
        "tool-rules"
    }

    fn priority(&self) -> PromptPriority {
        PromptPriority::Tools
    }

    fn render(&self, _request: &PromptRequest<'_>) -> Result<Option<String>, PromptError> {
        Ok(Some(
            "# Tool Use\n\n\
             - The tool schema attached to the current request is the single source of truth for tool names, parameters, and capabilities.\n\
             - Independent reads, searches, and inspections should be issued in parallel where possible; operations with dependencies must run in order.\n\
             - When a tool fails, read the error and adjust your approach; do not repeat the same failing call verbatim.\n\
             - After a change, verify the result with the check closest to the goal rather than repeatedly reading the whole project."
                .to_owned(),
        ))
    }
}
