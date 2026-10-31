//! Permission requests and outcomes.

use serde::{Deserialize, Serialize};

/// One action that needs a decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRequest {
    /// Action key, for example `fs.write` or `shell.exec`.
    pub action: String,
    /// Resources touched by the action.
    pub resources: Vec<String>,
    /// Model-provided justification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Whether execution remains inside the active sandbox.
    #[serde(default)]
    pub sandboxed: bool,
}

impl PermissionRequest {
    /// Build a request.
    pub fn new(action: impl Into<String>, resources: impl IntoIterator<Item = String>) -> Self {
        Self {
            action: action.into(),
            resources: resources.into_iter().collect(),
            reason: None,
            sandboxed: false,
        }
    }

    /// Attach a justification.
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// Mark whether execution remains inside the active sandbox.
    pub fn with_sandboxed(mut self, sandboxed: bool) -> Self {
        self.sandboxed = sandboxed;
        self
    }
}

/// How long an approval lasts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantScope {
    /// This call only.
    Once,
    /// Until the session ends.
    Session,
}

/// User-facing approval choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// Reject the request.
    Deny,
    /// The approval window expired.
    Timeout,
    /// Allow this call only.
    AllowOnce,
    /// Allow for the rest of the session.
    AllowSession,
}

/// Outcome of a permission check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum PermissionOutcome {
    /// The action may proceed.
    Allowed {
        /// Effective grant scope.
        scope: GrantScope,
    },
    /// The action must not proceed.
    Denied {
        /// Why the action was denied.
        reason: String,
    },
}

impl PermissionOutcome {
    /// Whether the action may proceed.
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed { .. })
    }
}
