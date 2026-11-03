use std::path::Path;
use std::sync::Arc;

use alpha_kernel::{Ctx, Module};
use alpha_module_fs::{FsConfig, FsLocalModule};
use alpha_module_permission::{PermissionConfig, PermissionModule};
use alpha_module_process::{ProcessConfig, ProcessLocalModule};
use alpha_module_tools::{
    RegistryError, ToolContext, ToolError, ToolRegistry, ToolsConfig, ToolsModule,
};
use alpha_permission_api::PermissionService;
use alpha_schema::{MessageId, PermissionMode, SessionId, ToolCallId};
use serde_json::{Value, json};

fn setup(mode: PermissionMode, root: &Path) -> (Ctx, Arc<ToolRegistry>) {
    let ctx = Ctx::root();
    FsLocalModule
        .activate(
            &ctx,
            &FsConfig {
                root: root.to_path_buf(),
                cwd: root.to_path_buf(),
            },
        )
        .expect("fs");
    ProcessLocalModule
        .activate(
            &ctx,
            &ProcessConfig {
                cwd: root.to_path_buf(),
                ..ProcessConfig::default()
            },
        )
        .expect("process");
    PermissionModule
        .activate(
            &ctx,
            &PermissionConfig {
                mode,
                ..PermissionConfig::default()
            },
        )
        .expect("permission");
    ToolsModule
        .activate(&ctx, &ToolsConfig::default())
        .expect("tools");
    let registry = ctx.require::<ToolRegistry>().expect("registry");
    (ctx, registry)
}

fn call() -> ToolContext {
    ToolContext::new(SessionId::new(), MessageId::new(), ToolCallId::new())
}

async fn run(
    registry: &ToolRegistry,
    name: &str,
    input: Value,
) -> Result<alpha_module_tools::ToolResponse, ToolError> {
    let tool = registry.get(name).expect("tool");
    tool.execute(input, call()).await
}

#[tokio::test]
async fn write_read_and_edit_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_ctx, registry) = setup(PermissionMode::Free, dir.path());

    let write = run(
        &registry,
        "write",
        json!({ "path": "notes.txt", "content": "hello world" }),
    )
    .await
    .expect("write");
    assert!(write.content.contains("Wrote 11 bytes"));

    let read = run(&registry, "read", json!({ "path": "notes.txt" }))
        .await
        .expect("read");
    assert_eq!(read.content, "hello world");
    assert_eq!(read.output.expect("output")["lines"], json!(1));

    let edit = run(
        &registry,
        "edit",
        json!({ "path": "notes.txt", "old": "world", "new": "rust" }),
    )
    .await
    .expect("edit");
    assert!(edit.content.contains("Replaced 1"));
    assert_eq!(
        run(&registry, "read", json!({ "path": "notes.txt" }))
            .await
            .expect("read")
            .content,
        "hello rust"
    );
}

#[tokio::test]
async fn read_reports_empty_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("empty.txt"), b"").expect("write empty");
    let (_ctx, registry) = setup(PermissionMode::Free, dir.path());

    let empty = run(&registry, "read", json!({ "path": "empty.txt" }))
        .await
        .expect("read empty");
    assert_eq!(empty.content, "(empty file)");
}

#[tokio::test]
async fn auto_mode_rejects_files_outside_the_workspace() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_ctx, registry) = setup(PermissionMode::Auto, dir.path());

    let error = run(&registry, "read", json!({ "path": "../outside.txt" }))
        .await
        .expect_err("escape");
    assert!(matches!(error, ToolError::Failure(_)));
    assert!(error.to_string().contains("escapes the workspace root"));
}

#[tokio::test]
async fn free_mode_writes_outside_the_workspace() {
    let workspace = tempfile::tempdir().expect("workspace");
    let outside = tempfile::tempdir().expect("outside");
    let (_ctx, registry) = setup(PermissionMode::Free, workspace.path());
    let path = outside.path().join("outside.txt");

    run(
        &registry,
        "write",
        json!({ "path": path, "content": "full access" }),
    )
    .await
    .expect("write outside workspace");
    assert_eq!(
        std::fs::read_to_string(&path).expect("outside file"),
        "full access"
    );
}

