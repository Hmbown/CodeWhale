pub mod bash_arity;

use std::collections::HashSet;

use anyhow::Result;
use bash_arity::BashArityDict;
use codewhale_protocol::{NetworkPolicyAmendment, NetworkPolicyRuleAction};
use serde::{Deserialize, Serialize};

/// Priority layer for a permission ruleset. Higher ordinal = higher priority.
/// Deny rules still win across layers; for non-deny ties the higher-priority
/// layer wins before pattern specificity is considered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RulesetLayer {
    BuiltinDefault = 0,
    Agent = 1,
    User = 2,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    Allow,
    Deny,
    Ask,
}

impl PermissionDecision {
    fn precedence(self) -> u8 {
        match self {
            Self::Deny => 3,
            Self::Ask => 2,
            Self::Allow => 1,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::Ask => "ask",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolPermissionRule {
    pub tool: String,
    pub decision: PermissionDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl ToolPermissionRule {
    #[must_use]
    pub fn new(tool: impl Into<String>, decision: PermissionDecision) -> Self {
        Self {
            tool: tool.into(),
            decision,
            command: None,
            path: None,
        }
    }

    #[must_use]
    pub fn exec_shell(decision: PermissionDecision, command: impl Into<String>) -> Self {
        Self {
            tool: "exec_shell".to_string(),
            decision,
            command: Some(command.into()),
            path: None,
        }
    }

    #[must_use]
    pub fn file_path(
        tool: impl Into<String>,
        decision: PermissionDecision,
        path: impl Into<String>,
    ) -> Self {
        Self {
            tool: tool.into(),
            decision,
            command: None,
            path: Some(path.into()),
        }
    }

    #[must_use]
    pub fn pattern_label(&self) -> String {
        let mut parts = vec![format!("tool '{}'", self.tool)];
        if let Some(command) = self.command.as_deref() {
            parts.push(format!("command '{command}'"));
        }
        if let Some(path) = self.path.as_deref() {
            parts.push(format!("path '{path}'"));
        }
        parts.join(", ")
    }
}

/// A named set of allow/deny prefix rules at a given priority layer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Ruleset {
    pub layer: RulesetLayer,
    #[serde(default)]
    pub trusted_prefixes: Vec<String>,
    #[serde(default)]
    pub denied_prefixes: Vec<String>,
    #[serde(default)]
    pub rules: Vec<ToolPermissionRule>,
}

impl Ruleset {
    pub fn builtin_default() -> Self {
        Self {
            layer: RulesetLayer::BuiltinDefault,
            trusted_prefixes: vec![],
            denied_prefixes: vec![],
            rules: vec![],
        }
    }

    pub fn agent(trusted: Vec<String>, denied: Vec<String>) -> Self {
        Self {
            layer: RulesetLayer::Agent,
            trusted_prefixes: trusted,
            denied_prefixes: denied,
            rules: vec![],
        }
    }

    pub fn user(trusted: Vec<String>, denied: Vec<String>) -> Self {
        Self {
            layer: RulesetLayer::User,
            trusted_prefixes: trusted,
            denied_prefixes: denied,
            rules: vec![],
        }
    }

    #[must_use]
    pub fn with_rules(mut self, rules: Vec<ToolPermissionRule>) -> Self {
        self.rules = rules;
        self
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

#[derive(Debug, Clone)]
pub struct ToolPermissionContext<'a> {
    pub tool: &'a str,
    pub command: Option<&'a str>,
    pub path: Option<&'a str>,
}

impl<'a> ToolPermissionContext<'a> {
    #[must_use]
    pub fn exec_shell(command: &'a str) -> Self {
        Self {
            tool: "exec_shell",
            command: Some(command),
            path: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MatchedToolPermissionRule {
    pub layer: RulesetLayer,
    pub rule: ToolPermissionRule,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolPermissionCheck {
    pub decision: Option<PermissionDecision>,
    pub matched_rule: Option<MatchedToolPermissionRule>,
}

impl ToolPermissionCheck {
    #[must_use]
    pub fn unmatched() -> Self {
        Self {
            decision: None,
            matched_rule: None,
        }
    }
}

#[derive(Debug, Clone)]
struct LayeredToolPermissionRule {
    layer: RulesetLayer,
    rule: ToolPermissionRule,
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

    fn layered_permission_rules(&self) -> Vec<LayeredToolPermissionRule> {
        let mut rules = Vec::new();
        if self.rulesets.is_empty() {
            rules.extend(legacy_command_rules(
                RulesetLayer::User,
                &self.trusted_prefixes,
                &self.denied_prefixes,
            ));
            return rules;
        }

        for ruleset in &self.rulesets {
            rules.extend(
                ruleset
                    .rules
                    .iter()
                    .cloned()
                    .map(|rule| LayeredToolPermissionRule {
                        layer: ruleset.layer,
                        rule,
                    }),
            );
            rules.extend(legacy_command_rules(
                ruleset.layer,
                &ruleset.trusted_prefixes,
                &ruleset.denied_prefixes,
            ));
        }
        rules.extend(legacy_command_rules(
            RulesetLayer::User,
            &self.trusted_prefixes,
            &self.denied_prefixes,
        ));
        rules
    }

    pub fn remember_session_approval(&mut self, approval_key: String) {
        self.approved_for_session.insert(approval_key);
    }

    pub fn is_session_approved(&self, approval_key: &str) -> bool {
        self.approved_for_session.contains(approval_key)
    }

    #[must_use]
    pub fn check_tool_permission(&self, ctx: ToolPermissionContext<'_>) -> ToolPermissionCheck {
        let mut best: Option<(LayeredToolPermissionRule, usize)> = None;

        for candidate in self.layered_permission_rules() {
            if !tool_rule_matches(&candidate.rule, &ctx, &self.arity_dict) {
                continue;
            }
            let specificity = rule_specificity(&candidate.rule);
            let should_replace = best.as_ref().is_none_or(|(current, current_specificity)| {
                compare_rule_priority(&candidate, specificity, current, *current_specificity)
            });
            if should_replace {
                best = Some((candidate, specificity));
            }
        }

        match best {
            Some((matched, _)) => ToolPermissionCheck {
                decision: Some(matched.rule.decision),
                matched_rule: Some(MatchedToolPermissionRule {
                    layer: matched.layer,
                    rule: matched.rule,
                }),
            },
            None => ToolPermissionCheck::unmatched(),
        }
    }

    pub fn check(&self, ctx: ExecPolicyContext<'_>) -> Result<ExecPolicyDecision> {
        let tool_rule = self.check_tool_permission(ToolPermissionContext::exec_shell(ctx.command));
        if let Some(matched) = tool_rule
            .matched_rule
            .as_ref()
            .filter(|matched| matched.rule.decision == PermissionDecision::Deny)
        {
            return Ok(ExecPolicyDecision {
                allow: false,
                requires_approval: false,
                matched_rule: Some(matched.rule.pattern_label()),
                requirement: ExecApprovalRequirement::Forbidden {
                    reason: format!(
                        "Command blocked by {} rule ({})",
                        matched.rule.decision.as_str(),
                        matched.rule.pattern_label()
                    ),
                },
            });
        }

        let requirement = match ctx.ask_for_approval {
            AskForApproval::Never => ExecApprovalRequirement::Skip {
                bypass_sandbox: false,
                proposed_execpolicy_amendment: None,
            },
            AskForApproval::Reject { rules, .. } if rules => ExecApprovalRequirement::Forbidden {
                reason: "Policy is configured to reject rule-exceptions.".to_string(),
            },
            _ if matches!(tool_rule.decision, Some(PermissionDecision::Ask)) => {
                ExecApprovalRequirement::NeedsApproval {
                    reason: tool_rule
                        .matched_rule
                        .as_ref()
                        .map(|matched| {
                            format!(
                                "Approval required by ask rule ({})",
                                matched.rule.pattern_label()
                            )
                        })
                        .unwrap_or_else(|| "Approval required by ask rule.".to_string()),
                    proposed_execpolicy_amendment: None,
                    proposed_network_policy_amendments: vec![],
                }
            }
            _ if matches!(tool_rule.decision, Some(PermissionDecision::Allow)) => {
                ExecApprovalRequirement::Skip {
                    bypass_sandbox: false,
                    proposed_execpolicy_amendment: None,
                }
            }
            AskForApproval::OnFailure => ExecApprovalRequirement::Skip {
                bypass_sandbox: false,
                proposed_execpolicy_amendment: None,
            },
            _ => ExecApprovalRequirement::NeedsApproval {
                reason: "Unmatched command prefix requires approval.".to_string(),
                proposed_execpolicy_amendment: Some(ExecPolicyAmendment {
                    prefixes: vec![first_token(ctx.command)],
                }),
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
            matched_rule: tool_rule
                .matched_rule
                .as_ref()
                .map(|matched| matched.rule.pattern_label()),
            requirement,
        })
    }
}

fn legacy_command_rules(
    layer: RulesetLayer,
    trusted_prefixes: &[String],
    denied_prefixes: &[String],
) -> Vec<LayeredToolPermissionRule> {
    trusted_prefixes
        .iter()
        .map(|command| LayeredToolPermissionRule {
            layer,
            rule: ToolPermissionRule::exec_shell(PermissionDecision::Allow, command.clone()),
        })
        .chain(
            denied_prefixes
                .iter()
                .map(|command| LayeredToolPermissionRule {
                    layer,
                    rule: ToolPermissionRule::exec_shell(PermissionDecision::Deny, command.clone()),
                }),
        )
        .collect()
}

fn compare_rule_priority(
    candidate: &LayeredToolPermissionRule,
    candidate_specificity: usize,
    current: &LayeredToolPermissionRule,
    current_specificity: usize,
) -> bool {
    (
        candidate.rule.decision.precedence(),
        candidate.layer,
        candidate_specificity,
    ) > (
        current.rule.decision.precedence(),
        current.layer,
        current_specificity,
    )
}

fn tool_rule_matches(
    rule: &ToolPermissionRule,
    ctx: &ToolPermissionContext<'_>,
    arity_dict: &BashArityDict,
) -> bool {
    if !rule.tool.eq_ignore_ascii_case(ctx.tool) {
        return false;
    }
    if let Some(command_rule) = rule.command.as_deref() {
        let Some(command) = ctx.command else {
            return false;
        };
        if !command_rule_matches(rule.decision, command_rule, command, arity_dict) {
            return false;
        }
    }
    if let Some(path_rule) = rule.path.as_deref() {
        let Some(path) = ctx.path else {
            return false;
        };
        if !path_pattern_matches(path_rule, path) {
            return false;
        }
    }
    true
}

fn command_rule_matches(
    decision: PermissionDecision,
    pattern: &str,
    command: &str,
    arity_dict: &BashArityDict,
) -> bool {
    match decision {
        PermissionDecision::Deny => {
            normalize_command(command).starts_with(&normalize_command(pattern))
        }
        PermissionDecision::Allow | PermissionDecision::Ask => {
            arity_dict.allow_rule_matches(pattern, command)
        }
    }
}

fn path_pattern_matches(pattern: &str, path: &str) -> bool {
    let pattern = normalize_path_pattern(pattern);
    let path = normalize_path_pattern(path);
    if let Some(prefix) = pattern.strip_suffix("/**")
        && (path == prefix || path.starts_with(&format!("{prefix}/")))
    {
        return true;
    }
    if !pattern.contains('*') && !pattern.contains('?') {
        return pattern == path;
    }
    glob_match(pattern.as_bytes(), path.as_bytes())
}

fn glob_match(pattern: &[u8], path: &[u8]) -> bool {
    if pattern.is_empty() {
        return path.is_empty();
    }

    match pattern[0] {
        b'*' if pattern.get(1) == Some(&b'*') => {
            let rest = &pattern[2..];
            (0..=path.len()).any(|idx| glob_match(rest, &path[idx..]))
        }
        b'*' => {
            if glob_match(&pattern[1..], path) {
                return true;
            }
            let mut idx = 0;
            while idx < path.len() && path[idx] != b'/' {
                idx += 1;
                if glob_match(&pattern[1..], &path[idx..]) {
                    return true;
                }
            }
            false
        }
        b'?' => !path.is_empty() && path[0] != b'/' && glob_match(&pattern[1..], &path[1..]),
        literal => !path.is_empty() && path[0] == literal && glob_match(&pattern[1..], &path[1..]),
    }
}

fn rule_specificity(rule: &ToolPermissionRule) -> usize {
    let mut score = rule.tool.len();
    if let Some(command) = rule.command.as_deref() {
        score += 1_000 + non_wildcard_len(command);
    }
    if let Some(path) = rule.path.as_deref() {
        score += 1_000 + non_wildcard_len(path);
    }
    score
}

fn non_wildcard_len(value: &str) -> usize {
    value.chars().filter(|ch| *ch != '*' && *ch != '?').count()
}

fn normalize_command(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_path_pattern(value: &str) -> String {
    let raw = value.trim().replace('\\', "/");
    let absolute = raw.starts_with('/');
    let mut segments = Vec::new();

    for segment in raw.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.is_empty() || segments.last() == Some(&"..") {
                    segments.push(segment);
                } else {
                    segments.pop();
                }
            }
            _ => segments.push(segment),
        }
    }

    let normalized = segments.join("/");
    if absolute && normalized.is_empty() {
        "/".to_string()
    } else if absolute {
        format!("/{normalized}")
    } else {
        normalized
    }
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

    fn exec_ctx(command: &str) -> ExecPolicyContext<'_> {
        ctx(command, AskForApproval::UnlessTrusted)
    }

    #[test]
    fn trusted_prefix_skips_approval_when_policy_is_unless_trusted() {
        let engine = ExecPolicyEngine::new(vec!["git status".to_string()], vec![]);

        let decision = engine
            .check(ctx("git status --porcelain", AskForApproval::UnlessTrusted))
            .unwrap();

        assert!(decision.allow);
        assert!(!decision.requires_approval);
        assert_eq!(
            decision.matched_rule.as_deref(),
            Some("tool 'exec_shell', command 'git status'")
        );
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
        assert_eq!(
            decision.matched_rule.as_deref(),
            Some("tool 'exec_shell', command 'git status'")
        );
        assert!(matches!(
            decision.requirement,
            ExecApprovalRequirement::Forbidden { .. }
        ));
        assert_eq!(
            decision.reason(),
            "Command blocked by deny rule (tool 'exec_shell', command 'git status')"
        );
    }

    #[test]
    fn legacy_auto_allow_prefixes_become_exec_shell_rules() {
        let engine = ExecPolicyEngine::new(vec!["git status".to_string()], vec![]);

        let decision = engine.check(exec_ctx("git status --porcelain")).unwrap();

        assert!(decision.allow);
        assert!(!decision.requires_approval);
        assert_eq!(
            decision.matched_rule.as_deref(),
            Some("tool 'exec_shell', command 'git status'")
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
        assert_eq!(
            decision.matched_rule.as_deref(),
            Some("tool 'exec_shell', command 'cargo test'")
        );
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
    fn legacy_auto_allow_uses_bash_arity() {
        let engine = ExecPolicyEngine::new(vec!["git status".to_string()], vec![]);

        let decision = engine.check(exec_ctx("git push origin main")).unwrap();

        assert!(decision.requires_approval);
        assert_eq!(decision.matched_rule, None);
    }

    #[test]
    fn deny_rule_wins_over_user_allow_rule() {
        let engine = ExecPolicyEngine::with_rulesets(vec![
            Ruleset::builtin_default().with_rules(vec![ToolPermissionRule::exec_shell(
                PermissionDecision::Deny,
                "cargo",
            )]),
            Ruleset::user(vec![], vec![]).with_rules(vec![ToolPermissionRule::exec_shell(
                PermissionDecision::Allow,
                "cargo test",
            )]),
        ]);

        let decision = engine.check(exec_ctx("cargo test --workspace")).unwrap();

        assert!(!decision.allow);
        assert_eq!(decision.requirement.phase(), "forbidden");
        assert_eq!(
            decision.matched_rule.as_deref(),
            Some("tool 'exec_shell', command 'cargo'")
        );
    }

    #[test]
    fn ask_rule_wins_over_allow_rule() {
        let engine =
            ExecPolicyEngine::with_rulesets(vec![Ruleset::user(vec![], vec![]).with_rules(vec![
                ToolPermissionRule::exec_shell(PermissionDecision::Allow, "cargo"),
                ToolPermissionRule::exec_shell(PermissionDecision::Ask, "cargo test"),
            ])]);

        let decision = engine.check(exec_ctx("cargo test --workspace")).unwrap();

        assert!(decision.allow);
        assert!(decision.requires_approval);
        assert_eq!(decision.requirement.phase(), "needs_approval");
        assert_eq!(
            decision.matched_rule.as_deref(),
            Some("tool 'exec_shell', command 'cargo test'")
        );
    }

    #[test]
    fn ask_rule_overrides_on_failure_policy() {
        let engine =
            ExecPolicyEngine::with_rulesets(vec![Ruleset::user(vec![], vec![]).with_rules(vec![
                ToolPermissionRule::exec_shell(PermissionDecision::Ask, "cargo test"),
            ])]);

        let decision = engine
            .check(ExecPolicyContext {
                command: "cargo test --workspace",
                cwd: ".",
                ask_for_approval: AskForApproval::OnFailure,
                sandbox_mode: Some("workspace-write"),
            })
            .unwrap();

        assert!(decision.allow);
        assert!(decision.requires_approval);
        assert_eq!(decision.requirement.phase(), "needs_approval");
    }

    #[test]
    fn path_rules_match_workspace_relative_globs() {
        let engine =
            ExecPolicyEngine::with_rulesets(vec![Ruleset::user(vec![], vec![]).with_rules(vec![
                ToolPermissionRule::file_path("file_edit", PermissionDecision::Allow, "docs/**"),
            ])]);

        let decision = engine.check_tool_permission(ToolPermissionContext {
            tool: "file_edit",
            command: None,
            path: Some("./docs/guide/setup.md"),
        });

        assert_eq!(decision.decision, Some(PermissionDecision::Allow));
    }

    #[test]
    fn path_star_does_not_cross_directory_separator() {
        assert!(path_pattern_matches("docs/*.md", "docs/readme.md"));
        assert!(!path_pattern_matches("docs/*.md", "docs/guides/readme.md"));
    }

    #[test]
    fn path_rules_normalize_dot_segments() {
        assert!(path_pattern_matches("src/**", "src/./lib.rs"));
        assert!(!path_pattern_matches("src/**", "src/../secret.txt"));
        assert!(!path_pattern_matches("src/**", "../src/lib.rs"));
    }

    #[test]
    fn tool_name_only_rule_matches_unknown_tool() {
        let engine =
            ExecPolicyEngine::with_rulesets(vec![Ruleset::user(vec![], vec![]).with_rules(vec![
                ToolPermissionRule::new("agent_spawn", PermissionDecision::Ask),
            ])]);

        let decision = engine.check_tool_permission(ToolPermissionContext {
            tool: "agent_spawn",
            command: None,
            path: None,
        });

        assert_eq!(decision.decision, Some(PermissionDecision::Ask));
    }
}
