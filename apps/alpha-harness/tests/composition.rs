#[test]
fn generated_module_table_matches_the_composition() {
    let keys = alpha_harness::module_keys();
    for expected in [
        "config",
        "fs.local",
        "http.local",
        "loop",
        "permission",
        "process.local",
        "provider.openai",
        "tools",
    ] {
        assert!(
            keys.contains(&expected),
            "missing module `{expected}` in {keys:?}"
        );
    }
}
