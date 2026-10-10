//! Permission modes, approvals, and policy evaluation.
//!
//! Modes: `read` denies mutations, `auto` asks on request and fails closed
//! without an approval channel, `free` allows everything. `reviewed` behaves
//! like `auto` until the review chain lands in M1.5.

#![forbid(unsafe_code)]

pub mod policy;
pub mod request;
pub mod service;

use std::sync::Arc;

use alpha_kernel::{ActivateError, CapabilityScope, CapabilitySpec, Ctx, Module};
use alpha_schema::PermissionMode;
use serde::Deserialize;

/// Stable module key used by the generated composition table.
pub const __ALPHA_MODULE__: &str = "permission";

pub use policy::{Policy, Rule, RuleDecision, default_protected, wildcard_match};
pub use request::{ApprovalDecision, GrantScope, PermissionOutcome, PermissionRequest};
pub use service::{
    ApprovalChannel, ApprovalPrompt, AuditRecord, AuditSink, DecisionSource,
    LocalPermissionService, PermissionService,
};

/// Configuration for the permission module.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct PermissionConfig {
    /// Effective permission mode.
    #[serde(default)]
    pub mode: PermissionMode,
    /// Explicit policy rules.
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Additional protected patterns.
    #[serde(default)]
    pub protected: Vec<String>,
}

/// Provides the permission service.
pub struct PermissionModule;

impl Module for PermissionModule {
    type Requirements = ();
    type Config = PermissionConfig;

    const KEY: &'static str = "permission";
    const CAPABILITIES: &'static [CapabilitySpec] = &[CapabilitySpec::singleton(
        "permission",
        CapabilityScope::Location,
    )];

    fn activate(&self, ctx: &Ctx, config: &Self::Config) -> Result<(), ActivateError> {
        let mut protected = default_protected();
        protected.extend(config.protected.iter().cloned());
        let policy = Policy::new(config.rules.clone(), protected);
        let service: Arc<dyn PermissionService> =
            Arc::new(LocalPermissionService::new(config.mode, policy));
        ctx.provide::<dyn PermissionService>(service)
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        Ok(())
    }
}
