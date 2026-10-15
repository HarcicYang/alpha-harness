//! Local disk driver for the filesystem seam.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::seam::{DirEntry, FileKind, FileStat, FileSystem, FsError};

/// Filesystem seam backed by the local disk.
#[derive(Clone, Debug)]
pub struct LocalFileSystem {
    root: Option<PathBuf>,
    cwd: PathBuf,
}

impl LocalFileSystem {
    /// Create a driver rooted at a workspace path.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = normalize(&root.into());
        Self {
            cwd: root.clone(),
            root: Some(root),
        }
    }

    /// Create a driver with full local filesystem access.
    pub fn new_full_access(cwd: impl Into<PathBuf>) -> Self {
        Self {
            root: None,
            cwd: normalize(&cwd.into()),
        }
    }

    /// The workspace root.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// Resolve a path for this driver.
    ///
    /// Rooted drivers reject paths outside their workspace. Full-access drivers
    /// resolve relative paths from the configured working directory.
    pub fn resolve(&self, path: &Path) -> Result<PathBuf, FsError> {
        let joined = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.cwd.join(path)
        };
        let normalized = normalize(&joined);
        if let Some(root) = &self.root
            && !normalized.starts_with(root)
        {
            return Err(FsError::OutsideWorkspace(path.to_path_buf()));
        }
        Ok(normalized)
    }
}

impl FileSystem for LocalFileSystem {
    fn read(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        let path = self.resolve(path)?;
        Ok(fs::read(path)?)
    }

    fn write(&self, path: &Path, data: &[u8]) -> Result<(), FsError> {
        let path = self.resolve(path)?;
        let parent = path
            .parent()
            .ok_or_else(|| FsError::OutsideWorkspace(path.clone()))?;
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".to_owned());
        let temp = parent.join(format!(".{file_name}.tmp-{}", std::process::id()));
        fs::write(&temp, data)?;
        if let Err(error) = fs::rename(&temp, &path) {
            let _ = fs::remove_file(&temp);
            return Err(FsError::Io(error));
        }
        Ok(())
    }

    fn list(&self, path: &Path) -> Result<Vec<DirEntry>, FsError> {
        let path = self.resolve(path)?;
        let mut entries = Vec::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let entry_path = entry.path();
            let relative = self
                .root
                .as_ref()
                .and_then(|root| entry_path.strip_prefix(root).ok())
                .unwrap_or(&entry_path)
                .to_path_buf();
            let kind = entry.file_type()?.into();
            entries.push(DirEntry {
                path: relative,
                kind,
            });
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(entries)
    }

    fn stat(&self, path: &Path) -> Result<FileStat, FsError> {
        let path = self.resolve(path)?;
        let metadata = fs::metadata(path)?;
        let modified_ms = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as i64);
        Ok(FileStat {
            kind: metadata.file_type().into(),
            size: metadata.len(),
            modified_ms,
        })
    }
}

impl From<fs::FileType> for FileKind {
    fn from(file_type: fs::FileType) -> Self {
        if file_type.is_file() {
            Self::File
        } else if file_type.is_dir() {
            Self::Directory
        } else {
            Self::Other
        }
    }
}

/// Lexically normalize a path, dropping `.` and folding `..`.
fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

/// Wall-clock timestamp helper for tests and diagnostics.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}
