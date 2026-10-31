//! The generated table must follow the composition: a front crate in the
//! dependency graph contributes its commands, and removing it removes them.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, contents).expect("write");
}

const MODULE_BODY: &str = "api = 1\n\
kind = \"module\"\n\
key = \"core\"\n\
scope = \"global\"\n\
module = \"Core\"\n\
config = \"Config\"\n\
provides = [{ key = \"store\", kind = \"singleton\", scope = \"global\" }]\n";

const FRONT_BODY: &str = "api = 1\n\
kind = \"front\"\n\
commands = [\n\
  { name = \"tui\", about = \"Open the terminal interface\", entry = \"__front_tui\", takes_prompt = false },\n\
  { name = \"run\", about = \"Run one prompt and exit\", entry = \"__front_run\", takes_prompt = true },\n\
]\n";

fn build_fixture(with_front: bool) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let alpha_build = Path::new(env!("CARGO_MANIFEST_DIR"));

    let members = if with_front {
        "[workspace]\nmembers = [\"app\", \"core\", \"front\"]\nresolver = \"2\"\n"
    } else {
        "[workspace]\nmembers = [\"app\", \"core\"]\nresolver = \"2\"\n"
    };
    write(&root.join("Cargo.toml"), members);

    write(
        &root.join("core/Cargo.toml"),
        &format!(
            "[package]\nname = \"core\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[package.metadata.alpha]\n{MODULE_BODY}"
        ),
    );
    write(
        &root.join("core/src/lib.rs"),
        "pub const __ALPHA_MODULE__: &str = \"core\";\n",
    );

    if with_front {
        write(
            &root.join("front/Cargo.toml"),
            &format!(
                "[package]\nname = \"front\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[package.metadata.alpha]\n{FRONT_BODY}"
            ),
        );
        write(
            &root.join("front/src/lib.rs"),
            "pub fn __front_tui() {}\npub fn __front_run() {}\n",
        );
    }

    let front_dep = if with_front {
        "front = { path = \"../front\" }\n"
    } else {
        ""
    };
    write(
        &root.join("app/Cargo.toml"),
        &format!(
            "[package]\nname = \"fixture-app\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n\
             [dependencies]\ncore = {{ path = \"../core\" }}\n{front_dep}\n\
             [build-dependencies]\nalpha-build = {{ path = \"{}\" }}\n",
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

fn cargo_check(root: &Path) {
    let output = Command::new(env!("CARGO"))
        .arg("check")
        .arg("--offline")
        .arg("--manifest-path")
        .arg(root.join("app/Cargo.toml"))
        .env("CARGO_TARGET_DIR", root.join("target"))
        .output()
        .expect("cargo check");
    assert!(
        output.status.success(),
        "fixture build failed\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn generated(root: &Path) -> String {
    let build_dir = root.join("target/debug/build");
    for entry in fs::read_dir(&build_dir).expect("build dir") {
        let entry = entry.expect("entry");
        if !entry.file_type().expect("file type").is_dir() {
            continue;
        }
        let candidate = entry.path().join("out/alpha_composition.rs");
        if candidate.exists() {
            return fs::read_to_string(candidate).expect("generated composition");
        }
    }
    panic!("generated composition not found");
}

#[test]
fn front_crates_contribute_the_commands_they_declare() {
    let (_dir, root) = build_fixture(true);
    cargo_check(&root);
    let table = generated(&root);
    assert!(table.contains("command: \"tui\""), "{table}");
    assert!(table.contains("command: \"run\""), "{table}");
    assert!(table.contains("takes_prompt: true"), "{table}");
    assert!(table.contains("front::__front_tui"), "{table}");
}

#[test]
fn dropping_the_front_crate_drops_its_commands() {
    let (_dir, root) = build_fixture(false);
    cargo_check(&root);
    let table = generated(&root);
    assert!(!table.contains("command: \"tui\""), "{table}");
    assert!(!table.contains("command: \"run\""), "{table}");
    assert!(table.contains("key: \"core\""), "{table}");
}
