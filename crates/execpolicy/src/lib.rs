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
    /// Priority layer this ruleset belongs to.
    pub layer: RulesetLayer,
    /// Command prefixes that are allowed without requiring approval.
    pub trusted_prefixes: Vec<String>,
    /// Command prefixes that are always blocked, regardless of trust rules.
    pub denied_prefixes: Vec<String>,
    /// Typed rules that mark specific tool invocations as requiring approval.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ask_rules: Vec<ToolAskRule>,
}

impl Ruleset {
    /// Creates an empty ruleset at the builtin default priority layer.
    pub fn builtin_default() -> Self {
        Self {
            layer: RulesetLayer::BuiltinDefault,
            trusted_prefixes: vec![],
            denied_prefixes: vec![],
            ask_rules: vec![],
        }
    }

    /// Creates an agent-layer ruleset with the given trusted and denied prefixes.
    pub fn agent(trusted: Vec<String>, denied: Vec<String>) -> Self {
        Self {
            layer: RulesetLayer::Agent,
            trusted_prefixes: trusted,
            denied_prefixes: denied,
            ask_rules: vec![],
        }
    }

    /// Creates a user-layer ruleset with the given trusted and denied prefixes.
    pub fn user(trusted: Vec<String>, denied: Vec<String>) -> Self {
        Self {
            layer: RulesetLayer::User,
            trusted_prefixes: trusted,
            denied_prefixes: denied,
            ask_rules: vec![],
        }
    }

    /// Attaches typed ask rules to this ruleset and returns it.
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
#[serde(deny_unknown_fields)]
pub struct ToolAskRule {
    /// Name of the tool this rule applies to (e.g. `"exec_shell"`, `"edit_file"`).
    pub tool: String,
    /// Optional command prefix to match against (uses arity-aware matching).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Optional file path pattern to match against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl ToolAskRule {
    /// Creates a new ask rule matching any invocation of the given tool.
    pub fn new(tool: impl Into<String>) -> Self {
        Self {
            tool: tool.into(),
            command: None,
            path: None,
        }
    }

    /// Creates an ask rule for `exec_shell` matching a specific command prefix.
    pub fn exec_shell(command: impl Into<String>) -> Self {
        Self {
            tool: "exec_shell".to_string(),
            command: Some(command.into()),
            path: None,
        }
    }

    /// Creates an ask rule for a file-tool matching a specific path pattern.
    pub fn file_path(tool: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            tool: tool.into(),
            command: None,
            path: Some(path.into()),
        }
    }

    fn label(&self) -> String {
        let mut parts = vec![format!("tool={}", self.tool)];
        if let Some(command) = &self.command {
            parts.push(format!("command={command}"));
        }
        if let Some(path) = &self.path {
            parts.push(format!("path={path}"));
        }
        parts.join(" ")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
/// Policy mode controlling when tool invocations require human approval.
pub enum AskForApproval {
    /// Skip approval if the command matches a trusted prefix; otherwise require it.
    UnlessTrusted,
    /// Allow execution and only request approval after a failure occurs.
    OnFailure,
    /// Always require approval before execution.
    OnRequest,
    /// Reject invocations outright based on specific criteria.
    Reject {
        /// Whether sandbox approval requests are rejected.
        sandbox_approval: bool,
        /// Whether rule-exception requests are rejected.
        rules: bool,
        /// Whether MCP elicitation requests are rejected.
        mcp_elicitations: bool,
    },
    /// Never require approval; forbid commands that would need it.
    Never,
}

/// A proposed amendment to the execution policy, suggesting new trusted prefixes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecPolicyAmendment {
    /// Command prefixes to add to the trusted list.
    pub prefixes: Vec<String>,
}

/// The approval requirement determined by the execution policy engine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExecApprovalRequirement {
    /// Execution is allowed without approval.
    Skip {
        /// Whether the sandbox should be bypassed for this execution.
        bypass_sandbox: bool,
        /// Optional proposed policy amendment (e.g., to persist the allowed prefix).
        proposed_execpolicy_amendment: Option<ExecPolicyAmendment>,
    },
    /// Execution is allowed but requires human approval first.
    NeedsApproval {
        /// Human-readable reason explaining why approval is needed.
        reason: String,
        /// Optional proposed policy amendment that would be applied on approval.
        proposed_execpolicy_amendment: Option<ExecPolicyAmendment>,
        /// Proposed network policy amendments that would be applied on approval.
        proposed_network_policy_amendments: Vec<NetworkPolicyAmendment>,
    },
    /// Execution is forbidden by policy.
    Forbidden {
        /// Human-readable reason explaining why execution is forbidden.
        reason: String,
    },
}

impl ExecApprovalRequirement {
    /// Returns the human-readable reason for this approval requirement.
    pub fn reason(&self) -> &str {
        match self {
            ExecApprovalRequirement::Skip { .. } => "Execution allowed by policy.",
            ExecApprovalRequirement::NeedsApproval { reason, .. } => reason,
            ExecApprovalRequirement::Forbidden { reason } => reason,
        }
    }

