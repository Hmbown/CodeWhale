pub mod bash_arity;

use std::collections::HashSet;

use anyhow::Result;
use bash_arity::BashArityDict;
use codewhale_protocol::{NetworkPolicyAmendment, NetworkPolicyRuleAction};
use serde::{Deserialize, Serialize};

/// Priority layer for a permission ruleset. Higher ordinal = higher priority.
/// On conflict, the highest-priority layer's longest matching prefix wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RulesetLayer {
    BuiltinDefault = 0,
    Agent = 1,
    User = 2,
}

/// A named set of allow/deny prefix rules at a given priority layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ruleset {
    pub layer: RulesetLayer,
    pub trusted_prefixes: Vec<String>,
    pub denied_prefixes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ask_rules: Vec<ToolAskRule>,
}

impl Ruleset {
    pub fn builtin_default() -> Self {
        Self {
            layer: RulesetLayer::BuiltinDefault,
            trusted_prefixes: vec![],
            denied_prefixes: vec![],
            ask_rules: vec![],
        }
    }

    pub fn agent(trusted: Vec<String>, denied: Vec<String>) -> Self {
        Self {
            layer: RulesetLayer::Agent,
            trusted_prefixes: trusted,
            denied_prefixes: denied,
            ask_rules: vec![],
        }
    }

    pub fn user(trusted: Vec<String>, denied: Vec<String>) -> Self {
        Self {
            layer: RulesetLayer::User,
            trusted_prefixes: trusted,
            denied_prefixes: denied,
            ask_rules: vec![],
        }
    }

    pub fn with_ask_rules(mut self, ask_rules: Vec<ToolAskRule>) -> Self {
        self.ask_rules = ask_rules;
        self
    }
}

/// Typed rule that marks a tool invocation as requiring approval.
///
/// This foundation is intentionally ask-only. Existing trusted/denied command
/// prefix behavior is preserved while typed ask records can make
/// `AskForApproval::Never` reject invocations that cannot be approved.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolAskRule {
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

impl ToolAskRule {
    pub fn new(tool: impl Into<String>) -> Self {
        Self {
            tool: tool.into(),
            command: None,
        }
    }

    pub fn exec_shell(command: impl Into<String>) -> Self {
        Self {
            tool: "exec_shell".to_string(),
            command: Some(command.into()),
        }
    }

