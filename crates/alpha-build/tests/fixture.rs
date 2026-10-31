use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BODY_A: &str = "api = 1\n\
kind = \"module\"\n\
key = \"module-a\"\n\
scope = \"global\"\n\
module = \"ModuleA\"\n\
config = \"ConfigA\"\n\
provides = [{ key = \"store\", kind = \"singleton\", scope = \"global\" }]";

const BODY_B: &str = "api = 1\n\
kind = \"module\"\n\
key = \"module-b\"\n\
scope = \"global\"\n\
module = \"ModuleB\"\n\
config = \"ConfigB\"\n\
provides = [{ key = \"store\", kind = \"singleton\", scope = \"global\" }]";

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, contents).expect("write");
}

fn module_manifest(name: &str, metadata: &str) -> String {
    format!(
        "[package]\n\
         name = \"{name}\"\n\
         version = \"0.0.0\"\n\
         edition = \"2021\"\n\
         \n\
         [package.metadata.alpha]\n\
         {metadata}\n"
    )
}

fn build_fixture_with(body_a: &str, body_b: Option<&str>) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let alpha_build = Path::new(env!("CARGO_MANIFEST_DIR"));

    let members = match body_b {
        Some(_) => {
            "[workspace]\nmembers = [\"app\", \"module-a\", \"module-b\"]\nresolver = \"2\"\n"
                .to_owned()
        }
        None => "[workspace]\nmembers = [\"app\", \"module-a\"]\nresolver = \"2\"\n".to_owned(),
    };
    write(&root.join("Cargo.toml"), &members);
    write(
        &root.join("module-a/Cargo.toml"),
        &module_manifest("module-a", body_a),
    );
    write(
        &root.join("module-a/src/lib.rs"),
        "pub const __ALPHA_MODULE__: &str = \"module-a\";\n",
    );
    if let Some(body_b) = body_b {
        write(
            &root.join("module-b/Cargo.toml"),
            &module_manifest("module-b", body_b),
        );
        write(
            &root.join("module-b/src/lib.rs"),
            "pub const __ALPHA_MODULE__: &str = \"module-b\";\n",
        );
    }

    let module_b_dep = if body_b.is_some() {
        "module-b = { path = \"../module-b\" }\n"
    } else {
        ""
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
             {module_b_dep}\
             \n\
             [build-dependencies]\n\
             alpha-build = {{ path = \"{}\" }}\n\
             ",
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

fn build_fixture() -> (tempfile::TempDir, PathBuf) {
    build_fixture_with(BODY_A, Some(BODY_B))
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

fn generated_table(root: &Path) -> String {
    let build_dir = root.join("target/debug/build");
    for entry in fs::read_dir(&build_dir).expect("build dir") {
        let entry = entry.expect("entry");
        if !entry.file_type().expect("file type").is_dir() {
            continue;
        }
        let candidate = entry.path().join("out/alpha_modules.rs");
        if candidate.exists() {
            return fs::read_to_string(candidate).expect("generated table");
        }
    }
    panic!("generated module table not found");
}

#[test]
fn conflicting_singletons_fail_cargo_check_without_touching_sources() {
    let (_dir, root) = build_fixture();
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
fn missing_requirements_fail_cargo_check() {
    let body_a = "api = 1\n\
kind = \"module\"\n\
key = \"module-a\"\n\
scope = \"global\"\n\
module = \"ModuleA\"\n\
config = \"ConfigA\"\n\
provides = [{ key = \"store\", kind = \"singleton\", scope = \"global\" }]\n\
requires = [\"fs\"]";
    let body_b = "api = 1\n\
kind = \"module\"\n\
key = \"module-b\"\n\
scope = \"global\"\n\
module = \"ModuleB\"\n\
config = \"ConfigB\"\n\
provides = [{ key = \"other\", kind = \"singleton\", scope = \"global\" }]";
    let (_dir, root) = build_fixture_with(body_a, Some(body_b));

    let output = cargo_check(&root);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains("requires `fs`"),
        "missing diagnostic:\n{stderr}"
    );
}

#[test]
fn registry_collisions_fail_cargo_check() {
    let body_a = "api = 1\n\
kind = \"module\"\n\
key = \"module-a\"\n\
scope = \"global\"\n\
module = \"ModuleA\"\n\
config = \"ConfigA\"\n\
provides = [{ key = \"store\", kind = \"singleton\", scope = \"global\" }]\n\
registry_entries = [\"read\"]";
    let body_b = "api = 1\n\
kind = \"module\"\n\
key = \"module-b\"\n\
scope = \"global\"\n\
module = \"ModuleB\"\n\
config = \"ConfigB\"\n\
provides = [{ key = \"other\", kind = \"singleton\", scope = \"global\" }]\n\
registry_entries = [\"read\"]";
    let (_dir, root) = build_fixture_with(body_a, Some(body_b));

    let output = cargo_check(&root);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains("registry entry `read`"),
        "missing diagnostic:\n{stderr}"
    );
}

#[test]
fn module_set_follows_the_dependency_graph() {
    let body_b_single = "api = 1\n\
kind = \"module\"\n\
key = \"module-b\"\n\
scope = \"global\"\n\
module = \"ModuleB\"\n\
config = \"ConfigB\"\n\
provides = [{ key = \"store\", kind = \"singleton\", scope = \"global\" }]";

    let (_dir_a, root_a) = build_fixture_with(BODY_A, None);
    assert!(cargo_check(&root_a).status.success());
    let table_a = generated_table(&root_a);
    assert!(table_a.contains("module-a"));

    let (_dir_b, root_b) = build_fixture_with(body_b_single, None);
    assert!(cargo_check(&root_b).status.success());
    let table_b = generated_table(&root_b);
    assert!(table_b.contains("module-b"));
}
