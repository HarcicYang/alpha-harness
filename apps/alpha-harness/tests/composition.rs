use alpha_host_api::ModuleScope;

#[test]
fn generated_module_table_matches_the_composition() {
    let keys = alpha_harness::module_keys();
    for expected in [
        "config",
        "fs.local",
        "http.local",
        "loop",
        "permission",
        "provider.openai",
        "tools",
    ] {
        assert!(
            keys.contains(&expected),
            "missing module `{expected}` in {keys:?}"
        );
    }
}

#[test]
fn generated_fronts_cover_every_command() {
    let host = alpha_harness::composition::host();
    let commands: Vec<&str> = host.fronts.iter().map(|front| front.command).collect();
    for expected in ["desktop", "run", "chat", "tui"] {
        assert!(
            commands.contains(&expected),
            "missing front `{expected}` in {commands:?}"
        );
    }
    assert!(
        host.fronts
            .iter()
            .any(|front| front.command == "run" && front.takes_prompt),
        "`run` must take a prompt"
    );
}

#[test]
fn generated_modules_activate_in_scope_order() {
    let host = alpha_harness::composition::host();
    let scopes: Vec<ModuleScope> = host.modules.iter().map(|module| module.scope).collect();
    let rank = |scope: ModuleScope| match scope {
        ModuleScope::Global => 0,
        ModuleScope::Location => 1,
        ModuleScope::Agent => 2,
    };
    assert!(
        scopes.windows(2).all(|pair| rank(pair[0]) <= rank(pair[1])),
        "modules are not grouped by scope: {scopes:?}"
    );
}

#[test]
fn generated_modules_order_providers_before_consumers() {
    let host = alpha_harness::composition::host();
    let position = |key: &str| {
        host.modules
            .iter()
            .position(|module| module.key == key)
            .unwrap_or_else(|| panic!("missing module `{key}`"))
    };
    assert!(position("provider.api") < position("provider.deepseek"));
    assert!(position("fs.local") < position("prompt"));
    assert!(position("permission") < position("tools"));
}
