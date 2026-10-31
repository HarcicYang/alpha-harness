//! Permission modes, approvals, and policy evaluation.
//!
//! Modes: `read` denies mutations, `auto` asks on request and fails closed
//! without an approval channel, `free` allows everything. `reviewed` routes
//! requests through the intent classifier and approval reviewer chain; the
//! defaults allow sandboxed and local read-only requests and deny other
//! unsandboxed requests.

#![forbid(unsafe_code)]

pub mod policy;
pub mod service;

use std::sync::Arc;

use alpha_kernel::{ActivateError, CapabilityScope, CapabilitySpec, Ctx, Module};
use alpha_schema::PermissionMode;
use serde::Deserialize;

/// Stable module key used by the generated composition table.
pub const __ALPHA_MODULE__: &str = "permission";

pub use policy::{Policy, Rule, RuleDecision, default_protected, wildcard_match};
pub use alpha_permission_api::{
    ApprovalChannel, ApprovalDecision, ApprovalPrompt, ApprovalReviewer, AuditRecord, AuditSink,
    CollectingAuditSink, DecisionSource, GrantScope, IntentClassifier, IntentDecision,
    PermissionOutcome, PermissionRequest, PermissionService, ReviewDecision,
};
pub use service::LocalPermissionService;


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
        let mut service = LocalPermissionService::new(config.mode, policy);
        if let Some(classifier) = ctx.get::<dyn IntentClassifier>() {
            service = service.with_intent_classifier(classifier);
        }
        if let Some(reviewer) = ctx.get::<dyn ApprovalReviewer>() {
            service = service.with_approval_reviewer(reviewer);
        }
        if let Some(channel) = ctx.get::<dyn ApprovalChannel>() {
            service = service.with_channel(channel);
        }
        if let Some(audit) = ctx.get::<dyn AuditSink>() {
            service = service.with_audit(audit);
        }
        let service: Arc<dyn PermissionService> = Arc::new(service);
        ctx.provide::<dyn PermissionService>(service)
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        Ok(())
    }
}