#[tokio::test]
async fn tools_run_unrestricted_without_a_permission_module() {
    let workspace = tempfile::tempdir().expect("workspace");
    let outside = tempfile::tempdir().expect("outside");
    let ctx = Ctx::root();
    FsLocalModule
        .activate(
            &ctx,
            &FsConfig {
                root: workspace.path().to_path_buf(),
                cwd: workspace.path().to_path_buf(),
            },
        )
        .expect("fs");
    ProcessLocalModule
        .activate(
            &ctx,
            &ProcessConfig {
                cwd: workspace.path().to_path_buf(),
                ..ProcessConfig::default()
            },
        )
        .expect("process");
    ToolsModule
        .activate(&ctx, &ToolsConfig::default())
        .expect("tools");
    let registry = ctx.require::<ToolRegistry>().expect("registry");
    assert!(ctx.get::<dyn PermissionService>().is_none());

    let path = outside.path().join("outside.txt");
    run(
        &registry,
        "write",
        json!({ "path": path, "content": "no permission" }),
    )
    .await
    .expect("write outside workspace");
    assert_eq!(
        std::fs::read_to_string(&path).expect("outside file"),
        "no permission"
    );
}

#[tokio::test]
async fn switching_to_free_updates_the_execution_boundary_immediately() {
    let workspace = tempfile::tempdir().expect("workspace");
    let outside = tempfile::tempdir().expect("outside");
    let (ctx, registry) = setup(PermissionMode::Auto, workspace.path());
    let path = outside.path().join("switched.txt");

    let error = run(
        &registry,
        "write",
        json!({ "path": path, "content": "blocked" }),
    )
    .await
    .expect_err("auto boundary");
    assert!(error.to_string().contains("escapes the workspace root"));

    ctx.require::<dyn PermissionService>()
        .expect("permission")
        .set_mode(PermissionMode::Free);
    run(
        &registry,
        "write",
        json!({ "path": path, "content": "allowed" }),
    )
    .await
    .expect("free boundary");
    assert_eq!(
        std::fs::read_to_string(&path).expect("outside file"),
        "allowed"
    );
}

#[tokio::test]
async fn edit_requires_unique_matches_unless_replace_all() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("file.txt"), b"aa").expect("write");
    let (_ctx, registry) = setup(PermissionMode::Free, dir.path());

    let error = run(
        &registry,
        "edit",
        json!({ "path": "file.txt", "old": "a", "new": "b" }),
    )
    .await
    .expect_err("ambiguous");
    assert!(error.to_string().contains("replace_all"));

    let edit = run(
        &registry,
        "edit",
        json!({
            "path": "file.txt",
            "old": "a",
            "new": "b",
            "replace_all": true
        }),
    )
    .await
    .expect("replace all");
    assert_eq!(edit.output.expect("output")["replacements"], json!(2));
}

#[tokio::test]
async fn invalid_inputs_and_denied_permissions_are_model_visible() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_ctx, registry) = setup(PermissionMode::Free, dir.path());
    let error = run(&registry, "read", json!({}))
        .await
        .expect_err("invalid input");
    assert!(error.to_string().contains("invalid input"));

    let (_ctx, registry) = setup(PermissionMode::Read, dir.path());
    let error = run(
        &registry,
        "write",
        json!({ "path": "file.txt", "content": "nope" }),
    )
    .await
    .expect_err("denied");
    assert!(error.to_string().contains("permission denied"));
}

#[test]
fn registry_rejects_duplicates_and_exposes_metadata() {
    let registry = ToolRegistry::new();
    let dir = tempfile::tempdir().expect("tempdir");
    let (_ctx, tools) = setup(PermissionMode::Free, dir.path());
    let read = tools.get("read").expect("read");
    let write = tools.get("write").expect("write");

    registry.register(read.clone()).expect("first");
    assert!(matches!(
        registry.register(read.clone()),
        Err(RegistryError::Duplicate(_))
    ));
    assert!(read.parallel_safe());
    assert!(!write.parallel_safe());
    assert!(!read.description().is_empty());
    assert!(tools.get("grep").is_some());
    assert!(tools.get("shell").is_some());
    assert!(tools.get("view_image").is_some());
}

