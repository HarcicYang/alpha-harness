//! Permission contract: requests, approvals, audit, and the service trait.
//!
//! The fixed shell and the tools module consume these types only, so the
//! permission implementation is a replaceable dependency.

#![forbid(unsafe_code)]

pub mod request;
pub mod review;

use std::sync::Mutex;

use alpha_kernel::Ctx;
use alpha_schema::PermissionMode;
use futures::future::BoxFuture;

pub use request::{ApprovalDecision, GrantScope, PermissionOutcome, PermissionRequest};
pub use review::{ApprovalReviewer, IntentClassifier, IntentDecision, ReviewDecision};

/// Permission checks used by tools.
pub trait PermissionService: Send + Sync + 'static {
    /// Decide whether a request may proceed.
    fn check(&self, request: PermissionRequest) -> BoxFuture<'_, PermissionOutcome>;
    /// The effective mode for later checks.
    fn mode(&self) -> PermissionMode;
    /// Change the effective mode for later checks.
    fn set_mode(&self, mode: PermissionMode);
}

/// Prompt shown to an approval channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalPrompt {
    /// The original request.
    pub request: PermissionRequest,
    /// Why approval is needed.
    pub reason: String,
    /// Whether a protected pattern triggered the prompt.
    pub protected: bool,
}

/// Interactive approval channel.
pub trait ApprovalChannel: Send + Sync + 'static {
    /// Ask for a decision.
    fn decide(&self, prompt: ApprovalPrompt) -> BoxFuture<'_, ApprovalDecision>;
}

/// Where a decision came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecisionSource {
    /// A session grant covered the request.
    Grant,
    /// A static rule decided.
    Policy,
    /// The mode default decided.
    Default,
    /// A protected pattern forced the path.
    Protected,
    /// The reviewed-mode intent classifier allowed the request.
    IntentClassifier,
    /// Automatic approval review decided.
    ApprovalReviewer,
    /// An approval channel decided.
    Approval,
    /// The request needed approval but no channel existed.
    NoChannel,
    /// The approval window expired.
    Timeout,
}

/// One audit record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditRecord {
    /// The checked request.
    pub request: PermissionRequest,
    /// The resulting outcome.
    pub outcome: PermissionOutcome,
    /// Where the decision came from.
    pub source: DecisionSource,
}

/// Receives audit records; the app wires this to durable session events.
pub trait AuditSink: Send + Sync + 'static {
    /// Record one decision.
    fn record(&self, record: AuditRecord);
}

/// Audit sink that collects records in memory for the caller to drain.
#[derive(Default)]
pub struct CollectingAuditSink {
    records: Mutex<Vec<AuditRecord>>,
}

impl DecisionSource {
    /// Stable label used in durable audit events.
    pub fn label(self) -> &'static str {
        match self {
            Self::Grant => "grant",
            Self::Policy => "policy",
            Self::Default => "default",
            Self::Protected => "protected",
            Self::IntentClassifier => "intent_classifier",
            Self::ApprovalReviewer => "approval_reviewer",
            Self::Approval => "approval",
            Self::NoChannel => "no_channel",
            Self::Timeout => "timeout",
        }
    }
}

impl CollectingAuditSink {
    /// Create an empty sink.
    pub fn new() -> Self {
        Self::default()
    }

    /// Take every collected record.
    pub fn drain(&self) -> Vec<AuditRecord> {
        std::mem::take(&mut *self.records.lock().expect("audit lock poisoned"))
    }
}

impl AuditSink for CollectingAuditSink {
    fn record(&self, record: AuditRecord) {
        self.records
            .lock()
            .expect("audit lock poisoned")
            .push(record);
    }
}
