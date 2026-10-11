use alpha_kernel::{Ctx, Module};
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

    let runtime = prompt.find("# 你的运行环境与使用方式").expect("runtime");
    let rules = prompt.find("# 强制规则").expect("rules");
    let output = prompt.find("# 你的输出").expect("output");
    let tools = prompt.find("# 工具使用").expect("tools");
    let stop = prompt.find("# 停止条件").expect("stop");
    let persona = prompt.find("# 角色").expect("persona");

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
    assert!(before.contains("# 你的输出"));

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
