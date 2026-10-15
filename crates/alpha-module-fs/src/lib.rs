//! Filesystem seam and local driver.
//!
//! The driver is rooted at a workspace path: relative paths resolve inside the
//! root and paths that escape it are rejected.

#![forbid(unsafe_code)]

/// Stable module key used by the generated composition table.
pub const __ALPHA_MODULE__: &str = "fs.local";

pub mod local;
pub mod seam;

use std::path::PathBuf;
use std::sync::Arc;

use alpha_kernel::{ActivateError, CapabilityScope, CapabilitySpec, Ctx, Module};
use serde::Deserialize;

pub use local::LocalFileSystem;
pub use seam::{DirEntry, FileKind, FileStat, FileSystem, FsError};

/// One filesystem execution boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileSystemAccess {
    /// Access is rooted at the session workspace.
    Workspace,
    /// Access uses the local user's full filesystem permissions.
    FullAccess,
}

/// Filesystem drivers available to a Location.
#[derive(Clone)]
pub struct FileSystemSet {
    workspace: Arc<dyn FileSystem>,
    full_access: Arc<dyn FileSystem>,
}

impl FileSystemSet {
    /// Build a filesystem set.
    pub fn new(workspace: Arc<dyn FileSystem>, full_access: Arc<dyn FileSystem>) -> Self {
        Self {
            workspace,
            full_access,
        }
    }

    /// Resolve one filesystem boundary.
    pub fn get(&self, access: FileSystemAccess) -> Arc<dyn FileSystem> {
        match access {
            FileSystemAccess::Workspace => self.workspace.clone(),
            FileSystemAccess::FullAccess => self.full_access.clone(),
        }
    }
}

/// Configuration for the local filesystem driver.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct FsConfig {
    /// Workspace root. Must be an absolute path at activation time.
    pub root: PathBuf,
    /// Working directory used by full-access relative paths.
    #[serde(default)]
    pub cwd: PathBuf,
}

/// Provides the filesystem seam backed by the local disk.
pub struct FsLocalModule;

impl Module for FsLocalModule {
    type Requirements = ();
    type Config = FsConfig;

    const KEY: &'static str = "fs.local";
    const CAPABILITIES: &'static [CapabilitySpec] =
        &[CapabilitySpec::singleton("fs", CapabilityScope::Location)];

    fn activate(&self, ctx: &Ctx, config: &Self::Config) -> Result<(), ActivateError> {
        if config.root.as_os_str().is_empty() {
            return Err(ActivateError::message(
                Self::KEY,
                "fs root is not configured",
            ));
        }
        let cwd = if config.cwd.as_os_str().is_empty() {
            config.root.clone()
        } else {
            config.cwd.clone()
        };
        let workspace: Arc<dyn FileSystem> = Arc::new(LocalFileSystem::new(config.root.clone()));
        let full_access: Arc<dyn FileSystem> = Arc::new(LocalFileSystem::new_full_access(cwd));
        ctx.provide::<FileSystemSet>(Arc::new(FileSystemSet::new(workspace.clone(), full_access)))
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        ctx.provide::<dyn FileSystem>(workspace)
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        Ok(())
    }
}
