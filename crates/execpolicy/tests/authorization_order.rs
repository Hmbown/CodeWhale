use codewhale_execpolicy::{
    AskForApproval, ExecPolicyContext, ExecPolicyEngine, PermissionAction, Ruleset, ToolAskRule,
};

struct AuthorizationCase {
    name: &'static str,
    engine: ExecPolicyEngine,
    command: &'static str,
    approval: AskForApproval,
    expected_allow: bool,
    expected_approval: bool,
    expected_phase: &'static str,
    expected_action: Option<PermissionAction>,
    expected_rule: Option<&'static str>,
}

fn command_rule(command: &str, action: PermissionAction) -> ToolAskRule {
    ToolAskRule {
        action,
        ..ToolAskRule::exec_shell(command)
    }
}

fn tool_rule(action: PermissionAction) -> ToolAskRule {
    ToolAskRule {
        action,
        ..ToolAskRule::new("exec_shell")
    }
}

#[test]
fn authorization_order_contract_matches_documented_precedence() {
    let cases = vec![
        AuthorizationCase {
            name: "hard denied prefix beats typed allow",
            engine: ExecPolicyEngine::with_rulesets(vec![
                Ruleset::user(vec![], vec!["cargo publish".to_string()])
                    .with_ask_rules(vec![command_rule("cargo publish", PermissionAction::Allow)]),
            ]),
            command: "cargo publish --dry-run",
            approval: AskForApproval::OnRequest,
            expected_allow: false,
            expected_approval: false,
            expected_phase: "forbidden",
            expected_action: None,
            expected_rule: Some("cargo publish"),
        },
        AuthorizationCase {
            name: "higher typed layer beats lower action and specificity",
            engine: ExecPolicyEngine::with_rulesets(vec![
                Ruleset::agent(vec![], vec![]).with_ask_rules(vec![command_rule(
                    "cargo test --workspace",
                    PermissionAction::Deny,
                )]),
                Ruleset::user(vec![], vec![])
                    .with_ask_rules(vec![command_rule("cargo test", PermissionAction::Allow)]),
            ]),
            command: "cargo test --workspace",
            approval: AskForApproval::OnRequest,
            expected_allow: true,
            expected_approval: false,
            expected_phase: "allowed",
            expected_action: Some(PermissionAction::Allow),
            expected_rule: Some("tool=exec_shell command=cargo test"),
        },
        AuthorizationCase {
            name: "typed action beats specificity inside one layer",
            engine: ExecPolicyEngine::with_rulesets(vec![
                Ruleset::user(vec![], vec![]).with_ask_rules(vec![
                    tool_rule(PermissionAction::Deny),
                    command_rule("cargo test --workspace", PermissionAction::Allow),
                ]),
            ]),
            command: "cargo test --workspace",
            approval: AskForApproval::OnRequest,
            expected_allow: false,
            expected_approval: false,
            expected_phase: "forbidden",
            expected_action: Some(PermissionAction::Deny),
            expected_rule: Some("tool=exec_shell"),
        },
        AuthorizationCase {
            name: "specificity breaks a same-layer same-action tie",
            engine: ExecPolicyEngine::with_rulesets(vec![
                Ruleset::user(vec![], vec![]).with_ask_rules(vec![
                    tool_rule(PermissionAction::Allow),
                    command_rule("cargo test", PermissionAction::Allow),
                ]),
            ]),
            command: "cargo test --workspace",
            approval: AskForApproval::OnRequest,
            expected_allow: true,
            expected_approval: false,
            expected_phase: "allowed",
            expected_action: Some(PermissionAction::Allow),
            expected_rule: Some("tool=exec_shell command=cargo test"),
        },
        AuthorizationCase {
            name: "typed ask beats a trusted prefix",
            engine: ExecPolicyEngine::with_rulesets(vec![
                Ruleset::user(vec!["cargo test".to_string()], vec![])
                    .with_ask_rules(vec![command_rule("cargo test", PermissionAction::Ask)]),
            ]),
            command: "cargo test --workspace",
            approval: AskForApproval::UnlessTrusted,
            expected_allow: true,
            expected_approval: true,
            expected_phase: "needs_approval",
            expected_action: Some(PermissionAction::Ask),
            expected_rule: Some("tool=exec_shell command=cargo test"),
        },
        AuthorizationCase {
            name: "typed ask fails closed when prompts are forbidden",
            engine: ExecPolicyEngine::with_rulesets(vec![
                Ruleset::user(vec![], vec![])
                    .with_ask_rules(vec![command_rule("cargo test", PermissionAction::Ask)]),
            ]),
            command: "cargo test --workspace",
            approval: AskForApproval::Never,
            expected_allow: false,
            expected_approval: false,
            expected_phase: "forbidden",
            expected_action: Some(PermissionAction::Ask),
            expected_rule: Some("tool=exec_shell command=cargo test"),
        },
        AuthorizationCase {
            name: "trusted prefix feeds the approval fallback",
            engine: ExecPolicyEngine::with_rulesets(vec![Ruleset::user(
                vec!["cargo test".to_string()],
                vec![],
            )]),
            command: "cargo test --workspace",
            approval: AskForApproval::UnlessTrusted,
            expected_allow: true,
            expected_approval: false,
            expected_phase: "allowed",
            expected_action: None,
            expected_rule: Some("cargo test"),
        },
    ];

    for case in cases {
        let decision = case
            .engine
            .check(ExecPolicyContext {
                command: case.command,
                cwd: "/workspace",
                tool: Some("exec_shell"),
                path: None,
                ask_for_approval: case.approval,
                sandbox_mode: Some("workspace-write"),
            })
            .unwrap_or_else(|error| panic!("{}: policy check failed: {error}", case.name));

        assert_eq!(decision.allow, case.expected_allow, "{}: allow", case.name);
        assert_eq!(
            decision.requires_approval, case.expected_approval,
            "{}: requires_approval",
            case.name
        );
        assert_eq!(
            decision.requirement.phase(),
            case.expected_phase,
            "{}: phase",
            case.name
        );
        assert_eq!(
            decision.matched_action, case.expected_action,
            "{}: matched action",
            case.name
        );
        assert_eq!(
            decision.matched_rule.as_deref(),
            case.expected_rule,
            "{}: matched rule",
            case.name
        );
    }
}
