use std::sync::Arc;

use alpha_kernel::{
    ActivateError, CapabilityKind, CapabilityScope, CapabilitySpec, Ctx, Module, ModuleDescriptor,
};
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
struct EchoConfig {
    prefix: String,
}

struct Echo {
    prefix: String,
}

impl Echo {
    fn say(&self, text: &str) -> String {
        format!("{}{}", self.prefix, text)
    }
}

struct EchoModule;

impl Module for EchoModule {
    type Requirements = ();
    type Config = EchoConfig;

    const KEY: &'static str = "echo";
    const CAPABILITIES: &'static [CapabilitySpec] = &[CapabilitySpec::singleton(
        "echo.service",
        CapabilityScope::Global,
    )];

    fn activate(&self, ctx: &Ctx, config: &Self::Config) -> Result<(), ActivateError> {
        let service = Arc::new(Echo {
            prefix: config.prefix.clone(),
        });
        ctx.provide::<Echo>(service)
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        Ok(())
    }
}

#[test]
fn module_activates_and_disposes_cleanly() {
    let ctx = Ctx::root();
    let config = EchoConfig {
        prefix: "> ".to_owned(),
    };
    EchoModule
        .activate(&ctx, &config)
        .expect("activation succeeds");

    assert_eq!(ctx.require::<Echo>().expect("service").say("hi"), "> hi");
    ctx.dispose();
    assert!(ctx.get::<Echo>().is_none());
}

#[test]
fn descriptors_expose_capabilities() {
    let descriptor = ModuleDescriptor::of::<EchoModule>();
    assert_eq!(descriptor.key, "echo");
    assert_eq!(descriptor.capabilities.len(), 1);
    assert_eq!(descriptor.capabilities[0].key, "echo.service");
    assert_eq!(descriptor.capabilities[0].kind, CapabilityKind::Singleton);
    assert_eq!(descriptor.capabilities[0].scope, CapabilityScope::Global);
}

#[test]
fn activate_errors_carry_the_module_key() {
    struct Failing;

    impl Module for Failing {
        type Requirements = ();
        type Config = ();

        const KEY: &'static str = "failing";

        fn activate(&self, _ctx: &Ctx, _config: &()) -> Result<(), ActivateError> {
            Err(ActivateError::message(Self::KEY, "boom"))
        }
    }

    let error = Failing.activate(&Ctx::root(), &()).expect_err("failure");
    assert_eq!(error.module(), "failing");
    assert!(error.to_string().contains("boom"));
}
