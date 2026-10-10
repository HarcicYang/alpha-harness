#[test]
fn reexports_module_descriptors() {
    assert_eq!(
        alpha_bundle_default::alpha_module_config::__ALPHA_MODULE__,
        "config"
    );
    assert_eq!(
        alpha_bundle_default::alpha_module_tools::__ALPHA_MODULE__,
        "tools"
    );
    assert_eq!(
        alpha_bundle_default::alpha_module_http::__ALPHA_MODULE__,
        "http.local"
    );
    assert_eq!(
        alpha_bundle_default::alpha_module_permission::__ALPHA_MODULE__,
        "permission"
    );
    assert_eq!(
        alpha_bundle_default::alpha_module_provider_openai::__ALPHA_MODULE__,
        "provider.openai"
    );
}

#[allow(unused)]
const _LINKAGE: () = {
    let _ = alpha_bundle_default::alpha_module_tools::__ALPHA_MODULE__;
    let _ = alpha_bundle_default::alpha_module_http::__ALPHA_MODULE__;
    let _ = alpha_bundle_default::alpha_module_permission::__ALPHA_MODULE__;
    let _ = alpha_bundle_default::alpha_module_provider_openai::__ALPHA_MODULE__;
};
