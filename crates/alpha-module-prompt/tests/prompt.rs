use std::fs;

use alpha_kernel::{Ctx, Module};
use alpha_module_fs::{FsConfig, FsLocalModule};
use alpha_module_loop::{ChatMessage, ContextInjector};
use alpha_module_prompt::{PromptAssembler, PromptConfig, PromptModule, PromptPriority};
use alpha_session_log::{SessionEvent, SessionHeader, SessionState, SessionStore, WorkspaceRef};
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
    let presets = prompt
        .find("# Permission Presets")
        .expect("permission presets");
    let output = prompt.find("# Your Output").expect("output");
    let tools = prompt.find("# Tool Use").expect("tools");
    let stop = prompt.find("# Stop Conditions").expect("stop");
    let persona = prompt.find("# Persona").expect("persona");

    assert!(runtime < rules);
    assert!(rules < presets);
    assert!(presets < output);
    assert!(output < tools);
    assert!(tools < stop);
    assert!(stop < persona);
    assert!(!prompt.contains("Current permission mode"));
    assert!(!prompt.contains("`/workspace/project`"));
    assert!(prompt.contains("Codex-compatible default"));
    assert!(prompt.contains("reviewer denial is final"));
    assert!(prompt.contains("GFM tables"));
    assert!(prompt.contains("fenced code blocks"));
}

#[test]
fn runtime_context_is_injected_once_and_updated_as_new_messages() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::new(dir.path());
    let header = SessionHeader::new(
        SessionId::new(),
        PermissionMode::Auto,
        WorkspaceRef {
            root: "/workspace".to_owned(),
            cwd: "/workspace/project".to_owned(),
        },
    );
    let writer = store.create(header.clone()).expect("create");
    let mut state = SessionState::new(&header);
    let ctx = Ctx::root();
    PromptModule
        .activate(&ctx, &PromptConfig::default())
        .expect("activate");
    let injector = ctx
        .require::<dyn ContextInjector>()
        .expect("context injector");
    let mut messages = Vec::new();

    futures::executor::block_on(injector.inject(&ctx, &writer, &mut state, &mut messages))
        .expect("inject initial context");
    assert_eq!(state.messages.len(), 1);
    assert_eq!(messages.len(), 1);
    let ChatMessage::User(first) = &messages[0] else {
        panic!("context must be a user message");
    };
    assert!(first.contains("key=\"runtime\""));
    assert!(first.contains("Permission mode: `auto`"));
    assert!(first.contains("`/workspace/project`"));
    let first_seq = writer.last_seq();

    futures::executor::block_on(injector.inject(&ctx, &writer, &mut state, &mut messages))
        .expect("deduplicate context");
    assert_eq!(writer.last_seq(), first_seq);
    assert_eq!(messages.len(), 1);

    let changed = writer
        .append(SessionEvent::ModeChanged {
            mode: PermissionMode::Free,
        })
        .expect("mode changed");
    state.apply(&changed);
    futures::executor::block_on(injector.inject(&ctx, &writer, &mut state, &mut messages))
        .expect("inject changed context");
    assert_eq!(messages.len(), 2);
    let ChatMessage::User(second) = &messages[1] else {
        panic!("context update must be a user message");
    };
    assert!(second.contains("Permission mode: `free`"));
    assert_eq!(state.messages.len(), 2);
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
    assert!(ctx.get::<dyn ContextInjector>().is_none());
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
                cwd: root.to_path_buf(),
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
                cwd: dir.path().to_path_buf(),
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
