//! Permission decisions, approvals, and audit records.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use alpha_schema::PermissionMode;
use futures::future::BoxFuture;

use crate::Policy;
use crate::policy::RuleDecision;
use crate::request::{ApprovalDecision, GrantScope, PermissionOutcome, PermissionRequest};

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
    /// An approval channel decided.
    Approval,
    /// The request needed approval but no channel existed.
    NoChannel,
    /// The approval window expired.
    Timeout,
}

impl DecisionSource {
    /// Stable label used in durable audit events.
    pub fn label(self) -> &'static str {
        match self {
            Self::Grant => "grant",
            Self::Policy => "policy",
            Self::Default => "default",
            Self::Protected => "protected",
            Self::Approval => "approval",
            Self::NoChannel => "no_channel",
            Self::Timeout => "timeout",
        }
    }
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

/// Local permission service implementing the M1 modes.
pub struct LocalPermissionService {
    mode: Mutex<PermissionMode>,
    policy: Policy,
    grants: Mutex<BTreeSet<(String, String)>>,
    channel: Option<Arc<dyn ApprovalChannel>>,
    audit: Option<Arc<dyn AuditSink>>,
}

impl LocalPermissionService {
    /// Create a service for a mode and policy.
    pub fn new(mode: PermissionMode, policy: Policy) -> Self {
        Self {
            mode: Mutex::new(mode),
            policy,
            grants: Mutex::new(BTreeSet::new()),
            channel: None,
            audit: None,
        }
    }

    /// Attach an approval channel.
    pub fn with_channel(mut self, channel: Arc<dyn ApprovalChannel>) -> Self {
        self.channel = Some(channel);
        self
    }

    /// Attach an audit sink.
    pub fn with_audit(mut self, audit: Arc<dyn AuditSink>) -> Self {
        self.audit = Some(audit);
        self
    }

    /// The effective mode.
    pub fn mode(&self) -> PermissionMode {
        *self.mode.lock().expect("mode lock poisoned")
    }

    /// Current session grants.
    pub fn session_grants(&self) -> Vec<(String, String)> {
        self.grants
            .lock()
            .expect("grant lock poisoned")
            .iter()
            .cloned()
            .collect()
    }

    fn granted(&self, request: &PermissionRequest) -> bool {
        if request.resources.is_empty() {
            return false;
        }
        let grants = self.grants.lock().expect("grant lock poisoned");
        request
            .resources
            .iter()
            .all(|resource| grants.contains(&(request.action.clone(), resource.clone())))
    }

    async fn decide(&self, request: &PermissionRequest) -> (PermissionOutcome, DecisionSource) {
        if self.granted(request) {
            return (
                PermissionOutcome::Allowed {
                    scope: GrantScope::Session,
                },
                DecisionSource::Grant,
            );
        }

        if self.policy.is_protected(request) {
            return match self.mode() {
                PermissionMode::Free => (
                    PermissionOutcome::Allowed {
                        scope: GrantScope::Once,
                    },
                    DecisionSource::Protected,
                ),
                PermissionMode::Read => (
                    denied("read mode denies protected mutations"),
                    DecisionSource::Protected,
                ),
                PermissionMode::Auto | PermissionMode::Reviewed => self.ask(request, true).await,
            };
        }

        match self.policy.evaluate(request) {
            Some(RuleDecision::Deny) => (denied("denied by policy"), DecisionSource::Policy),
            Some(RuleDecision::Allow) => (
                PermissionOutcome::Allowed {
                    scope: GrantScope::Once,
                },
                DecisionSource::Policy,
            ),
            None => match self.mode() {
                PermissionMode::Free => (
                    PermissionOutcome::Allowed {
                        scope: GrantScope::Once,
                    },
                    DecisionSource::Default,
                ),
                PermissionMode::Read => {
                    if is_read_only(&request.action) {
                        (
                            PermissionOutcome::Allowed {
                                scope: GrantScope::Once,
                            },
                            DecisionSource::Default,
                        )
                    } else {
                        (
                            denied("read mode denies mutations"),
                            DecisionSource::Default,
                        )
                    }
                }
                PermissionMode::Auto | PermissionMode::Reviewed => self.ask(request, false).await,
            },
        }
    }

    async fn ask(
        &self,
        request: &PermissionRequest,
        protected: bool,
    ) -> (PermissionOutcome, DecisionSource) {
        let Some(channel) = &self.channel else {
            return (
                denied("no approval channel available; failing closed"),
                DecisionSource::NoChannel,
            );
        };
        let prompt = ApprovalPrompt {
            request: request.clone(),
            reason: request
                .reason
                .clone()
                .unwrap_or_else(|| "permission requested".to_owned()),
            protected,
        };
        match channel.decide(prompt).await {
            ApprovalDecision::Deny => (denied("denied by user"), DecisionSource::Approval),
            ApprovalDecision::Timeout => (denied("approval timed out"), DecisionSource::Timeout),
            ApprovalDecision::AllowOnce => (
                PermissionOutcome::Allowed {
                    scope: GrantScope::Once,
                },
                DecisionSource::Approval,
            ),
            ApprovalDecision::AllowSession => {
                let mut grants = self.grants.lock().expect("grant lock poisoned");
                for resource in &request.resources {
                    grants.insert((request.action.clone(), resource.clone()));
                }
                drop(grants);
                (
                    PermissionOutcome::Allowed {
                        scope: GrantScope::Session,
                    },
                    DecisionSource::Approval,
                )
            }
        }
    }
}

impl PermissionService for LocalPermissionService {
    fn check(&self, request: PermissionRequest) -> BoxFuture<'_, PermissionOutcome> {
        Box::pin(async move {
            let (outcome, source) = self.decide(&request).await;
            if let Some(audit) = &self.audit {
                audit.record(AuditRecord {
                    request,
                    outcome: outcome.clone(),
                    source,
                });
            }
            outcome
        })
    }

    fn set_mode(&self, mode: PermissionMode) {
        *self.mode.lock().expect("mode lock poisoned") = mode;
    }

    fn mode(&self) -> PermissionMode {
        *self.mode.lock().expect("mode lock poisoned")
    }
}

fn denied(reason: &str) -> PermissionOutcome {
    PermissionOutcome::Denied {
        reason: reason.to_owned(),
    }
}

fn is_read_only(action: &str) -> bool {
    [
        "fs.read",
        "fs.list",
        "fs.stat",
        "http.get",
        "tool.read",
        "tool.grep",
    ]
    .iter()
    .any(|prefix| action == *prefix || action.starts_with(prefix))
}
