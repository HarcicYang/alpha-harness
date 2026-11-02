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
use alpha_prompt_api::{PromptError, PromptPriority, PromptRequest, PromptSectionProvider};
use alpha_schema::PermissionMode;
use serde::Deserialize;

/// Stable module key used by the generated composition table.
pub const __ALPHA_MODULE__: &str = "permission";

pub use alpha_permission_api::{
    ApprovalChannel, ApprovalDecision, ApprovalPrompt, ApprovalReviewer, AuditRecord, AuditSink,
    CollectingAuditSink, DecisionSource, GrantScope, IntentClassifier, IntentDecision,
    PermissionOutcome, PermissionRequest, PermissionService, ReviewDecision,
};
pub use policy::{Policy, Rule, RuleDecision, default_protected, wildcard_match};
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
        // The modes are this module's facts, so it also owns how the model is
        // told about them.
        alpha_prompt_api::section_registry(ctx)
            .and_then(|registry| registry.register(ctx, Arc::new(PermissionModesSection)))
            .map_err(|error| ActivateError::new(Self::KEY, error))?;
        Ok(())
    }
}

/// Permission preset and approval-flow reference.
pub struct PermissionModesSection;

impl PromptSectionProvider for PermissionModesSection {
    fn key(&self) -> &'static str {
        "permission-modes"
    }

    fn priority(&self) -> PromptPriority {
        PromptPriority::Policy
    }

    fn render(&self, request: &PromptRequest<'_>) -> Result<Option<String>, PromptError> {
        let _ = request;
        Ok(Some(
            "# Permission Presets\n\n\
             The session runs in one of four modes. Modes are switched by the user, never by you.\n\
             - `read`: read-only workspace access with network disabled. Reads and searches work; mutations, shell execution, and protected operations are denied without prompting.\n\
             - `auto`: the standard workspace-write preset. It uses a workspace-write sandbox with network disabled and keeps protected paths such as `.git` read-only. Reads, edits, and commands that remain inside the sandbox run without prompting; protected operations and requests that cross the sandbox boundary require user approval.\n\
             - `reviewed`: uses the same sandbox boundary as `auto`. Sandbox-contained requests run directly; other requests first pass through the intent classifier, then automatic approval review when the classifier hands them off. A classifier or reviewer allow permits execution; reviewer denial is final. Protected operations require user approval.\n\
             - `free`: full local filesystem and process access with network enabled and no approval prompts.\n\
             Approval prompts offer deny, allow once, and allow for this session; there is no always-allow option, and session grants expire with the session. Never auto-approved: critical-path deletions, credential files, policy and sandbox configuration, push and publish/deploy operations, and cross-session messaging.\n\
             Do not attempt to bypass the active mode, the sandbox, or an approval decision."
                .to_owned(),
        ))
    }
}
