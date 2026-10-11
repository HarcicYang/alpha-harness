use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use alpha_kernel::{Ctx, Module};
use alpha_module_permission::{
    ApprovalChannel, ApprovalDecision, ApprovalPrompt, AuditRecord, AuditSink, DecisionSource,
    LocalPermissionService, PermissionConfig, PermissionModule, PermissionOutcome,
    PermissionRequest, PermissionService, Policy, Rule, RuleDecision, default_protected,
    wildcard_match,
};
use alpha_schema::PermissionMode;
use futures::future::BoxFuture;

#[derive(Default)]
struct Channel {
    decisions: Mutex<VecDeque<ApprovalDecision>>,
    prompts: Mutex<Vec<ApprovalPrompt>>,
}

impl Channel {
    fn with_decisions(decisions: impl IntoIterator<Item = ApprovalDecision>) -> Arc<Self> {
        Arc::new(Self {
            decisions: Mutex::new(decisions.into_iter().collect()),
            prompts: Mutex::new(Vec::new()),
        })
    }

    fn prompt_count(&self) -> usize {
        self.prompts.lock().expect("prompt lock").len()
    }
}

impl ApprovalChannel for Channel {
    fn decide(&self, prompt: ApprovalPrompt) -> BoxFuture<'_, ApprovalDecision> {
        self.prompts.lock().expect("prompt lock").push(prompt);
        let decision = self
            .decisions
            .lock()
            .expect("decision lock")
            .pop_front()
            .unwrap_or(ApprovalDecision::Deny);
        Box::pin(async move { decision })
    }
}

#[derive(Default)]
struct Audit {
    records: Mutex<Vec<AuditRecord>>,
}

impl AuditSink for Audit {
    fn record(&self, record: AuditRecord) {
        self.records.lock().expect("audit lock").push(record);
    }
}

fn request(action: &str, resource: &str) -> PermissionRequest {
    PermissionRequest::new(action, [resource.to_owned()])
}

#[tokio::test]
async fn read_mode_allows_reads_and_denies_mutations() {
    let service = LocalPermissionService::new(PermissionMode::Read, Policy::default());
    assert!(
        service
            .check(request("fs.read", "workspace/main.rs"))
            .await
            .is_allowed()
    );
    assert!(
        !service
            .check(request("fs.write", "workspace/main.rs"))
            .await
            .is_allowed()
    );
    assert!(
        !service
            .check(request("fs.write", "workspace/.env"))
            .await
            .is_allowed()
    );
}

#[tokio::test]
async fn set_mode_changes_later_checks() {
    let service = LocalPermissionService::new(PermissionMode::Free, Policy::default());
    assert!(
        service
            .check(request("fs.write", "workspace/main.rs"))
            .await
            .is_allowed()
    );
    service.set_mode(PermissionMode::Read);
    assert_eq!(service.mode(), PermissionMode::Read);
    assert!(
        !service
            .check(request("fs.write", "workspace/main.rs"))
            .await
            .is_allowed()
    );
}

#[tokio::test]
async fn session_grants_skip_later_prompts() {
    let channel = Channel::with_decisions([ApprovalDecision::AllowSession]);
    let service = LocalPermissionService::new(PermissionMode::Auto, Policy::default())
        .with_channel(channel.clone());

    let first = service
        .check(request("fs.write", "workspace/main.rs"))
        .await;
    assert!(matches!(
        first,
        PermissionOutcome::Allowed {
            scope: alpha_module_permission::GrantScope::Session
        }
    ));
    let second = service
        .check(request("fs.write", "workspace/main.rs"))
        .await;
    assert!(second.is_allowed());
    assert_eq!(channel.prompt_count(), 1);
    assert_eq!(service.session_grants().len(), 1);
}

#[tokio::test]
async fn once_grants_do_not_persist() {
    let channel =
        Channel::with_decisions([ApprovalDecision::AllowOnce, ApprovalDecision::AllowOnce]);
    let service = LocalPermissionService::new(PermissionMode::Auto, Policy::default())
        .with_channel(channel.clone());

    assert!(
        service
            .check(request("fs.write", "workspace/main.rs"))
            .await
            .is_allowed()
    );
    assert!(
        service
            .check(request("fs.write", "workspace/main.rs"))
            .await
            .is_allowed()
    );
    assert_eq!(channel.prompt_count(), 2);
    assert!(service.session_grants().is_empty());
}