    /// Returns a short phase label: `"allowed"`, `"needs_approval"`, or `"forbidden"`.
    pub fn phase(&self) -> &'static str {
        match self {
            ExecApprovalRequirement::Skip { .. } => "allowed",
            ExecApprovalRequirement::NeedsApproval { .. } => "needs_approval",
            ExecApprovalRequirement::Forbidden { .. } => "forbidden",
        }
    }
}

/// The result of evaluating a command against the execution policy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecPolicyDecision {
    /// Whether the command is allowed to execute.
    pub allow: bool,
    /// Whether human approval is required before execution.
    pub requires_approval: bool,
    /// The detailed approval requirement, including any proposed amendments.
    pub requirement: ExecApprovalRequirement,
    /// The rule that matched, if any (e.g. a trusted prefix or ask rule label).
    pub matched_rule: Option<String>,
}

impl ExecPolicyDecision {
    /// Returns the human-readable reason for this decision.
    pub fn reason(&self) -> &str {
        self.requirement.reason()
    }
}

/// Input context provided to the execution policy engine for a single check.
#[derive(Debug, Clone)]
pub struct ExecPolicyContext<'a> {
    /// The shell command string being evaluated.
    pub command: &'a str,
    /// The current working directory at invocation time.
    pub cwd: &'a str,
    /// The tool name (e.g. `"exec_shell"`, `"edit_file"`). Defaults to `"exec_shell"` when `None`.
    pub tool: Option<&'a str>,
    /// An optional file path relevant to the invocation (used for path-based ask rules).
    pub path: Option<&'a str>,
    /// The current approval policy mode.
    pub ask_for_approval: AskForApproval,
    /// The sandbox mode in effect, if any (e.g. `"workspace-write"`).
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
            .flat_map(|ruleset| ruleset.ask_rules.iter())
            .filter(|rule| rule.tool == tool)
            .filter(|rule| match rule.command.as_deref() {
                Some(command) => self.arity_dict.allow_rule_matches(command, ctx.command),
                None => true,
            })
            .filter(|rule| match (rule.path.as_deref(), ctx.path) {
                (Some(pattern), Some(path)) => {
                    normalize_path_value(pattern) == normalize_path_value(path)
                }
                (Some(_), None) => false,
                (None, _) => true,
            })
            .max_by_key(|rule| ask_rule_specificity(rule))
            .cloned()
    }

    /// Records an approval key for the current session so subsequent checks skip approval.
    pub fn remember_session_approval(&mut self, approval_key: String) {
        self.approved_for_session.insert(approval_key);
    }

    /// Returns whether the given approval key has been recorded for this session.
    pub fn is_session_approved(&self, approval_key: &str) -> bool {
        self.approved_for_session.contains(approval_key)
    }

    /// Evaluates a command against the policy and returns a decision.
    ///
    /// The evaluation order is: deny rules first (always win), then trusted prefix
    /// matching (arity-aware), then typed ask rules, and finally the approval mode.
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

fn normalize_path_value(value: &str) -> String {
    value
        .replace('\\', "/")
        .trim()
        .trim_matches('/')
        .to_ascii_lowercase()
}

fn ask_rule_specificity(rule: &ToolAskRule) -> usize {
    rule.tool.len()
        + rule
            .command
            .as_ref()
            .map_or(0, |command| command.len() + 1000)
        + rule.path.as_ref().map_or(0, |path| path.len() + 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(command: &str, ask_for_approval: AskForApproval) -> ExecPolicyContext<'_> {
        ExecPolicyContext {
            command,
            cwd: "/workspace",
            tool: Some("exec_shell"),
            path: None,
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

    #[test]
    fn typed_ask_rule_label_wins_when_never_blocks_trusted_command() {
        let engine = ExecPolicyEngine::with_rulesets(vec![
            Ruleset::user(vec!["cargo test".to_string()], vec![])
                .with_ask_rules(vec![ToolAskRule::exec_shell("cargo test")]),
        ]);

        let decision = engine
            .check(ctx("cargo test --workspace", AskForApproval::Never))
            .unwrap();

        assert!(!decision.allow);
        assert_eq!(
            decision.matched_rule.as_deref(),
            Some("tool=exec_shell command=cargo test")
        );
        assert_eq!(
            decision.reason(),
            "Typed ask rule 'tool=exec_shell command=cargo test' requires approval, but approval policy is never."
        );
    }

    #[test]
    fn typed_ask_path_matching_trims_spaces_before_boundary_slashes() {
        let engine = ExecPolicyEngine::with_rulesets(vec![
            Ruleset::user(vec![], vec![])
                .with_ask_rules(vec![ToolAskRule::file_path("edit_file", " /TMP/PROJECT/ ")]),
        ]);

        let decision = engine
            .check(ExecPolicyContext {
                command: "",
                cwd: "/workspace",
                tool: Some("edit_file"),
                path: Some("tmp/project"),
                ask_for_approval: AskForApproval::Never,
                sandbox_mode: Some("workspace-write"),
            })
            .unwrap();

        assert!(!decision.allow);
        assert_eq!(
            decision.matched_rule.as_deref(),
            Some("tool=edit_file path= /TMP/PROJECT/ ")
        );
    }
}
