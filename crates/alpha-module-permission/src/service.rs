//! Permission decisions, approvals, and audit records.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use alpha_schema::PermissionMode;
use futures::future::BoxFuture;

use crate::Policy;
use crate::policy::RuleDecision;
use alpha_permission_api::request::{
    ApprovalDecision, GrantScope, PermissionOutcome, PermissionRequest,
};
use alpha_permission_api::review::{
    ApprovalReviewer, IntentClassifier, IntentDecision, ReviewDecision,
};
use alpha_permission_api::{
    ApprovalChannel, ApprovalPrompt, AuditRecord, AuditSink, DecisionSource, PermissionService,
};

/// Local permission service implementing the M1 modes.
pub struct LocalPermissionService {
    mode: Mutex<PermissionMode>,
    policy: Policy,
    grants: Mutex<BTreeSet<(String, String)>>,
    classifier: Arc<dyn IntentClassifier>,
    reviewer: Arc<dyn ApprovalReviewer>,
    channel: Option<Arc<dyn ApprovalChannel>>,
    audit: Option<Arc<dyn AuditSink>>,
}

struct SandboxIntentClassifier;

impl IntentClassifier for SandboxIntentClassifier {
    fn classify(&self, request: PermissionRequest) -> BoxFuture<'_, IntentDecision> {
        Box::pin(async move {
            if request.sandboxed {
                IntentDecision::Allow
            } else {
                IntentDecision::Review
            }
        })
    }
}

struct ReadOnlyApprovalReviewer;

impl ApprovalReviewer for ReadOnlyApprovalReviewer {
    fn review(&self, request: PermissionRequest) -> BoxFuture<'_, ReviewDecision> {
        Box::pin(async move {
            if is_local_read_only(&request.action) {
                ReviewDecision::Allow
            } else {
                ReviewDecision::Deny
            }
        })
    }
}

impl LocalPermissionService {
    /// Create a service for a mode and policy.
    pub fn new(mode: PermissionMode, policy: Policy) -> Self {
        Self {
            mode: Mutex::new(mode),
            policy,
            grants: Mutex::new(BTreeSet::new()),
            classifier: Arc::new(SandboxIntentClassifier),
            reviewer: Arc::new(ReadOnlyApprovalReviewer),
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

    /// Override the reviewed-mode intent classifier.
    pub fn with_intent_classifier(mut self, classifier: Arc<dyn IntentClassifier>) -> Self {
        self.classifier = classifier;
        self
    }

    /// Override the reviewed-mode automatic approval reviewer.
    pub fn with_approval_reviewer(mut self, reviewer: Arc<dyn ApprovalReviewer>) -> Self {
        self.reviewer = reviewer;
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
                PermissionMode::Auto => {
                    if request.sandboxed {
                        (
                            PermissionOutcome::Allowed {
                                scope: GrantScope::Once,
                            },
                            DecisionSource::Default,
                        )
                    } else {
                        self.ask(request, false).await
                    }
                }
                PermissionMode::Reviewed => self.review(request).await,
            },
        }
    }

    async fn review(&self, request: &PermissionRequest) -> (PermissionOutcome, DecisionSource) {
        match self.classifier.classify(request.clone()).await {
            IntentDecision::Allow => (
                PermissionOutcome::Allowed {
                    scope: GrantScope::Once,
                },
                DecisionSource::IntentClassifier,
            ),
            IntentDecision::Review => match self.reviewer.review(request.clone()).await {
                ReviewDecision::Allow => (
                    PermissionOutcome::Allowed {
                        scope: GrantScope::Once,
                    },
                    DecisionSource::ApprovalReviewer,
                ),
                ReviewDecision::Deny => (
                    denied("denied by automatic approval review"),
                    DecisionSource::ApprovalReviewer,
                ),
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

fn is_local_read_only(action: &str) -> bool {
    ["fs.read", "fs.list", "fs.stat", "tool.read", "tool.grep"]
        .iter()
        .any(|prefix| action == *prefix || action.starts_with(prefix))
}
