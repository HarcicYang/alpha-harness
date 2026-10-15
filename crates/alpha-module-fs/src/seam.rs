//! Filesystem seam types.

use std::path::PathBuf;

/// Kind of a filesystem entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    /// Regular file.
    File,
    /// Directory.
    Directory,
    /// Symlink or other special entry.
    Other,
}

/// One directory entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    /// Workspace-relative path, or an absolute path in full-access mode.
    pub path: PathBuf,
    /// Entry kind.
    pub kind: FileKind,
}

/// Metadata for one entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStat {
    /// Entry kind.
    pub kind: FileKind,
    /// Size in bytes.
    pub size: u64,
    /// Modification time in milliseconds since the Unix epoch.
    pub modified_ms: Option<i64>,
}

/// Filesystem errors.
#[derive(Debug, thiserror::Error)]
pub enum FsError {
    /// Underlying IO error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The path escapes the workspace root.
    #[error("path `{0}` escapes the workspace root")]
    OutsideWorkspace(PathBuf),
}

/// Filesystem contract.
///
/// Implementations are rooted at one workspace. Paths are relative to that
/// root; escaping paths fail with [`FsError::OutsideWorkspace`].
pub trait FileSystem: Send + Sync + 'static {
    /// Read a file.
    fn read(&self, path: &std::path::Path) -> Result<Vec<u8>, FsError>;
    /// Write a file atomically.
    fn write(&self, path: &std::path::Path, data: &[u8]) -> Result<(), FsError>;
    /// List a directory.
    fn list(&self, path: &std::path::Path) -> Result<Vec<DirEntry>, FsError>;
    /// Stat an entry.
    fn stat(&self, path: &std::path::Path) -> Result<FileStat, FsError>;
}