#[tokio::test]
async fn view_image_returns_model_visible_image_content() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("pixel.png"), b"\x89PNG\r\n\x1a\n").expect("image");
    let (_ctx, registry) = setup(PermissionMode::Free, dir.path());

    let response = run(&registry, "view_image", json!({ "path": "pixel.png" }))
        .await
        .expect("view image");
    assert_eq!(response.images.len(), 1);
    assert_eq!(response.images[0].name, "pixel.png");
    assert_eq!(response.images[0].mime_type, "image/png");
    assert!(!response.images[0].data.is_empty());
}

#[tokio::test]
async fn shell_is_probed_before_schema_metadata_is_read() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_ctx, registry) = setup(PermissionMode::Free, dir.path());
    let shell = registry.get("shell").expect("shell");
    assert!(shell.description().contains("probed on first use"));

    registry.prepare_all().await.expect("prepare tools");

    let description = shell.description();
    assert!(!description.contains("probed on first use"));
    registry.prepare_all().await.expect("prepare tools again");
    assert_eq!(shell.description(), description);
}

#[tokio::test]
async fn grep_finds_matches_with_filters() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join("src")).expect("mkdir");
    std::fs::write(
        dir.path().join("src/lib.rs"),
        b"fn main() {}\n// TODO fix\n",
    )
    .expect("write lib");
    std::fs::write(dir.path().join("README.md"), b"TODO in readme\n").expect("write readme");
    let (_ctx, registry) = setup(PermissionMode::Free, dir.path());

    let all = run(&registry, "grep", json!({ "pattern": "TODO" }))
        .await
        .expect("grep");
    assert!(all.content.contains("src/lib.rs:2:"));
    assert!(all.content.contains("README.md:1:"));
    assert_eq!(all.output.expect("output")["matches"], json!(2));

    let filtered = run(
        &registry,
        "grep",
        json!({ "pattern": "TODO", "include": "lib" }),
    )
    .await
    .expect("grep filtered");
    assert!(filtered.content.contains("src/lib.rs"));
    assert!(!filtered.content.contains("README"));

    let glob = run(
        &registry,
        "grep",
        json!({ "pattern": "TODO", "include": "*.rs" }),
    )
    .await
    .expect("grep glob");
    assert!(glob.content.contains("src/lib.rs"));
    assert!(!glob.content.contains("README"));

    let none = run(&registry, "grep", json!({ "pattern": "NOPE" }))
        .await
        .expect("grep none");
    assert_eq!(none.content, "(no matches)");
}

#[cfg(unix)]
#[tokio::test]
async fn shell_runs_commands_and_reports_failures() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_ctx, registry) = setup(PermissionMode::Free, dir.path());

    let ok = run(
        &registry,
        "shell",
        json!({ "shell": "sh", "command": "printf hello" }),
    )
    .await
    .expect("shell");
    assert!(ok.content.contains("status: completed"));
    assert!(ok.content.contains("hello"));
    assert_eq!(ok.output.expect("output")["exit_code"], json!(0));

    let error = run(
        &registry,
        "shell",
        json!({ "shell": "sh", "command": "exit 3" }),
    )
    .await
    .expect_err("exit code");
    assert!(error.to_string().contains("status: failed"));
    assert!(error.to_string().contains("exit_code: 3"));

    let error = run(
        &registry,
        "shell",
        json!({ "shell": "definitely-not-a-shell", "command": "true" }),
    )
    .await
    .expect_err("unknown shell");
    assert!(error.to_string().contains("not available"));
}

#[cfg(unix)]
#[tokio::test]
async fn shell_jobs_continue_after_the_initial_wait_and_can_be_queried() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_ctx, registry) = setup(PermissionMode::Free, dir.path());

    let started = run(
        &registry,
        "shell",
        json!({
            "shell": "sh",
            "command": "printf first; sleep 0.05; printf second",
            "wait_ms": 0,
        }),
    )
    .await
    .expect("start background shell");
    let output = started.output.expect("start metadata");
    assert_eq!(output["status"], json!("running"));
    let job_id = output["job_id"].as_str().expect("job id");

    let finished = run(
        &registry,
        "shell",
        json!({ "job_id": job_id, "since_bytes": 0 }),
    )
    .await
    .expect("query shell");
    assert!(finished.content.contains("status: completed"));
    assert!(finished.content.contains("firstsecond"));
}