#[tokio::test]
async fn auto_mode_fails_closed_without_a_channel() {
    let service = LocalPermissionService::new(PermissionMode::Auto, Policy::default());
    let outcome = service
        .check(request("fs.write", "workspace/main.rs"))
        .await;
    assert!(matches!(
        outcome,
        PermissionOutcome::Denied { ref reason } if reason.contains("no approval channel")
    ));
}

#[tokio::test]
async fn deny_rules_win_over_allow_rules() {
    let policy = Policy::new(
        vec![
            Rule {
                decision: RuleDecision::Allow,
                action: "fs.*".to_owned(),
                resource: None,
            },
            Rule {
                decision: RuleDecision::Deny,
                action: "fs.write".to_owned(),
                resource: None,
            },
        ],
        Vec::new(),
    );
    let service = LocalPermissionService::new(PermissionMode::Auto, policy);
    assert!(
        !service
            .check(request("fs.write", "workspace/main.rs"))
            .await
            .is_allowed()
    );
    assert!(
        service
            .check(request("fs.read", "workspace/main.rs"))
            .await
            .is_allowed()
    );
}

#[tokio::test]
async fn allow_rules_skip_approval_and_protected_actions_always_prompt() {
    let policy = Policy::new(
        vec![Rule {
            decision: RuleDecision::Allow,
            action: "fs.write".to_owned(),
            resource: None,
        }],
        default_protected(),
    );
    let plain = LocalPermissionService::new(PermissionMode::Auto, policy.clone());
    assert!(
        plain
            .check(request("fs.write", "workspace/main.rs"))
            .await
            .is_allowed()
    );

    let channel = Channel::with_decisions([ApprovalDecision::AllowOnce]);
    let protected =
        LocalPermissionService::new(PermissionMode::Auto, policy).with_channel(channel.clone());
    assert!(
        protected
            .check(request("fs.write", "workspace/.env"))
            .await
            .is_allowed()
    );
    assert_eq!(channel.prompt_count(), 1);
    let prompt = channel.prompts.lock().expect("prompt lock")[0].clone();
    assert!(prompt.protected);
}

#[tokio::test]
async fn free_mode_allows_everything_without_a_channel() {
    let service = LocalPermissionService::new(PermissionMode::Free, Policy::default());
    assert!(
        service
            .check(request("fs.write", "workspace/.env"))
            .await
            .is_allowed()
    );
    assert!(
        service
            .check(request("git.push", "origin/main"))
            .await
            .is_allowed()
    );
}

#[tokio::test]
async fn project_policies_only_tighten_unless_trusted() {
    let user = Policy::default();
    let project = Policy::new(
        vec![Rule {
            decision: RuleDecision::Allow,
            action: "shell.exec".to_owned(),
            resource: None,
        }],
        Vec::new(),
    );

    let untrusted = user.merge_project(&project, false);
    let service = LocalPermissionService::new(PermissionMode::Auto, untrusted);
    assert!(
        !service
            .check(request("shell.exec", "cargo test"))
            .await
            .is_allowed()
    );

    let trusted = user.merge_project(&project, true);
    let service = LocalPermissionService::new(PermissionMode::Auto, trusted);
    assert!(
        service
            .check(request("shell.exec", "cargo test"))
            .await
            .is_allowed()
    );
}

#[tokio::test]
async fn audit_records_every_decision() {
    let audit = Arc::new(Audit::default());
    let service = LocalPermissionService::new(PermissionMode::Free, Policy::default())
        .with_audit(audit.clone());
    service.check(request("fs.read", "workspace/main.rs")).await;
    let records = audit.records.lock().expect("audit lock");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].source, DecisionSource::Default);
    assert!(records[0].outcome.is_allowed());
}

#[test]
fn wildcard_matching_handles_stars_and_questions() {
    assert!(wildcard_match("fs.*", "fs.write"));
    assert!(wildcard_match("*.env", "workspace/.env"));
    assert!(wildcard_match("a?c", "abc"));
    assert!(!wildcard_match("a?c", "ac"));
    assert!(wildcard_match("*", "anything"));
}

#[test]
fn module_provides_the_permission_service() {
    let ctx = Ctx::root();
    PermissionModule
        .activate(&ctx, &PermissionConfig::default())
        .expect("activate");
    assert!(ctx.get::<dyn PermissionService>().is_some());
    ctx.dispose();
    assert!(ctx.get::<dyn PermissionService>().is_none());
}
