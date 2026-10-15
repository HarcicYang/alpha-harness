use std::path::Path;

use alpha_kernel::{Ctx, Module};
use alpha_module_fs::{FileKind, FileSystem, FsConfig, FsError, FsLocalModule, LocalFileSystem};

#[test]
fn reads_writes_and_lists_inside_the_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    let fs = LocalFileSystem::new(dir.path());
    std::fs::create_dir(dir.path().join("notes")).expect("create notes");

    fs.write(Path::new("notes/hello.txt"), b"hello")
        .expect("write");
    assert_eq!(
        fs.read(Path::new("notes/hello.txt")).expect("read"),
        b"hello"
    );

    let stat = fs.stat(Path::new("notes/hello.txt")).expect("stat");
    assert_eq!(stat.kind, FileKind::File);
    assert_eq!(stat.size, 5);
    assert!(stat.modified_ms.is_some());

    let entries = fs.list(Path::new("notes")).expect("list");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, Path::new("notes/hello.txt"));
}

#[test]
fn atomic_writes_replace_content_and_leave_no_temp_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let fs = LocalFileSystem::new(dir.path());

    fs.write(Path::new("file.txt"), b"first").expect("write");
    fs.write(Path::new("file.txt"), b"second").expect("rewrite");
    assert_eq!(fs.read(Path::new("file.txt")).expect("read"), b"second");

    let entries = fs.list(Path::new(".")).expect("list");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, Path::new("file.txt"));
}

#[test]
fn escaping_paths_are_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let fs = LocalFileSystem::new(dir.path());

    let error = fs.read(Path::new("../outside.txt")).expect_err("escape");
    assert!(matches!(error, FsError::OutsideWorkspace(_)));

    let error = fs
        .read(Path::new("/etc/hostname"))
        .expect_err("absolute escape");
    assert!(matches!(error, FsError::OutsideWorkspace(_)));
}

#[test]
fn full_access_allows_paths_outside_the_working_directory() {
    let cwd = tempfile::tempdir().expect("cwd");
    let outside = tempfile::tempdir().expect("outside");
    let fs = LocalFileSystem::new_full_access(cwd.path());
    let outside_file = outside.path().join("outside.txt");

    fs.write(Path::new("relative.txt"), b"relative")
        .expect("relative write");
    fs.write(&outside_file, b"outside").expect("outside write");
    assert_eq!(
        fs.read(Path::new("relative.txt")).expect("relative read"),
        b"relative"
    );
    assert_eq!(fs.read(&outside_file).expect("outside read"), b"outside");
    assert!(fs.root().is_none());
}

#[test]
fn module_provides_the_seam() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ctx = Ctx::root();
    FsLocalModule
        .activate(
            &ctx,
            &FsConfig {
                root: dir.path().to_path_buf(),
                cwd: dir.path().to_path_buf(),
            },
        )
        .expect("activate");

    let fs = ctx.require::<dyn FileSystem>().expect("seam");
    assert!(
        ctx.get::<alpha_module_fs::FileSystemSet>().is_some(),
        "filesystem set"
    );
    fs.write(Path::new("file.txt"), b"content").expect("write");
    assert_eq!(fs.read(Path::new("file.txt")).expect("read"), b"content");

    ctx.dispose();
    assert!(ctx.get::<dyn FileSystem>().is_none());
}
