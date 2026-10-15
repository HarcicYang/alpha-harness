//! Mode-aware execution adapters used by built-in tools.

use std::path::Path;
use std::sync::Arc;

use alpha_module_fs::{DirEntry, FileStat, FileSystem, FileSystemAccess, FileSystemSet, FsError};
use alpha_module_permission::PermissionService;
use alpha_schema::PermissionMode;

pub(crate) struct ModeAwareFileSystem {
    filesystems: Arc<FileSystemSet>,
    permission: Arc<dyn PermissionService>,
}

impl ModeAwareFileSystem {
    pub(crate) fn new(
        filesystems: Arc<FileSystemSet>,
        permission: Arc<dyn PermissionService>,
    ) -> Self {
        Self {
            filesystems,
            permission,
        }
    }

    fn current(&self) -> Arc<dyn FileSystem> {
        let access = if self.permission.mode() == PermissionMode::Free {
            FileSystemAccess::FullAccess
        } else {
            FileSystemAccess::Workspace
        };
        self.filesystems.get(access)
    }
}

impl FileSystem for ModeAwareFileSystem {
    fn read(&self, path: &Path) -> Result<Vec<u8>, FsError> {
        self.current().read(path)
    }

    fn write(&self, path: &Path, data: &[u8]) -> Result<(), FsError> {
        self.current().write(path, data)
    }

    fn list(&self, path: &Path) -> Result<Vec<DirEntry>, FsError> {
        self.current().list(path)
    }

    fn stat(&self, path: &Path) -> Result<FileStat, FsError> {
        self.current().stat(path)
    }
}
