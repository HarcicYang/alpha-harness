//! Static policy rules and protected patterns.

use serde::{Deserialize, Serialize};

use crate::request::PermissionRequest;

/// Allow or deny.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleDecision {
    /// Allow the action.
    Allow,
    /// Deny the action.
    Deny,
}

/// One policy rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    /// Rule decision.
    pub decision: RuleDecision,
    /// Action pattern, for example `fs.*` or `shell.exec`.
    pub action: String,
    /// Optional resource pattern.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
}

impl Rule {
    /// Whether this rule matches a request.
    pub fn matches(&self, request: &PermissionRequest) -> bool {
        if !wildcard_match(&self.action, &request.action) {
            return false;
        }
        match &self.resource {
            None => true,
            Some(pattern) => request
                .resources
                .iter()
                .any(|resource| wildcard_match(pattern, resource)),
        }
    }
}

/// Static policy: rules plus protected patterns.
///
/// Protected patterns match against `action:resource`. Protected actions are
/// never auto-allowed by rules; they require approval outside `free` mode.
#[derive(Clone, Debug, Default)]
pub struct Policy {
    rules: Vec<Rule>,
    protected: Vec<String>,
}

impl Policy {
    /// Build a policy.
    pub fn new(rules: Vec<Rule>, protected: Vec<String>) -> Self {
        Self { rules, protected }
    }

    /// The configured rules.
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// The configured protected patterns.
    pub fn protected(&self) -> &[String] {
        &self.protected
    }

    /// Evaluate rules. Deny wins over allow.
    pub fn evaluate(&self, request: &PermissionRequest) -> Option<RuleDecision> {
        if self
            .rules
            .iter()
            .any(|rule| rule.decision == RuleDecision::Deny && rule.matches(request))
        {
            return Some(RuleDecision::Deny);
        }
        if self
            .rules
            .iter()
            .any(|rule| rule.decision == RuleDecision::Allow && rule.matches(request))
        {
            return Some(RuleDecision::Allow);
        }
        None
    }

    /// Whether the request touches a protected pattern.
    pub fn is_protected(&self, request: &PermissionRequest) -> bool {
        self.protected.iter().any(|pattern| {
            if request.resources.is_empty() {
                return wildcard_match(pattern, &request.action);
            }
            request.resources.iter().any(|resource| {
                wildcard_match(pattern, &format!("{}:{}", request.action, resource))
            })
        })
    }

    /// Merge a project policy on top of this one.
    ///
    /// Project deny rules always apply. Project allow rules apply only when
    /// the project is explicitly trusted; otherwise the project can only
    /// tighten the policy.
    pub fn merge_project(&self, project: &Policy, trust_project_allow: bool) -> Policy {
        let mut rules = self.rules.clone();
        for rule in &project.rules {
            match rule.decision {
                RuleDecision::Deny => rules.push(rule.clone()),
                RuleDecision::Allow if trust_project_allow => rules.push(rule.clone()),
                RuleDecision::Allow => {}
            }
        }
        let mut protected = self.protected.clone();
        protected.extend(project.protected.iter().cloned());
        Policy { rules, protected }
    }
}

/// Protected patterns applied in every session.
pub fn default_protected() -> Vec<String> {
    [
        "fs.*:*.env",
        "fs.*:*.env.*",
        "fs.*:*/id_rsa",
        "fs.*:*.ssh/*",
        "fs.*:*.aws/credentials",
        "fs.write:*.git/*",
        "fs.write:*/.git/*",
        "fs.write:*.alpha/*",
        "fs.write:*/.alpha/*",
        "git.push:*",
        "publish:*",
        "deploy:*",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

/// Wildcard matching for `*` (any run) and `?` (one character).
pub fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let (mut pattern_index, mut value_index) = (0usize, 0usize);
    let (mut star_index, mut star_value_index) = (None, 0usize);

    while value_index < value.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == b'?' || pattern[pattern_index] == value[value_index])
        {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star_index = Some(pattern_index);
            star_value_index = value_index;
            pattern_index += 1;
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            star_value_index += 1;
            value_index = star_value_index;
        } else {
            return false;
        }
    }
    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}