    fn label(&self) -> String {
        let mut parts = vec![format!("tool={}", self.tool)];
        if let Some(command) = &self.command {
            parts.push(format!("command={command}"));
        }
        parts.join(" ")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AskForApproval {
    UnlessTrusted,
    OnFailure,
    OnRequest,
    Reject {
        sandbox_approval: bool,
        rules: bool,
        mcp_elicitations: bool,
    },
    Never,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecPolicyAmendment {
    pub prefixes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExecApprovalRequirement {
    Skip {
        bypass_sandbox: bool,
        proposed_execpolicy_amendment: Option<ExecPolicyAmendment>,
    },
    NeedsApproval {
        reason: String,
        proposed_execpolicy_amendment: Option<ExecPolicyAmendment>,
        proposed_network_policy_amendments: Vec<NetworkPolicyAmendment>,
    },
    Forbidden {
        reason: String,
    },
}

impl ExecApprovalRequirement {
    pub fn reason(&self) -> &str {
        match self {
            ExecApprovalRequirement::Skip { .. } => "Execution allowed by policy.",
            ExecApprovalRequirement::NeedsApproval { reason, .. } => reason,
            ExecApprovalRequirement::Forbidden { reason } => reason,
        }
    }

    pub fn phase(&self) -> &'static str {
        match self {
            ExecApprovalRequirement::Skip { .. } => "allowed",
            ExecApprovalRequirement::NeedsApproval { .. } => "needs_approval",
            ExecApprovalRequirement::Forbidden { .. } => "forbidden",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecPolicyDecision {
    pub allow: bool,
    pub requires_approval: bool,
    pub requirement: ExecApprovalRequirement,
    pub matched_rule: Option<String>,
}

impl ExecPolicyDecision {
    pub fn reason(&self) -> &str {
        self.requirement.reason()
    }
}

#[derive(Debug, Clone)]
pub struct ExecPolicyContext<'a> {
    pub command: &'a str,
    pub cwd: &'a str,
    pub tool: Option<&'a str>,
    pub ask_for_approval: AskForApproval,
    pub sandbox_mode: Option<&'a str>,
}

#[derive(Debug, Clone, Default)]
pub struct ExecPolicyEngine {
    /// Layered rulesets (builtin → agent → user). When non-empty, takes precedence
    /// over the legacy flat lists below.
    rulesets: Vec<Ruleset>,
    /// Legacy flat lists kept for backward compatibility with `new()`.
    trusted_prefixes: Vec<String>,
    denied_prefixes: Vec<String>,
    approved_for_session: HashSet<String>,
    /// Arity dictionary for command-prefix allow-rule matching.
    arity_dict: BashArityDict,
}

impl ExecPolicyEngine {
    /// Legacy constructor: wraps the two vecs into a User-layer ruleset.
    pub fn new(trusted_prefixes: Vec<String>, denied_prefixes: Vec<String>) -> Self {
        Self {
            rulesets: vec![],
            trusted_prefixes,
            denied_prefixes,
            approved_for_session: HashSet::new(),
            arity_dict: BashArityDict::new(),
        }
    }

    /// Build an engine from explicit layered rulesets.
    /// Rulesets are sorted by layer priority on construction.
    pub fn with_rulesets(mut rulesets: Vec<Ruleset>) -> Self {
        rulesets.sort_by_key(|r| r.layer);
        Self {
            rulesets,
            trusted_prefixes: vec![],
            denied_prefixes: vec![],
            approved_for_session: HashSet::new(),
            arity_dict: BashArityDict::new(),
        }
    }

    /// Add a ruleset layer (re-sorts internally).
    pub fn add_ruleset(&mut self, ruleset: Ruleset) {
        self.rulesets.push(ruleset);
        self.rulesets.sort_by_key(|r| r.layer);
    }

    /// Resolve the effective trusted/denied prefix sets by merging all rulesets.
    ///
    /// Collects all prefixes from every layer (builtin → agent → user) into flat
    /// trusted/denied lists. The `check()` method then applies deny-always-wins
    /// semantics: any matching deny prefix blocks the command regardless of layer.
    /// Trusted rules are only consulted after deny checks pass.
    fn resolve_prefixes(&self) -> (Vec<String>, Vec<String>) {
        if self.rulesets.is_empty() {
            return (self.trusted_prefixes.clone(), self.denied_prefixes.clone());
        }
        // Collect all trusted/denied across all layers, highest-priority last so they
        // shadow lower-priority entries with the same prefix.
        let mut trusted: Vec<String> = vec![];
        let mut denied: Vec<String> = vec![];
        for rs in &self.rulesets {
            trusted.extend(rs.trusted_prefixes.iter().cloned());
            denied.extend(rs.denied_prefixes.iter().cloned());
        }
        // Also merge legacy flat lists as user-layer.
        trusted.extend(self.trusted_prefixes.iter().cloned());
        denied.extend(self.denied_prefixes.iter().cloned());
        (trusted, denied)
    }

    fn matching_ask_rule(&self, ctx: &ExecPolicyContext<'_>) -> Option<ToolAskRule> {
        let tool = ctx.tool.unwrap_or("exec_shell");

        self.rulesets
            .iter()
            .flat_map(|ruleset| {
                ruleset
                    .ask_rules
                    .iter()
                    .map(move |rule| (ruleset.layer, rule))
            })
            .filter(|(_, rule)| rule.tool == tool)
            .filter(|(_, rule)| match rule.command.as_deref() {
                Some(command) => self.arity_dict.allow_rule_matches(command, ctx.command),
                None => true,
            })
            .max_by_key(|(layer, rule)| (*layer, ask_rule_specificity(rule)))
            .map(|(_, rule)| rule.clone())
    }

    pub fn remember_session_approval(&mut self, approval_key: String) {
        self.approved_for_session.insert(approval_key);
    }

    pub fn is_session_approved(&self, approval_key: &str) -> bool {
        self.approved_for_session.contains(approval_key)
    }

    pub fn check(&self, ctx: ExecPolicyContext<'_>) -> Result<ExecPolicyDecision> {
        let normalized = normalize_command(ctx.command);
        let (trusted_prefixes, denied_prefixes) = self.resolve_prefixes();
        // Deny rules use simple prefix matching (no arity semantics needed).
        if let Some(rule) = denied_prefixes
            .iter()
            .find(|rule| normalized.starts_with(&normalize_command(rule)))
        {
            return Ok(ExecPolicyDecision {
                allow: false,
                requires_approval: false,
                matched_rule: Some(rule.clone()),
                requirement: ExecApprovalRequirement::Forbidden {
                    reason: format!("Command blocked by denied prefix rule '{rule}'"),
                },
            });
        }

        // Allow (trusted) rules use arity-aware prefix matching so that
        // `auto_allow = ["git status"]` matches `git status -s` but NOT
        // `git push origin main`.
        let trusted_rule = trusted_prefixes
            .iter()
            .find(|rule| self.arity_dict.allow_rule_matches(rule, ctx.command))
            .cloned();
        let is_trusted = trusted_rule.is_some();

        let ask_rule = self.matching_ask_rule(&ctx);

        let requirement = match &ctx.ask_for_approval {
            AskForApproval::Never => {
                if let Some(rule) = &ask_rule {
                    ExecApprovalRequirement::Forbidden {
                        reason: format!(
                            "Typed ask rule '{}' requires approval, but approval policy is never.",
                            rule.label()
                        ),
                    }
                } else {
                    ExecApprovalRequirement::Skip {
                        bypass_sandbox: false,
                        proposed_execpolicy_amendment: None,
                    }
                }
            }
            AskForApproval::UnlessTrusted if is_trusted => ExecApprovalRequirement::Skip {
                bypass_sandbox: false,
                proposed_execpolicy_amendment: None,
            },
            AskForApproval::OnFailure => ExecApprovalRequirement::Skip {
                bypass_sandbox: false,
                proposed_execpolicy_amendment: None,
            },
            AskForApproval::Reject { rules, .. } if *rules => ExecApprovalRequirement::Forbidden {
                reason: "Policy is configured to reject rule-exceptions.".to_string(),
            },
            _ => ExecApprovalRequirement::NeedsApproval {
                reason: if is_trusted {
                    "Approval requested by policy mode.".to_string()
                } else {
                    "Unmatched command prefix requires approval.".to_string()
                },
                proposed_execpolicy_amendment: if is_trusted {
                    None
                } else {
                    Some(ExecPolicyAmendment {
                        prefixes: vec![first_token(ctx.command)],
                    })
                },
                proposed_network_policy_amendments: vec![NetworkPolicyAmendment {
                    host: ctx.cwd.to_string(),
                    action: NetworkPolicyRuleAction::Allow,
                }],
            },
        };

        let (allow, requires_approval) = match requirement {
            ExecApprovalRequirement::Skip { .. } => (true, false),
            ExecApprovalRequirement::NeedsApproval { .. } => (true, true),
            ExecApprovalRequirement::Forbidden { .. } => (false, false),
        };

        let matched_ask_rule = if matches!(&ctx.ask_for_approval, AskForApproval::Never) {
            ask_rule.map(|rule| rule.label())
        } else {
            None
        };

        Ok(ExecPolicyDecision {
            allow,
            requires_approval,
            matched_rule: matched_ask_rule.or(trusted_rule),
            requirement,
        })
    }
}

fn normalize_command(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn first_token(command: &str) -> String {
    command
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string()
}

fn ask_rule_specificity(rule: &ToolAskRule) -> usize {
    rule.tool.len()
        + rule
            .command
            .as_ref()
            .map_or(0, |command| command.len() + 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(command: &str, ask_for_approval: AskForApproval) -> ExecPolicyContext<'_> {
        ExecPolicyContext {
            command,
            cwd: "/workspace",
            tool: Some("exec_shell"),
            ask_for_approval,
            sandbox_mode: Some("workspace-write"),
        }
    }

    #[test]
    fn trusted_prefix_skips_approval_when_policy_is_unless_trusted() {
        let engine = ExecPolicyEngine::new(vec!["git status".to_string()], vec![]);

        let decision = engine
            .check(ctx("git status --porcelain", AskForApproval::UnlessTrusted))
            .unwrap();

        assert!(decision.allow);
        assert!(!decision.requires_approval);
        assert_eq!(decision.matched_rule.as_deref(), Some("git status"));
        assert!(matches!(
            decision.requirement,
            ExecApprovalRequirement::Skip {
                bypass_sandbox: false,
                proposed_execpolicy_amendment: None,
            }
        ));
    }

    #[test]
    fn denied_prefix_blocks_even_when_command_is_also_trusted() {
        let engine = ExecPolicyEngine::new(
            vec!["git status".to_string()],
            vec!["git status".to_string()],
        );

        let decision = engine
            .check(ctx("git status --porcelain", AskForApproval::UnlessTrusted))
            .unwrap();

        assert!(!decision.allow);
        assert!(!decision.requires_approval);
        assert_eq!(decision.matched_rule.as_deref(), Some("git status"));
        assert!(matches!(
            decision.requirement,
            ExecApprovalRequirement::Forbidden { .. }
        ));
        assert_eq!(
            decision.reason(),
            "Command blocked by denied prefix rule 'git status'"
        );
    }

    #[test]
    fn unmatched_command_requires_approval_and_proposes_first_token_rule() {
        let engine = ExecPolicyEngine::new(vec![], vec![]);

        let decision = engine
            .check(ctx("cargo test --workspace", AskForApproval::UnlessTrusted))
            .unwrap();

        assert!(decision.allow);
        assert!(decision.requires_approval);
        assert_eq!(decision.matched_rule, None);
        match decision.requirement {
            ExecApprovalRequirement::NeedsApproval {
                proposed_execpolicy_amendment: Some(amendment),
                proposed_network_policy_amendments,
                ..
            } => {
                assert_eq!(amendment.prefixes, vec!["cargo"]);
                assert_eq!(
                    proposed_network_policy_amendments,
                    vec![NetworkPolicyAmendment {
                        host: "/workspace".to_string(),
                        action: NetworkPolicyRuleAction::Allow,
                    }]
                );
            }
            other => panic!("expected approval with proposed amendment, got {other:?}"),
        }
    }

    #[test]
    fn trusted_command_in_on_request_mode_still_requires_approval_without_new_rule() {
        let engine = ExecPolicyEngine::new(vec!["cargo test".to_string()], vec![]);

        let decision = engine
            .check(ctx("cargo test --workspace", AskForApproval::OnRequest))
            .unwrap();

        assert!(decision.allow);
        assert!(decision.requires_approval);
        assert_eq!(decision.matched_rule.as_deref(), Some("cargo test"));
        match decision.requirement {
            ExecApprovalRequirement::NeedsApproval {
                proposed_execpolicy_amendment,
                ..
            } => assert_eq!(proposed_execpolicy_amendment, None),
            other => panic!("expected approval without amendment, got {other:?}"),
        }
    }

    #[test]
    fn reject_rules_mode_forbids_unmatched_command() {
        let engine = ExecPolicyEngine::new(vec![], vec![]);

        let decision = engine
            .check(ctx(
                "npm install",
                AskForApproval::Reject {
                    sandbox_approval: false,
                    rules: true,
                    mcp_elicitations: false,
                },
            ))
            .unwrap();

        assert!(!decision.allow);
        assert!(!decision.requires_approval);
        assert_eq!(decision.matched_rule, None);
        assert_eq!(decision.requirement.phase(), "forbidden");
        assert_eq!(
            decision.reason(),
            "Policy is configured to reject rule-exceptions."
        );
    }

    #[test]
    fn typed_ask_rule_forbids_matching_command_when_policy_is_never() {
        let engine = ExecPolicyEngine::with_rulesets(vec![
            Ruleset::user(vec![], vec![])
                .with_ask_rules(vec![ToolAskRule::exec_shell("cargo test")]),
        ]);

        let decision = engine
            .check(ctx("cargo test --workspace", AskForApproval::Never))
            .unwrap();

        assert!(!decision.allow);
        assert!(!decision.requires_approval);
        assert_eq!(
            decision.matched_rule.as_deref(),
            Some("tool=exec_shell command=cargo test")
        );
        assert_eq!(decision.requirement.phase(), "forbidden");
        assert_eq!(
            decision.reason(),
            "Typed ask rule 'tool=exec_shell command=cargo test' requires approval, but approval policy is never."
        );
    }

    #[test]
    fn typed_ask_rule_is_ignored_outside_never_mode_for_now() {
        let engine = ExecPolicyEngine::with_rulesets(vec![
            Ruleset::user(vec![], vec![])
                .with_ask_rules(vec![ToolAskRule::exec_shell("cargo test")]),
        ]);

        let decision = engine
            .check(ctx("cargo test --workspace", AskForApproval::UnlessTrusted))
            .unwrap();

        assert!(decision.allow);
        assert!(decision.requires_approval);
        assert_eq!(decision.matched_rule, None);
        match decision.requirement {
            ExecApprovalRequirement::NeedsApproval {
                proposed_execpolicy_amendment: Some(amendment),
                ..
            } => assert_eq!(amendment.prefixes, vec!["cargo"]),
            other => panic!("expected unchanged approval behavior, got {other:?}"),
        }
    }

    #[test]
    fn typed_ask_rule_prefers_higher_layer_before_specificity() {
        let engine = ExecPolicyEngine::with_rulesets(vec![
            Ruleset::agent(vec![], vec![])
                .with_ask_rules(vec![ToolAskRule::exec_shell("cargo test --workspace")]),
            Ruleset::user(vec![], vec![])
                .with_ask_rules(vec![ToolAskRule::exec_shell("cargo test")]),
        ]);

        let decision = engine
            .check(ctx(
                "cargo test --workspace --all-features",
                AskForApproval::Never,
            ))
            .unwrap();

        assert!(!decision.allow);
        assert_eq!(
            decision.matched_rule.as_deref(),
            Some("tool=exec_shell command=cargo test")
        );
    }

    #[test]
    fn typed_ask_rule_explains_forbidden_decision_even_when_trusted_prefix_matches() {
        let engine = ExecPolicyEngine::with_rulesets(vec![
            Ruleset::user(vec!["cargo test".to_string()], vec![])
                .with_ask_rules(vec![ToolAskRule::exec_shell("cargo test")]),
        ]);

        let decision = engine
            .check(ctx("cargo test --workspace", AskForApproval::Never))
            .unwrap();

        assert!(!decision.allow);
        assert!(!decision.requires_approval);
        assert_eq!(
            decision.matched_rule.as_deref(),
            Some("tool=exec_shell command=cargo test")
        );
        assert_eq!(decision.requirement.phase(), "forbidden");
    }

    #[test]
    fn typed_ask_rule_does_not_change_allow_deny_precedence() {
        let engine = ExecPolicyEngine::with_rulesets(vec![
            Ruleset::user(
                vec!["cargo test".to_string()],
                vec!["cargo test --danger".to_string()],
            )
            .with_ask_rules(vec![ToolAskRule::exec_shell("cargo test")]),
        ]);

        let trusted = engine
            .check(ctx("cargo test --workspace", AskForApproval::UnlessTrusted))
            .unwrap();
        assert!(trusted.allow);
        assert!(!trusted.requires_approval);
        assert_eq!(trusted.matched_rule.as_deref(), Some("cargo test"));

        let denied = engine
            .check(ctx("cargo test --danger", AskForApproval::Never))
            .unwrap();
        assert!(!denied.allow);
        assert!(!denied.requires_approval);
        assert_eq!(denied.matched_rule.as_deref(), Some("cargo test --danger"));
        assert_eq!(
            denied.reason(),
            "Command blocked by denied prefix rule 'cargo test --danger'"
        );
    }
}
