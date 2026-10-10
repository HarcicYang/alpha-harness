use alpha_build::metadata::CapabilityKind;
use alpha_build::{BuildError, ModuleInfo, check};

fn module(key: &str) -> ModuleInfo {
    ModuleInfo {
        package: format!("alpha-module-{key}"),
        key: key.to_owned(),
        extern_path: format!("alpha_module_{key}"),
        api: 1,
        provides: Vec::new(),
        requires: Vec::new(),
        registry_entries: Vec::new(),
    }
}

fn singleton(key: &str, capability: &str) -> ModuleInfo {
    let mut info = module(key);
    info.provides = vec![(capability.to_owned(), CapabilityKind::Singleton)];
    info
}

#[test]
fn accepts_a_consistent_composition() {
    let mut tools = module("tools");
    tools.requires = vec!["fs".to_owned()];
    tools.registry_entries = vec!["read".to_owned(), "shell".to_owned()];
    let mut fs = module("fs");
    fs.provides = vec![("fs".to_owned(), CapabilityKind::Singleton)];

    check(&[tools, fs], &[]).expect("consistent composition");
}

#[test]
fn rejects_duplicate_singletons_without_an_override() {
    let error =
        check(&[singleton("a", "store"), singleton("b", "store")], &[]).expect_err("conflict");
    assert!(matches!(error, BuildError::SingletonConflict { .. }));
    assert!(error.to_string().contains("store"));
}

#[test]
fn explicit_overrides_resolve_singleton_conflicts() {
    let modules = [singleton("a", "store"), singleton("b", "store")];
    check(&modules, &["store=b".to_owned()]).expect("override resolves");
}

#[test]
fn rejects_overrides_that_do_not_select_a_provider() {
    let modules = [singleton("a", "store"), singleton("b", "store")];
    let error = check(&modules, &["store=c".to_owned()]).expect_err("invalid");
    assert!(matches!(error, BuildError::InvalidOverride { .. }));
}

#[test]
fn rejects_unknown_and_registry_overrides() {
    let error = check(&[singleton("a", "store")], &["missing=a".to_owned()]).expect_err("unknown");
    assert!(matches!(error, BuildError::UnknownOverride { .. }));

    let mut registry = module("r");
    registry.provides = vec![("tools".to_owned(), CapabilityKind::Registry)];
    let error = check(&[registry], &["tools=r".to_owned()]).expect_err("not overridable");
    assert!(matches!(error, BuildError::NotOverridable { .. }));
}

#[test]
fn rejects_missing_requirements() {
    let mut tools = module("tools");
    tools.requires = vec!["fs".to_owned()];
    let error = check(&[tools], &[]).expect_err("missing");
    assert!(matches!(error, BuildError::MissingRequirement { .. }));
}

#[test]
fn rejects_registry_entry_collisions() {
    let mut first = module("first");
    first.registry_entries = vec!["read".to_owned()];
    let mut second = module("second");
    second.registry_entries = vec!["read".to_owned()];
    let error = check(&[first, second], &[]).expect_err("collision");
    assert!(matches!(error, BuildError::RegistryConflict { .. }));
}

#[test]
fn rejects_unsupported_apis_and_empty_graphs() {
    let mut old = module("old");
    old.api = 0;
    let error = check(&[old], &[]).expect_err("api");
    assert!(matches!(error, BuildError::UnsupportedApi { .. }));

    let error = check(&[], &[]).expect_err("empty");
    assert!(matches!(error, BuildError::NoModules));
}
