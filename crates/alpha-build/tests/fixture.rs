use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, contents).expect("write");
}

fn module_manifest(name: &str, key: &str) -> String {
    format!(
        "[package]\n\
         name = \"{name}\"\n\
         version = \"0.0.0\"\n\
         edition = \"2021\"\n\
         \n\
         [package.metadata.alpha]\n\
         api = 1\n\
         kind = \"module\"\n\
         key = \"{key}\"\n\
         provides = [{{ key = \"store\", kind = \"singleton\", scope = \"global\" }}]\n"
    )
}

fn build_fixture(overrides: &[&str]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let alpha_build = Path::new(env!("CARGO_MANIFEST_DIR"));

    write(
        &root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"app\", \"module-a\", \"module-b\"]\nresolver = \"2\"\n",
    );
    for (name, key) in [("module-a", "module-a"), ("module-b", "module-b")] {
        write(
            &root.join(name).join("Cargo.toml"),
            &module_manifest(name, key),
        );
        write(
            &root.join(name).join("src/lib.rs"),
            &format!("pub const __ALPHA_MODULE__: &str = \"{key}\";\n"),
        );
    }

    let overrides_toml = if overrides.is_empty() {
        String::new()
    } else {
        format!(
            "overrides = [{}]\n",
            overrides
                .iter()
                .map(|value| format!("\"{value}\""))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    write(
        &root.join("app/Cargo.toml"),
        &format!(
            "[package]\n\
             name = \"fixture-app\"\n\
             version = \"0.0.0\"\n\
             edition = \"2021\"\n\
             \n\
             [dependencies]\n\
             module-a = {{ path = \"../module-a\" }}\n\
             module-b = {{ path = \"../module-b\" }}\n\
             \n\
             [build-dependencies]\n\
             alpha-build = {{ path = \"{}\" }}\n\
             \n\
             [package.metadata.alpha]\n\
             {overrides_toml}",
            alpha_build.display()
        ),
    );
    write(
        &root.join("app/build.rs"),
        "fn main() {\n    if let Err(error) = alpha_build::check_and_generate() {\n        eprintln!(\"composition error: {error}\");\n        std::process::exit(1);\n    }\n}\n",
    );
    write(&root.join("app/src/main.rs"), "fn main() {}\n");

    (dir, root)
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, directory: &Path, entries: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(directory).expect("read_dir") {
            let entry = entry.expect("entry");
            let path = entry.path();
            let name = entry.file_name();
            if name == "target" || name == "Cargo.lock" {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, entries);
            } else {
                let relative = path.strip_prefix(root).expect("relative").to_path_buf();
                entries.insert(relative, fs::read(&path).expect("read"));
            }
        }
    }
    let mut entries = BTreeMap::new();
    walk(root, root, &mut entries);
    entries
}

fn cargo_check(root: &Path) -> Output {
    Command::new(env!("CARGO"))
        .arg("check")
        .arg("--offline")
        .arg("--manifest-path")
        .arg(root.join("app/Cargo.toml"))
        .env("CARGO_TARGET_DIR", root.join("target"))
        .output()
        .expect("cargo check")
}

#[test]
fn conflicting_singletons_fail_cargo_check_without_touching_sources() {
    let (_dir, root) = build_fixture(&[]);
    let before = snapshot(&root);

    let output = cargo_check(&root);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "expected cargo check to fail\nstdout:\n{}\nstderr:\n{stderr}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        stderr.contains("singleton capability `store`"),
        "missing diagnostic:\n{stderr}"
    );
    assert_eq!(snapshot(&root), before, "build must not modify sources");
}

#[test]
fn explicit_override_resolves_the_conflict_and_builds() {
    let (_dir, root) = build_fixture(&["store=module-b"]);
    let before = snapshot(&root);

    let output = cargo_check(&root);
    assert!(
        output.status.success(),
        "expected cargo check to succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(snapshot(&root), before, "build must not modify sources");
}
