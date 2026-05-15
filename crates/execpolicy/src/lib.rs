pub mod bash_arity;

use std::collections::HashSet;

use anyhow::Result;
use bash_arity::BashArityDict;
use deepseek_protocol::{NetworkPolicyAmendment, NetworkPolicyRuleAction};
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
}

impl Ruleset {
    pub fn builtin_default() -> Self {
        Self {
            layer: RulesetLayer::BuiltinDefault,
            trusted_prefixes: vec![],
            denied_prefixes: vec![],
        }
    }

    pub fn agent(trusted: Vec<String>, denied: Vec<String>) -> Self {
        Self {
            layer: RulesetLayer::Agent,
            trusted_prefixes: trusted,
            denied_prefixes: denied,
        }
    }

    pub fn user(trusted: Vec<String>, denied: Vec<String>) -> Self {
        Self {
            layer: RulesetLayer::User,
            trusted_prefixes: trusted,
            denied_prefixes: denied,
        }
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

        let requirement = match ctx.ask_for_approval {
            AskForApproval::Never => ExecApprovalRequirement::Skip {
                bypass_sandbox: false,
                proposed_execpolicy_amendment: None,
            },
            AskForApproval::UnlessTrusted if is_trusted => ExecApprovalRequirement::Skip {
                bypass_sandbox: false,
                proposed_execpolicy_amendment: None,
            },
            AskForApproval::OnFailure => ExecApprovalRequirement::Skip {
                bypass_sandbox: false,
                proposed_execpolicy_amendment: None,
            },
            AskForApproval::Reject { rules, .. } if rules => ExecApprovalRequirement::Forbidden {
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

        Ok(ExecPolicyDecision {
            allow,
            requires_approval,
            matched_rule: trusted_rule,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(command: &str, ask_for_approval: AskForApproval) -> ExecPolicyContext<'_> {
        ExecPolicyContext {
            command,
            cwd: "/workspace",
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
}
