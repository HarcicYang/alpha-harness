use std::fs;

use alpha_kernel::{Ctx, Module};
use alpha_module_fs::{FsConfig, FsLocalModule};
use alpha_module_prompt::{PromptAssembler, PromptConfig, PromptModule, PromptPriority};
use alpha_module_session::{SessionHeader, SessionState, WorkspaceRef};
use alpha_schema::{PermissionMode, SessionId};

fn state() -> SessionState {
    let header = SessionHeader::new(
        SessionId::new(),
        PermissionMode::Auto,
        WorkspaceRef {
            root: "/workspace".to_owned(),
            cwd: "/workspace/project".to_owned(),
        },
    );
    SessionState::new(&header)
}

#[test]
fn module_assembles_builtin_sections_in_stable_order() {
    let ctx = Ctx::root();
    PromptModule
        .activate(&ctx, &PromptConfig::default())
        .expect("activate");
    let assembler = ctx
        .require::<dyn PromptAssembler>()
        .expect("prompt assembler");
    let prompt = assembler.assemble(&ctx, &state()).expect("assemble");

    let runtime = prompt
        .find("# Your Runtime and Operating Model")
        .expect("runtime");
    let rules = prompt.find("# Mandatory Rules").expect("rules");
    let output = prompt.find("# Your Output").expect("output");
    let tools = prompt.find("# Tool Use").expect("tools");
    let stop = prompt.find("# Stop Conditions").expect("stop");
    let persona = prompt.find("# Persona").expect("persona");

    assert!(runtime < rules);
    assert!(rules < output);
    assert!(output < tools);
    assert!(tools < stop);
    assert!(stop < persona);
    assert!(prompt.contains("`auto`"));
    assert!(prompt.contains("`/workspace/project`"));
}

#[test]
fn sections_are_removed_when_the_context_is_disposed() {
    let ctx = Ctx::root();
    PromptModule
        .activate(&ctx, &PromptConfig::default())
        .expect("activate");
    let assembler = ctx
        .require::<dyn PromptAssembler>()
        .expect("prompt assembler");
    let before = assembler.assemble(&ctx, &state()).expect("before");
    assert!(before.contains("# Your Output"));

    ctx.dispose();
    assert!(ctx.get::<dyn PromptAssembler>().is_none());
}

#[test]
fn priorities_have_framework_before_persona() {
    assert!(PromptPriority::Runtime < PromptPriority::Policy);
    assert!(PromptPriority::Policy < PromptPriority::Output);
    assert!(PromptPriority::Output < PromptPriority::Tools);
    assert!(PromptPriority::Tools < PromptPriority::Project);
    assert!(PromptPriority::Project < PromptPriority::Capability);
    assert!(PromptPriority::Capability < PromptPriority::Persona);
}

fn activate_with_fs(root: &std::path::Path) -> Ctx {
    let ctx = Ctx::root();
    FsLocalModule
        .activate(
            &ctx,
            &FsConfig {
                root: root.to_path_buf(),
            },
        )
        .expect("activate fs");
    PromptModule
        .activate(&ctx, &PromptConfig::default())
        .expect("activate prompt");
    ctx
}

#[test]
fn project_instructions_are_merged_from_root_to_cwd() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("AGENTS.md"), "root rules").expect("root instructions");
    fs::create_dir_all(dir.path().join(".alpha")).expect("alpha dir");
    fs::write(dir.path().join(".alpha/AGENTS.md"), "workspace rules")
        .expect("workspace instructions");
    fs::create_dir_all(dir.path().join("sub")).expect("subdir");
    fs::write(dir.path().join("sub/AGENTS.md"), "subproject rules").expect("sub instructions");

    let ctx = activate_with_fs(dir.path());
    let assembler = ctx
        .require::<dyn PromptAssembler>()
        .expect("prompt assembler");
    let state = SessionState::new(&SessionHeader::new(
        SessionId::new(),
        PermissionMode::Auto,
        WorkspaceRef {
            root: dir.path().display().to_string(),
            cwd: dir.path().join("sub").display().to_string(),
        },
    ));
    let prompt = assembler.assemble(&ctx, &state).expect("assemble");

    let root = prompt.find("root rules").expect("root");
    let workspace = prompt.find("workspace rules").expect("workspace");
    let sub = prompt.find("subproject rules").expect("sub");
    assert!(root < workspace);
    assert!(workspace < sub);
    assert!(prompt.contains("`AGENTS.md`"));
    assert!(prompt.contains("`.alpha/AGENTS.md`"));
    assert!(prompt.contains("`sub/AGENTS.md`"));
}

#[test]
fn project_instruction_limit_is_enforced() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("AGENTS.md"), "0123456789abcdef").expect("instructions");

    let ctx = Ctx::root();
    FsLocalModule
        .activate(
            &ctx,
            &FsConfig {
                root: dir.path().to_path_buf(),
            },
        )
        .expect("activate fs");
    PromptModule
        .activate(
            &ctx,
            &PromptConfig {
                include_project_instructions: true,
                max_project_instruction_bytes: 4,
            },
        )
        .expect("activate prompt");
    let assembler = ctx
        .require::<dyn PromptAssembler>()
        .expect("prompt assembler");
    let state = SessionState::new(&SessionHeader::new(
        SessionId::new(),
        PermissionMode::Auto,
        WorkspaceRef {
            root: dir.path().display().to_string(),
            cwd: dir.path().display().to_string(),
        },
    ));
    let prompt = assembler.assemble(&ctx, &state).expect("assemble");

    assert!(prompt.contains("0123"));
    assert!(prompt.contains("[project instructions truncated]"));
    assert!(!prompt.contains("456789"));
}
