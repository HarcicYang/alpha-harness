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

/// Configuration for the local filesystem driver.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct FsConfig {
    /// Workspace root. Must be an absolute path at activation time.
    pub root: PathBuf,
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
        let filesystem: Arc<dyn FileSystem> = Arc::new(LocalFileSystem::new(config.root.clone()));
        ctx.provide::<dyn FileSystem>(filesystem)
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        Ok(())
    }
}
