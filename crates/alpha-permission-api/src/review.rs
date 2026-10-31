//! Reviewed-mode classifier and automatic approval reviewer seams.

use futures::future::BoxFuture;

use crate::PermissionRequest;

/// Outcome of the intent classifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntentDecision {
    /// The request may execute.
    Allow,
    /// Continue through automatic approval review.
    Review,
}

/// Outcome of automatic approval review.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewDecision {
    /// The request may execute.
    Allow,
    /// The request must not execute.
    Deny,
}

/// First-stage reviewed-mode classifier.
pub trait IntentClassifier: Send + Sync + 'static {
    /// Classify one permission request.
    fn classify(&self, request: PermissionRequest) -> BoxFuture<'_, IntentDecision>;
}

/// Second-stage reviewed-mode approval reviewer.
pub trait ApprovalReviewer: Send + Sync + 'static {
    /// Review one permission request after classifier handoff.
    fn review(&self, request: PermissionRequest) -> BoxFuture<'_, ReviewDecision>;
}
