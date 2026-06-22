use super::*;

use super::context::{COMPACTION_SUMMARY_MARKER, TURN_MAX_OUTPUT_TOKENS};
use crate::config::ApiProvider;
use crate::models::SystemBlock;
use crate::test_support::lock_test_env;
use crate::tools::plan::{PlanItemArg, PlanSnapshot, StepStatus};
use crate::tools::spec::ToolCapability;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tempfile::tempdir;

const WORKING_SET_SUMMARY_MARKER: &str = "## Repo Working Set";

#[test]
fn subagent_mailbox_keeps_lifecycle_events_reliable() {
    use crate::models::Usage;
    use crate::tools::subagent::MailboxMessage;

    assert!(subagent_mailbox_message_is_best_effort(
        &MailboxMessage::progress("agent_a", "step 1")
    ));
    assert!(subagent_mailbox_message_is_best_effort(
        &MailboxMessage::ToolCallStarted {
            agent_id: "agent_a".to_string(),
            tool_name: "read_file".to_string(),
            step: 1,
        }
    ));
    assert!(subagent_mailbox_message_is_best_effort(
        &MailboxMessage::ToolCallCompleted {
            agent_id: "agent_a".to_string(),
            tool_name: "read_file".to_string(),
            step: 1,
            ok: true,
        }
    ));

    assert!(!subagent_mailbox_message_is_best_effort(
        &MailboxMessage::started("agent_a", crate::tools::subagent::SubAgentType::Explore)
    ));
    assert!(!subagent_mailbox_message_is_best_effort(
        &MailboxMessage::Completed {
            agent_id: "agent_a".to_string(),
            summary: "done".to_string(),
        }
    ));
    assert!(!subagent_mailbox_message_is_best_effort(
        &MailboxMessage::Failed {
            agent_id: "agent_a".to_string(),
            error: "failed".to_string(),
        }
    ));
    assert!(!subagent_mailbox_message_is_best_effort(
        &MailboxMessage::TokenUsage {
            agent_id: "agent_a".to_string(),
            model: "model".to_string(),
            usage: Usage::default(),
        }
    ));
}

struct ScopedDeepSeekApiKey {
    previous: Option<OsString>,
}

impl ScopedDeepSeekApiKey {
    fn set(value: &str) -> Self {
        let previous = std::env::var_os("DEEPSEEK_API_KEY");
        // Safety: tests using this helper serialize with lock_test_env() and
        // restore the original value in Drop.
        unsafe {
            std::env::set_var("DEEPSEEK_API_KEY", value);
        }
        Self { previous }
    }
}

impl Drop for ScopedDeepSeekApiKey {
    fn drop(&mut self) {
        // Safety: tests using this helper serialize with lock_test_env().
        unsafe {
            if let Some(previous) = self.previous.take() {
                std::env::set_var("DEEPSEEK_API_KEY", previous);
            } else {
                std::env::remove_var("DEEPSEEK_API_KEY");
            }
        }
    }
}

fn catalog_tool(name: &str) -> Tool {
    Tool {
        tool_type: None,
        name: name.to_string(),
        description: String::new(),
        input_schema: json!({"type": "object"}),
        allowed_callers: None,
        defer_loading: None,
        input_examples: None,
        strict: None,
        cache_control: None,
    }
}

#[test]
fn tool_catalog_filter_applies_allow_and_deny_gates() {
    // #3027 AC1: the advertised catalog must not contain tools the execution
    // gates would deny; deny wins over allow.
    let mut catalog = vec![
        catalog_tool("read_file"),
        catalog_tool("exec_shell"),
        catalog_tool("grep_files"),
    ];
    filter_tool_catalog_for_gates(
        &mut catalog,
        Some(&["read_file".to_string(), "exec_shell".to_string()][..]),
        Some(&["exec_shell".to_string()][..]),
    );
    let names: Vec<&str> = catalog.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["read_file"]);
}

#[test]
fn tool_catalog_filter_is_inert_without_gates() {
    let mut catalog = vec![catalog_tool("read_file"), catalog_tool("exec_shell")];
    filter_tool_catalog_for_gates(&mut catalog, None, None);
    assert_eq!(catalog.len(), 2);
}

#[test]
fn structured_state_block_includes_rich_plan_artifact() {
    let state = StructuredState {
        mode_label: "Plan".to_string(),
        workspace: PathBuf::from("/workspace/codewhale"),
        cwd: None,
        working_set_summary: None,
        todo_snapshot: None,
        plan_snapshot: Some(PlanSnapshot {
            objective: Some("Make Plan mode reviewable".to_string()),
            context_summary: Some("Grounded in issue #2691".to_string()),
            sources_used: vec!["gh issue view 2691".to_string()],
            critical_files: vec!["crates/tui/src/tools/plan.rs".to_string()],
            constraints: vec!["Preserve legacy payloads".to_string()],
            recommended_approach: Some("Enrich update_plan".to_string()),
            verification_plan: Some("Run focused tests".to_string()),
            risks_and_unknowns: Some("Replay may drift".to_string()),
            handoff_packet: Some("Next agent should inspect replay".to_string()),
            items: vec![PlanItemArg {
                step: "Render rich artifact".to_string(),
                status: StepStatus::InProgress,
            }],
            ..PlanSnapshot::default()
        }),
        subagent_snapshots: Vec::new(),
    };

    let block = state.to_system_block().expect("fork state block");

    assert!(block.contains("Objective: Make Plan mode reviewable"));
    assert!(block.contains("Context: Grounded in issue #2691"));
    assert!(block.contains("Source: gh issue view 2691"));
    assert!(block.contains("Critical file: crates/tui/src/tools/plan.rs"));
    assert!(block.contains("Constraint: Preserve legacy payloads"));
    assert!(block.contains("Verification plan: Run focused tests"));
    assert!(block.contains("Handoff packet: Next agent should inspect replay"));
    assert!(block.contains("- [~] Render rich artifact"));
}

#[test]
fn env_only_auth_error_gets_recovery_hint() {
    let _guard = lock_test_env();
    let _env = ScopedDeepSeekApiKey::set("stale-env-key");
    let (engine, _handle) = Engine::new(EngineConfig::default(), &Config::default());

    let message =
        engine.decorate_auth_error_message("Authentication failed: invalid API key".to_string());

    assert!(message.contains("DEEPSEEK_API_KEY"));
    assert!(message.contains("no saved config key is present"));
    assert!(message.contains("codewhale auth status"));
    assert!(message.contains("codewhale auth set --provider deepseek"));
}

#[test]
fn config_auth_error_does_not_blame_env() {
    let _guard = lock_test_env();
    let _env = ScopedDeepSeekApiKey::set("stale-env-key");
    let cfg = Config {
        api_key: Some("fresh-config-key".to_string()),
        ..Config::default()
    };
    let (engine, _handle) = Engine::new(EngineConfig::default(), &cfg);

    let message =
        engine.decorate_auth_error_message("Authentication failed: invalid API key".to_string());

    assert_eq!(message, "Authentication failed: invalid API key");
}

#[test]
fn plugin_tools_dir_honors_missing_custom_directory_without_fallback() {
    let missing = PathBuf::from("definitely-missing-codewhale-plugin-dir");
    let tools_config = crate::config::ToolsConfig {
        plugin_dir: Some(missing.to_string_lossy().to_string()),
        ..Default::default()
    };

    assert_eq!(plugin_tools_dir(Some(&tools_config)), missing);
}

#[test]
fn configure_plugin_tools_applies_overrides_after_discovered_plugins() {
    let tmp = tempdir().expect("tempdir");
    let plugin_dir = tmp.path().join("tools");
    fs::create_dir(&plugin_dir).expect("plugin dir");
    fs::write(
        plugin_dir.join("same-name.sh"),
        "# name: same_tool\n# description: discovered plugin\n",
    )
    .expect("plugin script");

    let mut overrides = HashMap::new();
    overrides.insert(
        "same_tool".to_string(),
        crate::config::ToolOverride::Command {
            command: "configured-command".to_string(),
            args: None,
        },
    );
    let tools_config = crate::config::ToolsConfig {
        plugin_dir: Some(plugin_dir.to_string_lossy().to_string()),
        overrides: Some(overrides),
        ..Default::default()
    };

    let ctx = crate::tools::ToolContext::new(tmp.path().to_path_buf());
    let mut registry = crate::tools::ToolRegistry::new(ctx);

    let plugin_names = configure_plugin_tools(&mut registry, Some(&tools_config));

    let tool = registry.get("same_tool").expect("same_tool registered");
    assert!(tool.description().contains("configured-command"));
    assert!(plugin_names.contains("same_tool"));
}

fn make_plan(
    read_only: bool,
    supports_parallel: bool,
    approval_required: bool,
    interactive: bool,
) -> ToolExecutionPlan {
    make_plan_at(
        0,
        read_only,
        supports_parallel,
        approval_required,
        interactive,
    )
}

fn make_plan_at(
    index: usize,
    read_only: bool,
    supports_parallel: bool,
    approval_required: bool,
    interactive: bool,
) -> ToolExecutionPlan {
    ToolExecutionPlan {
        index,
        id: format!("tool-{index}"),
        name: "grep_files".to_string(),
        input: json!({"pattern": "test"}),
        caller: None,
        interactive,
        approval_required,
        approval_description: "desc".to_string(),
        approval_force_prompt: false,
        supports_parallel,
        read_only,
        detached_start: false,
        blocked_error: None,
        guard_result: None,
    }
}

fn ask_rule_engine(command: &str) -> codewhale_execpolicy::ExecPolicyEngine {
    codewhale_execpolicy::ExecPolicyEngine::with_rulesets(vec![
        codewhale_execpolicy::Ruleset::user(vec![], vec![])
            .with_ask_rules(vec![codewhale_execpolicy::ToolAskRule::exec_shell(command)]),
    ])
}

fn file_ask_rule_engine(tool: &str, path: &str) -> codewhale_execpolicy::ExecPolicyEngine {
    codewhale_execpolicy::ExecPolicyEngine::with_rulesets(vec![
        codewhale_execpolicy::Ruleset::user(vec![], vec![]).with_ask_rules(vec![
            codewhale_execpolicy::ToolAskRule::file_path(tool, path),
        ]),
    ])
}

#[test]
fn exec_shell_ask_rule_decision_prompts_for_matching_auto_command() {
    let config = EngineConfig {
        exec_policy_engine: ask_rule_engine("cargo test"),
        ..EngineConfig::default()
    };

    let decision = exec_shell_ask_rule_decision(
        &config,
        "exec_shell",
        &json!({"command": "cargo test --workspace"}),
        Path::new("/repo"),
        crate::tui::approval::ApprovalMode::Auto,
    );

    assert_eq!(
        decision,
        Some(ToolAskRuleDecision::Prompt(
            "Typed ask rule 'tool=exec_shell command=cargo test' requires approval.".to_string()
        ))
    );
}

#[test]
fn exec_shell_ask_rule_decision_blocks_matching_never_command() {
    let config = EngineConfig {
        exec_policy_engine: ask_rule_engine("cargo test"),
        ..EngineConfig::default()
    };

    let decision = exec_shell_ask_rule_decision(
        &config,
        "exec_shell",
        &json!({"command": "cargo test --workspace"}),
        Path::new("/repo"),
        crate::tui::approval::ApprovalMode::Never,
    );

    assert_eq!(
        decision,
        Some(ToolAskRuleDecision::Block(
            "Typed ask rule 'tool=exec_shell command=cargo test' requires approval, but approval policy is never.".to_string()
        ))
    );
}

#[test]
fn exec_shell_ask_rule_decision_ignores_unmatched_command() {
    let config = EngineConfig {
        exec_policy_engine: ask_rule_engine("cargo test"),
        ..EngineConfig::default()
    };

    let decision = exec_shell_ask_rule_decision(
        &config,
        "exec_shell",
        &json!({"command": "git status"}),
        Path::new("/repo"),
        crate::tui::approval::ApprovalMode::Auto,
    );

    assert_eq!(decision, None);
}

#[test]
fn file_ask_rule_decision_prompts_for_matching_read_path() {
    let config = EngineConfig {
        exec_policy_engine: file_ask_rule_engine("read_file", "secrets/api_key.txt"),
        ..EngineConfig::default()
    };

    let decision = file_tool_ask_rule_decision(
        &config,
        "read_file",
        &json!({"path": "secrets/api_key.txt"}),
        Path::new("/repo"),
        crate::tui::approval::ApprovalMode::Auto,
    );

    assert_eq!(
        decision,
        Some(ToolAskRuleDecision::Prompt(
            "Typed ask rule 'tool=read_file path=secrets/api_key.txt' requires approval."
                .to_string()
        ))
    );
}

#[test]
fn file_ask_rule_decision_blocks_matching_read_path_when_approval_is_never() {
    let config = EngineConfig {
        exec_policy_engine: file_ask_rule_engine("read_file", "secrets/api_key.txt"),
        ..EngineConfig::default()
    };

    let decision = file_tool_ask_rule_decision(
        &config,
        "read_file",
        &json!({"path": "secrets/api_key.txt"}),
        Path::new("/repo"),
        crate::tui::approval::ApprovalMode::Never,
    );

    assert_eq!(
        decision,
        Some(ToolAskRuleDecision::Block(
            "Typed ask rule 'tool=read_file path=secrets/api_key.txt' requires approval, but approval policy is never.".to_string()
        ))
    );
}

#[test]
fn file_ask_rule_decision_ignores_unmatched_path() {
    let config = EngineConfig {
        exec_policy_engine: file_ask_rule_engine("read_file", "secrets/api_key.txt"),
        ..EngineConfig::default()
    };

    let decision = file_tool_ask_rule_decision(
        &config,
        "read_file",
        &json!({"path": "docs/readme.md"}),
        Path::new("/repo"),
        crate::tui::approval::ApprovalMode::Auto,
    );

    assert_eq!(decision, None);
}

fn api_tool(name: &str) -> Tool {
    Tool {
        tool_type: Some("function".to_string()),
        name: name.to_string(),
        description: format!("Test tool {name}"),
        input_schema: json!({"type": "object"}),
        allowed_callers: Some(vec!["direct".to_string()]),
        defer_loading: None,
        input_examples: None,
        strict: None,
        cache_control: None,
    }
}

#[test]
fn engine_handle_cancel_tracks_latest_turn_token() {
    let (mut engine, handle) = Engine::new(EngineConfig::default(), &Config::default());
    let stale_token = engine.cancel_token.clone();

    engine.reset_cancel_token();
    handle.cancel();

    assert!(engine.cancel_token.is_cancelled());
    assert!(handle.is_cancelled());
    assert!(!stale_token.is_cancelled());
}

#[test]
fn engine_initial_prompt_includes_configured_goal() {
    let config = EngineConfig {
        goal_objective: Some("Fix goal handoff".to_string()),
        ..Default::default()
    };
    let (engine, _handle) = Engine::new(config, &Config::default());
    let prompt = match engine.session.system_prompt {
        Some(SystemPrompt::Text(text)) => text,
        Some(SystemPrompt::Blocks(blocks)) => blocks
            .into_iter()
            .map(|block| block.text)
            .collect::<Vec<_>>()
            .join("\n"),
        None => panic!("expected system prompt"),
    };

    assert!(prompt.contains("<session_goal>"));
    assert!(prompt.contains("Fix goal handoff"));
    assert!(
        engine
            .config
            .goal_state
            .lock()
            .expect("goal lock")
            .is_active()
    );
}

#[test]
fn engine_initial_prompt_omits_paused_goal() {
    let config = EngineConfig {
        goal_objective: Some("Wait for confirmation".to_string()),
        goal_status: GoalStatus::Paused,
        ..Default::default()
    };
    let (engine, _handle) = Engine::new(config, &Config::default());
    let prompt = match engine.session.system_prompt {
        Some(SystemPrompt::Text(text)) => text,
        Some(SystemPrompt::Blocks(blocks)) => blocks
            .into_iter()
            .map(|block| block.text)
            .collect::<Vec<_>>()
            .join("\n"),
        None => panic!("expected system prompt"),
    };

    assert!(!prompt.contains("<session_goal>"));
    assert!(
        !engine
            .config
            .goal_state
            .lock()
            .expect("goal lock")
            .is_active()
    );
}

#[test]
fn refresh_system_prompt_uses_runtime_goal_state() {
    let (mut engine, _handle) = Engine::new(EngineConfig::default(), &Config::default());
    {
        let mut goal = engine.config.goal_state.lock().expect("goal lock");
        goal.create("Close the runtime goal loop".to_string(), None);
    }

    engine.refresh_system_prompt();
    let prompt = match engine.session.system_prompt {
        Some(SystemPrompt::Text(text)) => text,
        Some(SystemPrompt::Blocks(blocks)) => blocks
            .into_iter()
            .map(|block| block.text)
            .collect::<Vec<_>>()
            .join("\n"),
        None => panic!("expected system prompt"),
    };

    assert!(prompt.contains("<session_goal>"));
    assert!(prompt.contains("Close the runtime goal loop"));
}

#[tokio::test]
async fn runtime_goal_updates_emit_ui_snapshot() {
    let (engine, handle) = Engine::new(EngineConfig::default(), &Config::default());
    {
        let mut goal = engine.config.goal_state.lock().expect("goal lock");
        goal.create("Ship the release lane".to_string(), Some(42_000));
        goal.mark_complete(
            "verified with focused tests".to_string(),
            crate::tools::goal::GoalCompletionVerification {
                status: "passed".to_string(),
                check: "cargo test -p codewhale-tui runtime_goal_updates_emit_ui_snapshot"
                    .to_string(),
                summary: "focused runtime goal snapshot test passed".to_string(),
            },
        )
        .expect("mark complete");
    }

    engine.emit_goal_updated().await;

    let mut rx = handle.rx_event.write().await;
    match rx.recv().await.expect("goal update event") {
        Event::GoalUpdated { snapshot } => {
            assert_eq!(snapshot.objective.as_deref(), Some("Ship the release lane"));
            assert_eq!(snapshot.status, "complete");
            assert_eq!(snapshot.token_budget, Some(42_000));
            assert_eq!(
                snapshot.evidence.as_deref(),
                Some("verified with focused tests")
            );
        }
        other => panic!("expected GoalUpdated, got {other:?}"),
    }
}

#[test]
fn parallel_batch_requires_read_only_parallel_tools() {
    let plans = vec![make_plan(true, true, false, false)];
    assert!(should_parallelize_tool_batch(&plans));

    let plans = vec![
        make_plan(true, true, false, false),
        make_plan(true, true, false, false),
    ];
    assert!(should_parallelize_tool_batch(&plans));

    let plans = vec![make_plan(false, true, false, false)];
    assert!(!should_parallelize_tool_batch(&plans));

    let plans = vec![make_plan(true, false, false, false)];
    assert!(!should_parallelize_tool_batch(&plans));

    let plans = vec![make_plan(true, true, true, false)];
    assert!(!should_parallelize_tool_batch(&plans));

    let plans = vec![make_plan(true, true, false, true)];
    assert!(!should_parallelize_tool_batch(&plans));

    let mut background = make_plan(false, false, false, false);
    background.detached_start = true;
    assert!(should_parallelize_tool_batch(&[background]));

    let mut gated_background = make_plan(false, false, true, false);
    gated_background.detached_start = true;
    assert!(!should_parallelize_tool_batch(&[gated_background]));
}

#[test]
fn tool_execution_batches_use_serial_barriers() {
    let batches = plan_tool_execution_batches(vec![
        make_plan_at(0, true, true, false, false),
        make_plan_at(1, true, true, false, false),
        make_plan_at(2, false, false, true, false),
        make_plan_at(3, true, true, false, false),
        make_plan_at(4, true, false, false, false),
        make_plan_at(5, true, true, false, false),
        make_plan_at(6, true, true, false, false),
    ]);

    assert_eq!(batches.len(), 5);

    match &batches[0] {
        ToolExecutionBatch::Parallel(plans) => {
            assert_eq!(
                plans.iter().map(|plan| plan.index).collect::<Vec<_>>(),
                vec![0, 1]
            );
        }
        ToolExecutionBatch::Serial(_) => panic!("first batch should be parallel"),
    }
    match &batches[1] {
        ToolExecutionBatch::Serial(plan) => assert_eq!(plan.index, 2),
        ToolExecutionBatch::Parallel(_) => panic!("second batch should be serial"),
    }
    match &batches[2] {
        ToolExecutionBatch::Parallel(plans) => {
            assert_eq!(
                plans.iter().map(|plan| plan.index).collect::<Vec<_>>(),
                vec![3]
            );
        }
        ToolExecutionBatch::Serial(_) => panic!("third batch should be parallel"),
    }
    match &batches[3] {
        ToolExecutionBatch::Serial(plan) => assert_eq!(plan.index, 4),
        ToolExecutionBatch::Parallel(_) => panic!("fourth batch should be serial"),
    }
    match &batches[4] {
        ToolExecutionBatch::Parallel(plans) => {
            assert_eq!(
                plans.iter().map(|plan| plan.index).collect::<Vec<_>>(),
                vec![5, 6]
            );
        }
        ToolExecutionBatch::Serial(_) => panic!("fifth batch should be parallel"),
    }
}

#[test]
fn shell_readonly_plans_batch_around_serial_barrier() {
    let mut shell_a = make_plan_at(0, true, true, false, false);
    shell_a.name = "exec_shell".to_string();
    shell_a.input = json!({"command": "git status -s"});
    let mut shell_b = make_plan_at(1, true, true, false, false);
    shell_b.name = "exec_shell".to_string();
    shell_b.input = json!({"command": "git log --oneline -5"});
    let mut write_shell = make_plan_at(2, false, false, true, false);
    write_shell.name = "exec_shell".to_string();
    write_shell.input = json!({"command": "cargo build"});
    let mut shell_c = make_plan_at(3, true, true, false, false);
    shell_c.name = "exec_shell".to_string();
    shell_c.input = json!({"command": "bash -lc 'rg TODO crates/tui/src/core'"});

    let batches = plan_tool_execution_batches(vec![shell_a, shell_b, write_shell, shell_c]);
    assert_eq!(batches.len(), 3);

    match &batches[0] {
        ToolExecutionBatch::Parallel(plans) => {
            assert_eq!(
                plans.iter().map(|plan| plan.index).collect::<Vec<_>>(),
                vec![0, 1]
            );
        }
        ToolExecutionBatch::Serial(_) => panic!("first batch should be parallel"),
    }
    match &batches[1] {
        ToolExecutionBatch::Serial(plan) => assert_eq!(plan.index, 2),
        ToolExecutionBatch::Parallel(_) => panic!("write shell should be a serial barrier"),
    }
    match &batches[2] {
        ToolExecutionBatch::Parallel(plans) => {
            assert_eq!(
                plans.iter().map(|plan| plan.index).collect::<Vec<_>>(),
                vec![3]
            );
        }
        ToolExecutionBatch::Serial(_) => panic!("third batch should be parallel"),
    }
}

#[test]
fn background_shell_starts_batch_with_readonly_tools_when_auto_approved() {
    let mut shell_a = make_plan_at(0, true, true, false, false);
    shell_a.name = "exec_shell".to_string();
    shell_a.input = json!({"command": "git status -s"});

    let mut background_cargo = make_plan_at(1, false, false, false, false);
    background_cargo.name = "exec_shell".to_string();
    background_cargo.input = json!({"command": "cargo check --workspace", "background": true});
    background_cargo.detached_start = true;

    let mut shell_b = make_plan_at(2, true, true, false, false);
    shell_b.name = "exec_shell".to_string();
    shell_b.input = json!({"command": "rg TODO crates/tui/src/core"});

    let batches = plan_tool_execution_batches(vec![shell_a, background_cargo, shell_b]);
    assert_eq!(batches.len(), 1);

    match &batches[0] {
        ToolExecutionBatch::Parallel(plans) => {
            assert_eq!(
                plans.iter().map(|plan| plan.index).collect::<Vec<_>>(),
                vec![0, 1, 2]
            );
        }
        ToolExecutionBatch::Serial(_) => {
            panic!("background shell start should join parallel batch")
        }
    }
}

#[test]
fn background_verifier_starts_batch_with_readonly_tools_when_auto_approved() {
    let mut shell_a = make_plan_at(0, true, true, false, false);
    shell_a.name = "exec_shell".to_string();
    shell_a.input = json!({"command": "git status -s"});

    let mut verifier = make_plan_at(1, false, false, false, false);
    verifier.name = "run_verifiers".to_string();
    verifier.input = json!({"profile": "rust", "level": "full", "background": true});
    verifier.detached_start = true;

    let mut shell_b = make_plan_at(2, true, true, false, false);
    shell_b.name = "exec_shell".to_string();
    shell_b.input = json!({"command": "rg TODO crates/tui/src/core"});

    let batches = plan_tool_execution_batches(vec![shell_a, verifier, shell_b]);
    assert_eq!(batches.len(), 1);

    match &batches[0] {
        ToolExecutionBatch::Parallel(plans) => {
            assert_eq!(
                plans.iter().map(|plan| plan.index).collect::<Vec<_>>(),
                vec![0, 1, 2]
            );
        }
        ToolExecutionBatch::Serial(_) => {
            panic!("background verifier start should join parallel batch")
        }
    }
}

#[test]
fn successful_update_plan_ends_plan_mode_turn_immediately() {
    assert!(should_stop_after_plan_tool(
        AppMode::Plan,
        "update_plan",
        &Ok(ToolResult::success("planned"))
    ));
    assert!(!should_stop_after_plan_tool(
        AppMode::Agent,
        "update_plan",
        &Ok(ToolResult::success("planned"))
    ));
    assert!(!should_stop_after_plan_tool(
        AppMode::Plan,
        "request_user_input",
        &Ok(ToolResult::success("input"))
    ));
    assert!(!should_stop_after_plan_tool(
        AppMode::Plan,
        "update_plan",
        &Err(ToolError::execution_failed("failed".to_string()))
    ));
}

#[test]
fn quick_plan_requests_force_update_plan_on_first_step() {
    assert!(should_force_update_plan_first(
        AppMode::Plan,
        "Give me a quick 3-step plan to verify the UI changes."
    ));
    assert!(should_force_update_plan_first(
        AppMode::Plan,
        "Make a high-level plan for the footer work."
    ));
    assert!(!should_force_update_plan_first(
        AppMode::Plan,
        "Can you make a plan to get ver 0.8.61 fully built and benchmark it with our api server?"
    ));
    assert!(!should_force_update_plan_first(
        AppMode::Plan,
        "Make a high-level plan for benchmarking https://github.com/sierra-research/tau2-bench."
    ));
    assert!(!should_force_update_plan_first(
        AppMode::Plan,
        "Inspect the repo and then give me a quick plan."
    ));
    assert!(!should_force_update_plan_first(
        AppMode::Agent,
        "Give me a quick 3-step plan."
    ));
}

#[test]
fn quick_plan_turn_can_narrow_first_step_tools_to_update_plan() {
    let catalog = vec![
        Tool {
            tool_type: Some("function".to_string()),
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            input_schema: json!({"type": "object"}),
            allowed_callers: Some(vec!["direct".to_string()]),
            defer_loading: Some(false),
            input_examples: None,
            strict: None,
            cache_control: None,
        },
        Tool {
            tool_type: Some("function".to_string()),
            name: "update_plan".to_string(),
            description: "Publish a plan".to_string(),
            input_schema: json!({"type": "object"}),
            allowed_callers: Some(vec!["direct".to_string()]),
            defer_loading: Some(false),
            input_examples: None,
            strict: None,
            cache_control: None,
        },
    ];
    let active = initial_active_tools(&catalog);

    let forced = active_tools_for_step(&catalog, &active, true);
    assert_eq!(forced.len(), 1);
    assert_eq!(forced[0].name, "update_plan");

    let default = active_tools_for_step(&catalog, &active, false);
    assert_eq!(default.len(), 2);
}

#[test]
fn tool_error_messages_include_actionable_hints() {
    let path_error = ToolError::path_escape(PathBuf::from("../escape.txt"));
    let formatted = format_tool_error(&path_error, "read_file");
    assert!(formatted.contains("escapes workspace"));

    let missing_field = ToolError::missing_field("path");
    let formatted = format_tool_error(&missing_field, "read_file");
    assert!(formatted.contains("missing required field"));

    let timeout = ToolError::Timeout { seconds: 5 };
    let formatted = format_tool_error(&timeout, "exec_shell");
    assert!(formatted.contains("timed out"));

    // #3020: Plan-mode denials already explain the fix — pass through
    // verbatim, with no conflicting "Adjust approval mode" suffix.
    let plan_denied = ToolError::permission_denied(
        "'exec_shell' is not available in Plan mode — switch to Agent or YOLO mode to run commands and code.",
    );
    let formatted = format_tool_error(&plan_denied, "exec_shell");
    assert_eq!(
        formatted,
        "'exec_shell' is not available in Plan mode — switch to Agent or YOLO mode to run commands and code."
    );

    // Bare denials still get the actionable suffix.
    let bare_denied = ToolError::permission_denied("nope");
    let formatted = format_tool_error(&bare_denied, "exec_shell");
    assert!(
        formatted.contains("Adjust approval mode or request permission"),
        "{formatted}"
    );

    // "model" must not satisfy the "mode" pass-through check.
    let model_denied = ToolError::permission_denied("requested model is not allowed");
    let formatted = format_tool_error(&model_denied, "agent");
    assert!(
        formatted.contains("Adjust approval mode or request permission"),
        "{formatted}"
    );
}

#[test]
fn tool_exec_outcome_tracks_duration() {
    let outcome = ToolExecOutcome {
        index: 0,
        id: "tool-1".to_string(),
        name: "grep_files".to_string(),
        input: json!({"pattern": "test"}),
        started_at: Instant::now(),
        result: Ok(ToolResult::success("ok")),
    };

    assert!(outcome.started_at.elapsed().as_nanos() > 0);
}

#[test]
fn core_native_tools_stay_loaded_in_yolo_mode() {
    let always_load = HashSet::new();
    assert!(!should_default_defer_tool("exec_shell", &always_load));
    // git_blame remains deferred (read-only git history beyond log/show/diff).
    assert!(should_default_defer_tool("git_blame", &always_load));
}

#[test]
fn non_yolo_mode_retains_default_defer_policy() {
    let always_load = HashSet::new();
    assert!(!should_default_defer_tool("exec_shell", &always_load));
    assert!(!should_default_defer_tool("edit_file", &always_load));
    assert!(!should_default_defer_tool("apply_patch", &always_load));
    assert!(!should_default_defer_tool("fetch_url", &always_load));
    assert!(!should_default_defer_tool("git_diff", &always_load));
    // #2654: read-only git history joins the active set.
    assert!(!should_default_defer_tool("git_log", &always_load));
    assert!(!should_default_defer_tool("git_show", &always_load));
    assert!(!should_default_defer_tool("git_status", &always_load));
    assert!(!should_default_defer_tool("run_tests", &always_load));
    assert!(!should_default_defer_tool("agent", &always_load));
    assert!(!should_default_defer_tool("read_file", &always_load));
    assert!(!should_default_defer_tool("web_search", &always_load));
    assert!(!should_default_defer_tool("write_file", &always_load));
    assert!(!should_default_defer_tool("task_shell_start", &always_load));
    assert!(!should_default_defer_tool("task_shell_wait", &always_load));
    assert!(should_default_defer_tool("git_blame", &always_load));
}

#[test]
fn model_tool_catalog_applies_native_and_mcp_deferral() {
    let always_load = HashSet::new();
    let catalog = build_model_tool_catalog(
        vec![
            api_tool("read_file"),
            api_tool("write_file"),
            api_tool("exec_shell"),
            api_tool("edit_file"),
            api_tool("project_map"),
        ],
        vec![api_tool("list_mcp_resources"), api_tool("mcp_server_write")],
        AppMode::Agent,
        &always_load,
    );

    let defer_loading = |name: &str| {
        catalog
            .iter()
            .find(|tool| tool.name == name)
            .and_then(|tool| tool.defer_loading)
    };

    assert_eq!(defer_loading("read_file"), Some(false));
    assert_eq!(defer_loading("write_file"), Some(false));
    assert_eq!(defer_loading("exec_shell"), Some(false));
    assert_eq!(defer_loading("edit_file"), Some(false));
    assert_eq!(defer_loading("project_map"), Some(true));
    assert_eq!(defer_loading("list_mcp_resources"), Some(false));
    assert_eq!(defer_loading("mcp_server_write"), Some(true));
}

#[test]
fn plugin_or_benchmark_tools_marked_loaded_stay_active() {
    let always_load = HashSet::new();
    let mut catalog = build_model_tool_catalog(
        vec![api_tool("KB_search"), api_tool("read_file")],
        Vec::new(),
        AppMode::Agent,
        &always_load,
    );

    // Mirrors Engine::run after configure_plugin_tools(): plugin tools are
    // explicitly kept loaded, and no provider-specific policy should re-defer
    // them before the first model request.
    let bench_tool = catalog
        .iter_mut()
        .find(|tool| tool.name == "KB_search")
        .expect("benchmark tool in catalog");
    bench_tool.defer_loading = Some(false);
    ensure_advanced_tooling(&mut catalog, AppMode::Agent, &always_load);

    let active = initial_active_tools(&catalog);
    assert!(
        active.contains("KB_search"),
        "plugin/benchmark tools marked loaded must be callable on turn 1"
    );
    assert!(active.contains("read_file"));
}

#[test]
fn agent_catalog_keeps_edit_file_loaded_when_fuzz_is_omitted() {
    let (engine, _handle) = Engine::new(EngineConfig::default(), &Config::default());
    let registry = engine
        .build_turn_tool_registry_builder(
            AppMode::Agent,
            engine.config.todos.clone(),
            engine.config.plan_state.clone(),
        )
        .build(engine.build_tool_context(AppMode::Agent, false));
    let always_load = HashSet::new();
    let catalog = build_model_tool_catalog(
        registry.to_api_tools_with_cache(true),
        vec![],
        AppMode::Agent,
        &always_load,
    );
    let edit = catalog
        .iter()
        .find(|tool| tool.name == "edit_file")
        .expect("edit_file registered");

    assert_eq!(edit.defer_loading, Some(false));
    let required = edit.input_schema["required"]
        .as_array()
        .expect("edit_file schema should include required fields");
    assert!(required.iter().any(|field| field.as_str() == Some("path")));
    assert!(
        required
            .iter()
            .any(|field| field.as_str() == Some("search"))
    );
    assert!(
        required
            .iter()
            .any(|field| field.as_str() == Some("replace"))
    );
    assert!(!required.iter().any(|field| field.as_str() == Some("fuzz")));
    assert_eq!(
        edit.input_schema["properties"]["fuzz"]["type"].as_str(),
        Some("boolean")
    );

    let active_at_batch_start = initial_active_tools(&catalog);
    assert!(active_at_batch_start.contains("edit_file"));
    let mut hydrated_this_batch = HashSet::new();
    assert!(
        maybe_hydrate_requested_deferred_tool(
            "edit_file",
            &json!({
                "path": "src/foo.rs",
                "search": "before",
                "replace": "after"
            }),
            &catalog,
            &active_at_batch_start,
            &mut hydrated_this_batch,
        )
        .is_none(),
        "loaded edit_file calls without fuzz should execute instead of hydrating the schema"
    );
    assert!(hydrated_this_batch.is_empty());
}

#[test]
fn agent_catalog_advertises_and_searches_core_action_tools() {
    let (engine, _handle) = Engine::new(EngineConfig::default(), &Config::default());
    let registry = engine
        .build_turn_tool_registry_builder(
            AppMode::Agent,
            engine.config.todos.clone(),
            engine.config.plan_state.clone(),
        )
        .build(engine.build_tool_context(AppMode::Agent, false));
    let always_load = HashSet::new();
    let mut catalog = build_model_tool_catalog(
        registry.to_api_tools_with_cache(true),
        vec![],
        AppMode::Agent,
        &always_load,
    );
    ensure_advanced_tooling(&mut catalog, AppMode::Agent, &always_load);

    let issues = tool_catalog_consistency_issues(&catalog, &registry);
    assert!(
        issues.is_empty(),
        "Agent catalog should match the runtime registry: {issues:?}"
    );

    let names = catalog
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<HashSet<_>>();
    for tool_name in ["exec_shell", "write_file", "edit_file", "apply_patch"] {
        assert!(
            names.contains(tool_name),
            "{tool_name} must be advertised in Agent mode"
        );

        let mut active = initial_active_tools(&catalog);
        let result = execute_tool_search(
            TOOL_SEARCH_BM25_NAME,
            &json!({ "query": tool_name }),
            &catalog,
            &mut active,
        )
        .expect("tool search succeeds");
        let references = result.metadata.as_ref().unwrap()["tool_references"]
            .as_array()
            .expect("tool references are an array");
        assert!(
            references
                .iter()
                .any(|reference| reference.as_str() == Some(tool_name)),
            "{tool_name} should be discoverable by tool_search"
        );
        assert!(
            active.contains(tool_name),
            "{tool_name} should be activated by tool_search"
        );
    }
}

#[test]
fn catalog_consistency_self_check_flags_registered_core_tool_missing_from_catalog() {
    let (engine, _handle) = Engine::new(EngineConfig::default(), &Config::default());
    let registry = engine
        .build_turn_tool_registry_builder(
            AppMode::Agent,
            engine.config.todos.clone(),
            engine.config.plan_state.clone(),
        )
        .build(engine.build_tool_context(AppMode::Agent, false));
    let always_load = HashSet::new();
    let mut catalog = build_model_tool_catalog(
        registry.to_api_tools_with_cache(true),
        vec![],
        AppMode::Agent,
        &always_load,
    );
    catalog.retain(|tool| tool.name != "exec_shell");

    let issues = tool_catalog_consistency_issues(&catalog, &registry);
    assert!(
        issues
            .iter()
            .any(|issue| issue.contains("registered core tool 'exec_shell'")),
        "missing registered exec_shell should be reported: {issues:?}"
    );
}

#[test]
fn tool_search_reports_known_core_action_tool_when_current_catalog_omits_it() {
    let catalog = vec![api_tool("read_file")];
    let mut active = initial_active_tools(&catalog);

    let result = execute_tool_search(
        TOOL_SEARCH_BM25_NAME,
        &json!({ "query": "exec_shell" }),
        &catalog,
        &mut active,
    )
    .expect("tool search succeeds");

    assert!(!active.contains("exec_shell"));
    let unavailable = result.metadata.as_ref().unwrap()["unavailable_tool_references"]
        .as_array()
        .expect("unavailable references are an array");
    assert!(
        unavailable.iter().any(|reference| {
            reference["tool_name"].as_str() == Some("exec_shell")
                && reference["reason"]
                    .as_str()
                    .is_some_and(|reason| reason.contains("allow_shell = true"))
        }),
        "known-but-omitted core action tool should surface with a reason: {unavailable:?}"
    );
}

#[test]
fn tools_always_load_overrides_default_native_deferral() {
    let always_load = HashSet::from(["git_blame".to_string()]);
    assert!(!should_default_defer_tool("git_blame", &always_load));
}

#[test]
#[ignore = "one-shot metric for scripts/measure-tool-catalog.py"]
#[allow(clippy::print_stderr)]
fn print_agent_tool_catalog_metrics() {
    let tmp = tempdir().expect("tempdir");
    let context = crate::tools::ToolContext::new(tmp.path().to_path_buf());
    let client = DeepSeekClient::new(&Config {
        api_key: Some("test-key".to_string()),
        ..Config::default()
    })
    .expect("stub client");
    let manager = crate::tools::subagent::new_shared_subagent_manager(tmp.path().to_path_buf(), 8);
    let runtime = crate::tools::subagent::SubAgentRuntime::new(
        client,
        DEFAULT_TEXT_MODEL.to_string(),
        context.clone(),
        true,
        None,
        manager.clone(),
    );
    let registry = crate::tools::ToolRegistryBuilder::new()
        .with_agent_tools(true)
        .with_todo_tool(new_shared_todo_list())
        .with_plan_tool(new_shared_plan_state())
        .with_review_tool(None, DEFAULT_TEXT_MODEL.to_string())
        .with_rlm_tool(None, DEFAULT_TEXT_MODEL.to_string())
        .with_notify_tool()
        .with_subagent_tools(manager, runtime)
        .build(context);
    let baseline_catalog = registry.to_api_tools_with_cache(true);
    let baseline_json = serde_json::to_vec(&baseline_catalog).expect("serialize baseline");

    let always_load = HashSet::new();
    let mut catalog = build_model_tool_catalog(
        baseline_catalog.clone(),
        vec![],
        AppMode::Agent,
        &always_load,
    );
    ensure_advanced_tooling(&mut catalog, AppMode::Agent, &always_load);
    let active = initial_active_tools(&catalog);
    let active_catalog = active_tools_for_step(&catalog, &active, false);
    let active_json = serde_json::to_vec(&active_catalog).expect("serialize active");
    let reduction_percent = if baseline_json.is_empty() {
        0.0
    } else {
        100.0 * (baseline_json.len().saturating_sub(active_json.len())) as f64
            / baseline_json.len() as f64
    };

    eprintln!(
        "TOOL_CATALOG_METRICS {}",
        serde_json::json!({
            "baseline_tools": baseline_catalog.len(),
            "baseline_bytes": baseline_json.len(),
            "baseline_tokens_est": baseline_json.len().div_ceil(4),
            "active_tools": active_catalog.len(),
            "active_bytes": active_json.len(),
            "active_tokens_est": active_json.len().div_ceil(4),
            "reduction_percent": reduction_percent,
            "active_tool_names": active_catalog.iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>(),
        })
    );
}

#[test]
fn deferred_edit_file_first_use_hydrates_schema_without_execution() {
    let mut edit = api_tool("edit_file");
    edit.defer_loading = Some(true);
    edit.input_schema = json!({
        "type": "object",
        "properties": {
            "path": { "type": "string" },
            "search": { "type": "string" },
            "replace": { "type": "string" }
        },
        "required": ["path", "search", "replace"]
    });

    let catalog = vec![edit];
    let active_at_batch_start = HashSet::new();
    let mut hydrated_this_batch = HashSet::new();
    let result = maybe_hydrate_requested_deferred_tool(
        "edit_file",
        &json!({
            "path": "src/foo.rs",
            "old_string": "before",
            "new_string": "after"
        }),
        &catalog,
        &active_at_batch_start,
        &mut hydrated_this_batch,
    )
    .expect("first deferred use should hydrate");

    assert!(!active_at_batch_start.contains("edit_file"));
    assert!(hydrated_this_batch.contains("edit_file"));
    assert!(result.success);
    assert!(result.content.contains("Tool `edit_file` was deferred"));
    assert!(result.content.contains("path: string"));
    assert!(result.content.contains("search: string"));
    assert!(result.content.contains("replace: string"));
    assert!(result.content.contains("old_string -> search"));
    assert!(result.content.contains("new_string -> replace"));
    assert!(result.content.contains("The tool was not executed"));

    let metadata = result.metadata.expect("metadata");
    assert_eq!(metadata["event"], "tool.schema_hydrated");
    assert_eq!(metadata["executed"], false);
    assert_eq!(metadata["retry_required"], true);

    let second_result = maybe_hydrate_requested_deferred_tool(
        "edit_file",
        &json!({"path": "src/bar.rs", "old_string": "before", "new_string": "after"}),
        &catalog,
        &active_at_batch_start,
        &mut hydrated_this_batch,
    )
    .expect("later calls in the same batch should hydrate instead of executing");
    assert_eq!(second_result.metadata.unwrap()["executed"], false);
    assert_eq!(hydrated_this_batch.len(), 1);

    let mut active_next_batch = active_at_batch_start.clone();
    active_next_batch.extend(hydrated_this_batch);
    let mut hydrated_next_batch = HashSet::new();
    assert!(
        maybe_hydrate_requested_deferred_tool(
            "edit_file",
            &json!({"path": "src/foo.rs", "search": "before", "replace": "after"}),
            &catalog,
            &active_next_batch,
            &mut hydrated_next_batch,
        )
        .is_none(),
        "tools hydrated in a previous batch should execute normally"
    );
}

#[test]
fn model_tool_catalog_defers_non_core_native_tools_in_yolo_mode() {
    let always_load = HashSet::new();
    let catalog = build_model_tool_catalog(
        vec![api_tool("read_file"), api_tool("project_map")],
        vec![api_tool("mcp_server_write")],
        AppMode::Yolo,
        &always_load,
    );

    let defer_loading = |name: &str| {
        catalog
            .iter()
            .find(|tool| tool.name == name)
            .and_then(|tool| tool.defer_loading)
    };

    assert_eq!(defer_loading("read_file"), Some(false));
    assert_eq!(defer_loading("project_map"), Some(true));
    assert_eq!(defer_loading("mcp_server_write"), Some(false));
}

#[test]
fn model_tool_catalog_sorts_each_partition_for_prefix_cache_stability() {
    // Regression for #263: deterministic byte order of the tools array is a
    // hard requirement for DeepSeek's KV prefix cache. Built-ins stay as a
    // contiguous prefix; MCP tools follow. Within each partition: alphabetical.
    let always_load = HashSet::new();
    let catalog = build_model_tool_catalog(
        vec![
            api_tool("read_file"),
            api_tool("apply_patch"),
            api_tool("exec_shell"),
        ],
        vec![api_tool("mcp_zoo_b"), api_tool("mcp_aardvark_a")],
        AppMode::Yolo,
        &always_load,
    );

    let names: Vec<&str> = catalog.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "apply_patch",
            "exec_shell",
            "read_file",
            "mcp_aardvark_a",
            "mcp_zoo_b",
        ],
        "built-ins must be alphabetical and contiguous; MCP tools follow, alphabetical",
    );
}

#[test]
fn active_tool_list_pushes_deferred_activations_to_the_tail() {
    // Regression for #263: when ToolSearch activates a deferred tool mid-
    // session, it must NOT be inserted at its catalog index — that would
    // shift every later tool's byte offset and bust the cached prefix.
    // Deferred-but-now-active tools belong at the tail.
    let mut a = api_tool("a_load_now");
    a.defer_loading = Some(false);
    let mut search = api_tool("search_via_toolsearch");
    search.defer_loading = Some(true);
    let mut b = api_tool("b_load_now");
    b.defer_loading = Some(false);

    let catalog = vec![a, search, b];
    let active: HashSet<String> = ["a_load_now", "search_via_toolsearch", "b_load_now"]
        .into_iter()
        .map(String::from)
        .collect();

    let listed = active_tools_for_step(&catalog, &active, false);
    let names: Vec<&str> = listed.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["a_load_now", "b_load_now", "search_via_toolsearch"],
        "deferred-but-active tools must come after always-loaded tools",
    );
}

#[test]
fn deferred_tool_preflight_loads_edit_schema_without_executing_bad_aliases() {
    let (engine, _handle) = Engine::new(EngineConfig::default(), &Config::default());
    let registry = engine
        .build_turn_tool_registry_builder(
            AppMode::Agent,
            engine.config.todos.clone(),
            engine.config.plan_state.clone(),
        )
        .build(engine.build_tool_context(AppMode::Agent, false));
    let always_load = HashSet::new();
    let mut catalog = build_model_tool_catalog(
        registry.to_api_tools_with_cache(true),
        vec![],
        AppMode::Agent,
        &always_load,
    );
    catalog
        .iter_mut()
        .find(|tool| tool.name == "edit_file")
        .expect("edit_file registered")
        .defer_loading = Some(true);
    let mut active = initial_active_tools(&catalog);
    assert!(!active.contains("edit_file"));

    let result = preflight_requested_deferred_tool(
        "edit_file",
        &json!({
            "path": "src/foo.rs",
            "old_string": "before",
            "new_string": "after"
        }),
        &catalog,
        &mut active,
    )
    .expect("deferred edit_file should preflight");

    assert!(active.contains("edit_file"));
    assert!(result.success);
    assert!(result.content.contains("Tool `edit_file` was deferred"));
    assert!(result.content.contains("The tool was not executed"));
    assert!(result.content.contains("path: string required"));
    assert!(result.content.contains("search: string required"));
    assert!(result.content.contains("replace: string required"));
    assert!(result.content.contains("old_string -> search"));
    assert!(result.content.contains("new_string -> replace"));
    assert_eq!(
        result.metadata.as_ref().unwrap()["deferred_tool_loaded"],
        json!(true)
    );
}

#[test]
fn deferred_tool_preflight_guides_rlm_open_misnamed_source_fields() {
    let (engine, _handle) = Engine::new(EngineConfig::default(), &Config::default());
    let registry = engine
        .build_turn_tool_registry_builder(
            AppMode::Agent,
            engine.config.todos.clone(),
            engine.config.plan_state.clone(),
        )
        .build(engine.build_tool_context(AppMode::Agent, false));
    let always_load = HashSet::new();
    let mut catalog = build_model_tool_catalog(
        registry.to_api_tools_with_cache(true),
        vec![],
        AppMode::Agent,
        &always_load,
    );
    catalog
        .iter_mut()
        .find(|tool| tool.name == "rlm_open")
        .expect("rlm_open registered")
        .defer_loading = Some(true);
    let mut active = initial_active_tools(&catalog);
    assert!(!active.contains("rlm_open"));

    let result = preflight_requested_deferred_tool(
        "rlm_open",
        &json!({
            "name": "active_prompt",
            "prompt": "inspect this",
            "path": "src/lib.rs"
        }),
        &catalog,
        &mut active,
    )
    .expect("deferred rlm_open should preflight");

    assert!(active.contains("rlm_open"));
    assert!(result.success);
    assert!(result.content.contains("Tool `rlm_open` was deferred"));
    assert!(result.content.contains("The tool was not executed"));
    assert!(result.content.contains("session_object: string"));
    assert!(
        result.content.contains(
            "prompt -> file_path (local file), content (inline text), url, or session_object"
        ),
        "prompt correction includes session_object: {}",
        result.content
    );
    assert!(
        result.content.contains(
            "path -> file_path (local file), content (inline text), url, or session_object"
        ),
        "path correction includes session_object: {}",
        result.content
    );
    assert_eq!(
        result.metadata.as_ref().unwrap()["deferred_tool_loaded"],
        json!(true)
    );
}

#[test]
fn deferred_tool_preflight_guides_checklist_update_list_replacement() {
    let (engine, _handle) = Engine::new(EngineConfig::default(), &Config::default());
    let registry = engine
        .build_turn_tool_registry_builder(
            AppMode::Agent,
            engine.config.todos.clone(),
            engine.config.plan_state.clone(),
        )
        .build(engine.build_tool_context(AppMode::Agent, false));
    let always_load = HashSet::new();
    let catalog = build_model_tool_catalog(
        registry.to_api_tools_with_cache(true),
        vec![],
        AppMode::Agent,
        &always_load,
    );
    let mut active = initial_active_tools(&catalog);
    assert!(!active.contains("checklist_update"));

    let result = preflight_requested_deferred_tool(
        "checklist_update",
        &json!({
            "todos": [
                { "content": "wire preflight", "status": "completed" }
            ]
        }),
        &catalog,
        &mut active,
    )
    .expect("deferred checklist_update should preflight");

    assert!(active.contains("checklist_update"));
    assert!(result.success);
    assert!(
        result
            .content
            .contains("Tool `checklist_update` was deferred")
    );
    assert!(result.content.contains("id: integer required"));
    assert!(result.content.contains("status: string"));
    assert!(result.content.contains("Missing required fields:"));
    assert!(result.content.contains("id, status"));
    assert!(result.content.contains("Unexpected fields:"));
    assert!(result.content.contains("todos"));
    assert!(result.content.contains("Use checklist_write"));
}

#[tokio::test]
async fn run_shell_command_op_requests_approval_and_executes_shell() {
    let (mut engine, handle) = Engine::new(EngineConfig::default(), &Config::default());
    let handle_for_approval = handle.clone();

    let task = tokio::spawn(async move {
        engine
            .handle_run_shell_command(
                "echo bang-ok".to_string(),
                AppMode::Agent,
                false,
                false,
                crate::tui::approval::ApprovalMode::Suggest,
            )
            .await;
    });

    let mut saw_started = false;
    let mut saw_approval = false;
    let mut saw_complete = false;
    let mut saw_turn_complete = false;
    let mut rx = handle.rx_event.write().await;
    while let Some(event) = rx.recv().await {
        match event {
            Event::TurnStarted { turn_id } => {
                assert!(turn_id.starts_with(USER_SHELL_TOOL_ID_PREFIX));
            }
            Event::ToolCallStarted { id, name, input } => {
                saw_started = true;
                assert!(id.starts_with(USER_SHELL_TOOL_ID_PREFIX));
                assert_eq!(name, "exec_shell");
                assert_eq!(input["command"], json!("echo bang-ok"));
                assert_eq!(input["source"], json!("user"));
            }
            Event::ApprovalRequired { id, tool_name, .. } => {
                saw_approval = true;
                assert!(id.starts_with(USER_SHELL_TOOL_ID_PREFIX));
                assert_eq!(tool_name, "exec_shell");
                handle_for_approval
                    .approve_tool_call(id)
                    .await
                    .expect("approve shell");
            }
            Event::ToolCallComplete { id, name, result } => {
                saw_complete = true;
                assert!(id.starts_with(USER_SHELL_TOOL_ID_PREFIX));
                assert_eq!(name, "exec_shell");
                let result = result.expect("shell result");
                assert!(result.success, "{result:?}");
                assert!(result.content.contains("bang-ok"), "{result:?}");
            }
            Event::TurnComplete { status, .. } => {
                saw_turn_complete = true;
                assert_eq!(status, TurnOutcomeStatus::Completed);
                break;
            }
            _ => {}
        }
    }
    drop(rx);
    task.await.expect("shell op task");

    assert!(saw_started);
    assert!(saw_approval);
    assert!(saw_complete);
    assert!(saw_turn_complete);
}

#[tokio::test]
async fn run_shell_command_op_skips_approval_when_auto_approved() {
    let (mut engine, handle) = Engine::new(EngineConfig::default(), &Config::default());

    engine
        .handle_run_shell_command(
            "echo bang-yolo".to_string(),
            AppMode::Yolo,
            true,
            true,
            crate::tui::approval::ApprovalMode::Auto,
        )
        .await;

    let mut saw_complete = false;
    let mut rx = handle.rx_event.write().await;
    while let Some(event) = rx.recv().await {
        match event {
            Event::ApprovalRequired { .. } => {
                panic!("auto-approved shell shortcut should not request approval");
            }
            Event::ToolCallComplete { result, .. } => {
                saw_complete = true;
                let result = result.expect("shell result");
                assert!(result.success, "{result:?}");
                assert!(result.content.contains("bang-yolo"), "{result:?}");
            }
            Event::TurnComplete { status, .. } => {
                assert_eq!(status, TurnOutcomeStatus::Completed);
                break;
            }
            _ => {}
        }
    }

    assert!(saw_complete);
}

#[tokio::test]
async fn run_shell_command_op_preserves_plan_mode_shell_block() {
    let (mut engine, handle) = Engine::new(EngineConfig::default(), &Config::default());

    engine
        .handle_run_shell_command(
            "echo blocked".to_string(),
            AppMode::Plan,
            false,
            false,
            crate::tui::approval::ApprovalMode::Suggest,
        )
        .await;

    let mut saw_complete = false;
    let mut saw_turn_complete = false;
    let mut rx = handle.rx_event.write().await;
    while let Some(event) = rx.recv().await {
        match event {
            Event::ApprovalRequired { .. } => {
                panic!("Plan mode shell should be blocked before approval");
            }
            Event::ToolCallComplete { name, result, .. } => {
                saw_complete = true;
                assert_eq!(name, "exec_shell");
                let err = result.expect_err("plan shell should fail");
                assert!(
                    err.to_string().contains("unavailable in Plan mode"),
                    "{err}"
                );
            }
            Event::TurnComplete { status, .. } => {
                saw_turn_complete = true;
                assert_eq!(status, TurnOutcomeStatus::Failed);
                break;
            }
            _ => {}
        }
    }

    assert!(saw_complete);
    assert!(saw_turn_complete);
}

#[test]
fn deferred_tool_preflight_skips_already_active_tools() {
    let mut tool = api_tool("deferred_tool");
    tool.defer_loading = Some(true);
    let catalog = vec![tool];
    let mut active = HashSet::from(["deferred_tool".to_string()]);

    assert!(
        preflight_requested_deferred_tool("deferred_tool", &json!({}), &catalog, &mut active,)
            .is_none(),
        "already active tools should execute normally"
    );
}

#[test]
fn turn_tool_registry_builder_keeps_plan_mode_read_only_for_files() {
    let (engine, _handle) = Engine::new(EngineConfig::default(), &Config::default());
    let registry = engine
        .build_turn_tool_registry_builder(
            AppMode::Plan,
            engine.config.todos.clone(),
            engine.config.plan_state.clone(),
        )
        .build(engine.build_tool_context(AppMode::Plan, false));

    assert!(registry.contains("read_file"));
    assert!(registry.contains("list_dir"));
    assert!(!registry.contains("write_file"));
    assert!(!registry.contains("edit_file"));
    assert!(!registry.contains("exec_shell"));
    assert!(!registry.contains("exec_shell_wait"));
    assert!(!registry.contains("exec_shell_interact"));
    assert!(!registry.contains("task_shell_start"));
    assert!(!registry.contains("task_create"));
    assert!(!registry.contains("task_gate_run"));
    assert!(!registry.contains("rlm"));
    assert!(!registry.contains("fim_edit"));
    assert!(registry.contains("update_plan"));
    assert!(registry.contains("create_goal"));
    assert!(registry.contains("get_goal"));
    assert!(registry.contains("update_goal"));
    assert!(registry.contains("task_list"));
    assert!(registry.contains("task_read"));
    assert!(registry.contains("handle_read"));
    let plan_state_tools = [
        "checklist_add",
        "checklist_update",
        "checklist_write",
        "todo_add",
        "todo_update",
        "todo_write",
        "update_plan",
    ];
    let mut write_or_exec_tools: Vec<String> = registry
        .all()
        .into_iter()
        .filter(|tool| !plan_state_tools.contains(&tool.name()))
        .filter(|tool| {
            let capabilities = tool.capabilities();
            capabilities.contains(&ToolCapability::WritesFiles)
                || capabilities.contains(&ToolCapability::ExecutesCode)
        })
        .map(|tool| tool.name().to_string())
        .collect();
    write_or_exec_tools.sort();
    assert!(
        write_or_exec_tools.is_empty(),
        "Plan mode must not register file-writing or code-execution tools: {write_or_exec_tools:?}"
    );
}

/// Plan mode toggle must not change the byte representation of the tool
/// catalog head. DeepSeek's KV prefix cache includes the tools array in
/// the immutable prefix; if toggling between Plan and Agent mode changes
/// the tool bytes, every mode switch forces a full re-prefill.
///
/// This test verifies two invariants:
/// 1. Building the catalog twice for the same mode produces identical bytes.
/// 2. The head of the catalog (non-deferred tools) preserves its order
///    when deferred tools are activated mid-session.
#[test]
fn plan_mode_toggle_preserves_catalog_byte_stability() {
    let always_load = HashSet::new();

    // Build catalog for Plan mode twice — must be byte-identical.
    let plan_native = vec![
        api_tool("read_file"),
        api_tool("list_dir"),
        api_tool("write_file"),
        api_tool("edit_file"),
        api_tool("exec_shell"),
    ];
    let plan_mcp = vec![api_tool("mcp_search"), api_tool("mcp_write")];

    let catalog_a = build_model_tool_catalog(
        plan_native.clone(),
        plan_mcp.clone(),
        AppMode::Plan,
        &always_load,
    );
    let catalog_b = build_model_tool_catalog(
        plan_native.clone(),
        plan_mcp.clone(),
        AppMode::Plan,
        &always_load,
    );

    let json_a = serde_json::to_string(&catalog_a).unwrap();
    let json_b = serde_json::to_string(&catalog_b).unwrap();
    assert_eq!(
        json_a, json_b,
        "building the catalog twice for Plan mode must produce identical bytes"
    );

    // Build catalog for Agent mode twice — must be byte-identical.
    let agent_catalog_a = build_model_tool_catalog(
        plan_native.clone(),
        plan_mcp.clone(),
        AppMode::Agent,
        &always_load,
    );
    let agent_catalog_b = build_model_tool_catalog(
        plan_native.clone(),
        plan_mcp.clone(),
        AppMode::Agent,
        &always_load,
    );

    let agent_json_a = serde_json::to_string(&agent_catalog_a).unwrap();
    let agent_json_b = serde_json::to_string(&agent_catalog_b).unwrap();
    assert_eq!(
        agent_json_a, agent_json_b,
        "building the catalog twice for Agent mode must produce identical bytes"
    );

    // Verify that the non-deferred tools that are common to both modes
    // appear in the same order. Plan mode excludes execution tools, but
    // the tools that are present in both modes must have stable ordering.
    let plan_names: Vec<&str> = catalog_a
        .iter()
        .filter(|t| !t.defer_loading.unwrap_or(false))
        .map(|t| t.name.as_str())
        .collect();
    let agent_names: Vec<&str> = agent_catalog_a
        .iter()
        .filter(|t| !t.defer_loading.unwrap_or(false))
        .map(|t| t.name.as_str())
        .collect();

    // The common prefix of non-deferred tools must be identical.
    let common_len = plan_names.len().min(agent_names.len());
    assert_eq!(
        &plan_names[..common_len],
        &agent_names[..common_len],
        "non-deferred tools common to Plan and Agent must appear in the same order"
    );

    // Verify that activating a deferred tool mid-session appends to the
    // tail without reordering the head.
    let mut tools_with_deferred = plan_native.clone();
    tools_with_deferred.push({
        let mut t = api_tool("deferred_search");
        t.defer_loading = Some(true);
        t
    });
    let catalog_with_deferred = build_model_tool_catalog(
        tools_with_deferred,
        plan_mcp.clone(),
        AppMode::Agent,
        &always_load,
    );

    // Activate the deferred tool.
    let mut active: HashSet<String> = catalog_with_deferred
        .iter()
        .filter(|t| !t.defer_loading.unwrap_or(false))
        .map(|t| t.name.clone())
        .collect();
    active.insert("deferred_search".to_string());

    let listed = active_tools_for_step(&catalog_with_deferred, &active, false);
    let listed_names: Vec<&str> = listed.iter().map(|t| t.name.as_str()).collect();

    // The head (non-deferred tools) must still be in their original order.
    let head_names: Vec<&str> = catalog_with_deferred
        .iter()
        .filter(|t| !t.defer_loading.unwrap_or(false))
        .map(|t| t.name.as_str())
        .collect();
    assert!(
        listed_names.starts_with(&head_names),
        "activating a deferred tool must not reorder the catalog head: \
         expected {head_names:?} as prefix, got {listed_names:?}"
    );
    // The deferred tool must be at the tail.
    assert_eq!(
        listed_names.last(),
        Some(&"deferred_search"),
        "deferred tool must be appended at the tail"
    );
}

#[test]
fn parent_turn_registry_includes_goal_tools_for_all_modes() {
    let (engine, _handle) = Engine::new(EngineConfig::default(), &Config::default());

    for mode in [AppMode::Plan, AppMode::Agent, AppMode::Yolo] {
        let registry = engine
            .build_turn_tool_registry_builder(
                mode,
                engine.config.todos.clone(),
                engine.config.plan_state.clone(),
            )
            .build(engine.build_tool_context(mode, false));

        for name in ["create_goal", "get_goal", "update_goal"] {
            assert!(
                registry.contains(name),
                "parent {mode:?} registry should expose {name}"
            );
        }
    }
}

#[test]
fn agent_mode_can_build_auto_approved_tool_context() {
    let (engine, _handle) = Engine::new(EngineConfig::default(), &Config::default());

    assert!(
        !engine
            .build_tool_context(AppMode::Agent, false)
            .auto_approve
    );
    assert!(engine.build_tool_context(AppMode::Agent, true).auto_approve);
    assert!(engine.build_tool_context(AppMode::Yolo, false).auto_approve);
}

#[test]
fn build_tool_context_uses_typed_shell_policy_per_mode() {
    let mut config = EngineConfig {
        allow_shell: true,
        ..EngineConfig::default()
    };
    let (engine, _handle) = Engine::new(config.clone(), &Config::default());

    // Plan mode is shell-free and exposes no shell tools.
    assert_eq!(
        engine.build_tool_context(AppMode::Plan, false).shell_policy,
        crate::worker_profile::ShellPolicy::None
    );
    assert_eq!(
        engine
            .build_tool_context(AppMode::Agent, false)
            .shell_policy,
        crate::worker_profile::ShellPolicy::Full
    );
    assert_eq!(
        engine.build_tool_context(AppMode::Yolo, false).shell_policy,
        crate::worker_profile::ShellPolicy::Full
    );

    config.allow_shell = false;
    let (engine, _handle) = Engine::new(config, &Config::default());
    assert_eq!(
        engine
            .build_tool_context(AppMode::Agent, false)
            .shell_policy,
        crate::worker_profile::ShellPolicy::None
    );
}

#[test]
fn agent_and_yolo_modes_elevate_shell_sandbox_to_allow_network() {
    // Regression for #273: the seatbelt-default policy denies all outbound
    // network (including DNS), which broke `curl`, `yt-dlp`, package managers,
    // and similar shell commands in Agent mode. Elevation must include
    // network access so the application-level NetworkPolicy stays the only
    // outbound boundary.
    let (engine, _handle) = Engine::new(EngineConfig::default(), &Config::default());

    let agent_ctx = engine.build_tool_context(AppMode::Agent, false);
    let agent_policy = agent_ctx
        .elevated_sandbox_policy
        .as_ref()
        .expect("Agent mode should elevate the sandbox policy");
    assert!(
        agent_policy.has_network_access(),
        "Agent mode must allow shell network access; got {agent_policy:?}",
    );

    let yolo_ctx = engine.build_tool_context(AppMode::Yolo, false);
    let yolo_policy = yolo_ctx
        .elevated_sandbox_policy
        .as_ref()
        .expect("Yolo mode should elevate the sandbox policy");
    assert!(yolo_policy.has_network_access());
    // v0.8.11: YOLO drops to DangerFullAccess (no sandbox) so the user
    // is not bounced through approval round-trips for legitimate
    // outside-workspace writes (package installs, sub-agent
    // workspaces, ~/.cache mutations, etc.). YOLO is opt-in and
    // already enables trust mode + auto-approve; the sandbox was the
    // last guardrail and contradicts the contract.
    assert!(
        matches!(yolo_policy, crate::sandbox::SandboxPolicy::DangerFullAccess),
        "Yolo mode must use DangerFullAccess (no sandbox); got {yolo_policy:?}",
    );

    // Plan mode (#1077): the sandbox must actually deny workspace writes.
    // The previous WorkspaceWrite-with-empty-network policy whitelisted the
    // workspace as writable, so `python -c "open('f','w').write('x')"`
    // mutated files inside the workspace despite Plan-mode's intent. Lock
    // it to ReadOnly: no writes anywhere, no network. The shell tool stays
    // exposed for read-only inspection (`ls`, `git log`, `grep`, …) and
    // the per-platform sandbox enforces the rest.
    let plan_ctx = engine.build_tool_context(AppMode::Plan, false);
    let plan_policy = plan_ctx
        .elevated_sandbox_policy
        .as_ref()
        .expect("Plan mode should make the shell sandbox policy explicit");
    assert!(
        matches!(plan_policy, crate::sandbox::SandboxPolicy::ReadOnly),
        "Plan mode must use ReadOnly sandbox to deny workspace writes (#1077); got {plan_policy:?}",
    );
    assert!(!plan_policy.has_network_access());
    assert!(!plan_policy.has_full_disk_write_access());
    assert!(
        plan_policy
            .get_writable_roots(&std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
            .is_empty(),
        "ReadOnly policy must enumerate zero writable roots; got {plan_policy:?}",
    );
    assert!(
        plan_ctx
            .shell_network_denied_hint
            .as_deref()
            .is_some_and(|hint| hint.contains("Plan mode") && hint.contains("read-only")),
    );
}

#[test]
fn sandbox_policy_for_mode_returns_correct_policy_per_mode() {
    use super::tool_setup::sandbox_policy_for_mode;
    use crate::sandbox::SandboxPolicy;

    let workspace = PathBuf::from("/tmp/example-workspace");

    // Plan: ReadOnly. The whole point of #1077.
    assert!(matches!(
        sandbox_policy_for_mode(AppMode::Plan, &workspace),
        SandboxPolicy::ReadOnly
    ));

    // Agent: WorkspaceWrite with workspace as writable root, network on.
    match sandbox_policy_for_mode(AppMode::Agent, &workspace) {
        SandboxPolicy::WorkspaceWrite {
            writable_roots,
            network_access,
            ..
        } => {
            assert_eq!(writable_roots, vec![workspace.clone()]);
            assert!(network_access, "Agent mode must allow shell network access");
        }
        other => panic!("Agent mode should be WorkspaceWrite; got {other:?}"),
    }

    // YOLO: DangerFullAccess.
    assert!(matches!(
        sandbox_policy_for_mode(AppMode::Yolo, &workspace),
        SandboxPolicy::DangerFullAccess
    ));
}

#[tokio::test]
async fn session_update_preserves_reasoning_tool_only_turn() {
    let (mut engine, handle) = Engine::new(EngineConfig::default(), &Config::default());
    let assistant = Message {
        role: "assistant".to_string(),
        content: vec![
            ContentBlock::Thinking {
                signature: None,
                thinking: "Need a tool before answering.".to_string(),
            },
            ContentBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "read_file".to_string(),
                input: json!({"path": "Cargo.toml"}),
                caller: None,
            },
        ],
    };

    engine.add_session_message(assistant.clone()).await;

    let event = {
        let mut rx = handle.rx_event.write().await;
        rx.recv().await.expect("session update event")
    };
    let Event::SessionUpdated { messages, .. } = event else {
        panic!("expected session update event");
    };

    assert_eq!(messages, vec![assistant]);
}

#[tokio::test]
async fn set_model_reloads_instruction_sources_and_updates_session_prompt() {
    let tmp = tempdir().expect("tempdir");
    let instructions = tmp.path().join("instructions.md");
    fs::write(&instructions, "FLASH_INSTRUCTIONS_MARKER").expect("write instructions");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        model: "deepseek-v4-flash".to_string(),
        instructions: vec![instructions.clone().into()],
        ..Default::default()
    };
    let (engine, handle) = Engine::new(config, &Config::default());
    fs::write(&instructions, "PRO_INSTRUCTIONS_MARKER").expect("rewrite instructions");

    let run = tokio::spawn(engine.run());
    handle
        .send(Op::SetModel {
            model: "deepseek-v4-pro".to_string(),
            mode: AppMode::Agent,
        })
        .await
        .expect("send set model");

    let (model, prompt) = {
        let mut rx = handle.rx_event.write().await;
        loop {
            let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
                .await
                .expect("session update after model switch")
                .expect("event");
            if let Event::SessionUpdated {
                model,
                system_prompt,
                ..
            } = event
            {
                let prompt = match system_prompt.expect("system prompt") {
                    SystemPrompt::Text(text) => text,
                    SystemPrompt::Blocks(blocks) => blocks
                        .into_iter()
                        .map(|block| block.text)
                        .collect::<Vec<_>>()
                        .join("\n"),
                };
                break (model, prompt);
            }
        }
    };
    run.abort();

    assert_eq!(model, "deepseek-v4-pro");
    assert!(prompt.contains("PRO_INSTRUCTIONS_MARKER"));
    assert!(!prompt.contains("FLASH_INSTRUCTIONS_MARKER"));
}

#[tokio::test]
async fn change_mode_refreshes_session_prompt_and_updates_session() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        model: "deepseek-v4-pro".to_string(),
        ..Default::default()
    };
    let (engine, handle) = Engine::new(config, &Config::default());

    let run = tokio::spawn(engine.run());
    handle
        .send(Op::ChangeMode {
            mode: AppMode::Yolo,
        })
        .await
        .expect("send change mode");

    let (_prompt, messages) = {
        let mut rx = handle.rx_event.write().await;
        loop {
            let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
                .await
                .expect("session update after mode switch")
                .expect("event");
            if let Event::SessionUpdated {
                system_prompt,
                messages,
                ..
            } = event
            {
                let prompt = match system_prompt.expect("system prompt") {
                    SystemPrompt::Text(text) => text,
                    SystemPrompt::Blocks(blocks) => blocks
                        .into_iter()
                        .map(|block| block.text)
                        .collect::<Vec<_>>()
                        .join("\n"),
                };
                break (prompt, messages);
            }
        }
    };
    run.abort();

    assert!(
        messages.iter().all(|message| message.role != "system"),
        "mode switch must not persist appended system messages: {messages:?}"
    );
}

#[test]
fn turn_approval_mode_prefers_auto_approve_flag() {
    use crate::tui::approval::ApprovalMode;

    assert_eq!(
        agent_approval_mode_for_turn(true, ApprovalMode::Suggest),
        ApprovalMode::Auto
    );
    assert_eq!(
        agent_approval_mode_for_turn(true, ApprovalMode::Never),
        ApprovalMode::Auto
    );
}

#[test]
fn messages_with_turn_metadata_returns_stored_session_messages() {
    use crate::tui::approval::ApprovalMode;

    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    engine.current_mode = AppMode::Plan;
    engine.session.approval_mode = ApprovalMode::Suggest;
    engine.session.messages = vec![Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: "summary after compaction".to_string(),
            cache_control: None,
        }],
    }]
    .into();
    let stored = engine.session.messages.clone();

    let request_messages = engine.messages_with_turn_metadata();

    assert_eq!(&*engine.session.messages, &*stored);
    assert_eq!(request_messages.len(), stored.len());
    assert!(
        request_messages
            .iter()
            .all(|message| message.role != "system"),
        "model request projection must not create appended system messages"
    );
}

#[tokio::test]
async fn change_mode_op_updates_current_mode_and_emits_status() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        model: "deepseek-v4-pro".to_string(),
        ..Default::default()
    };
    let (engine, handle) = Engine::new(config, &Config::default());

    let run = tokio::spawn(engine.run());
    handle
        .send(Op::ChangeMode {
            mode: AppMode::Yolo,
        })
        .await
        .expect("send change mode");

    // Expect a SessionUpdated event confirming the mode change.
    let mut rx = handle.rx_event.write().await;
    let session_updated = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("session update after mode switch")
        .expect("event");
    let Event::SessionUpdated { messages, .. } = session_updated else {
        panic!("should emit SessionUpdated after mode change, got: {session_updated:?}");
    };
    assert!(
        messages.iter().all(|message| message.role != "system"),
        "mode switch must not persist synthetic system messages: {messages:?}"
    );

    // Also expect a status event
    let status = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("status after mode switch")
        .expect("event");
    assert!(
        matches!(status, Event::Status { .. }),
        "should emit Status after mode change, got: {status:?}"
    );

    run.abort();
}

#[test]
fn detects_context_length_errors_from_provider_payloads() {
    let msg = r#"SSE stream request failed: HTTP 400 Bad Request: {"error":{"message":"This model's maximum context length is 131072 tokens. However, you requested 153056 tokens (148960 in the messages, 4096 in the completion).","type":"invalid_request_error"}}"#;
    assert!(is_context_length_error_message(msg));
    assert!(!is_context_length_error_message(
        "SSE stream request failed: HTTP 400 Bad Request: model not found"
    ));
}

#[test]
fn context_budget_reserves_output_and_headroom() {
    // Serialize with other tests that mutate DEEPSEEK_MAX_OUTPUT_TOKENS so
    // the internal effective_max_output_tokens() call sees a stable env.
    let _lock = lock_test_env();
    // V4 has a 1M context window — the only family that comfortably hosts
    // a 256K output reservation without saturating the input budget to 0.
    let budget = context_input_budget_for_provider(ApiProvider::Deepseek, "deepseek-v4-pro")
        .expect("deepseek-v4-pro should have a known context window");
    let v4_window: usize = 1_000_000;
    let expected = v4_window - (TURN_MAX_OUTPUT_TOKENS as usize) - 1_024usize;
    assert_eq!(budget, expected);
}

#[test]
fn context_budget_uses_conservative_fallback_for_unknown_models() {
    let _lock = lock_test_env();
    let budget = context_input_budget_for_provider(ApiProvider::Openai, "auto")
        .expect("unknown/auto model ids should still get a conservative hard preflight budget");
    let expected = 128_000usize - effective_max_output_tokens("auto") as usize - 1_024usize;
    assert_eq!(budget, expected);
}

#[test]
fn context_budget_uses_provider_effective_window_for_openai_codex() {
    let _lock = lock_test_env();
    let budget = context_input_budget_for_provider(ApiProvider::OpenaiCodex, "gpt-5.5")
        .expect("OpenAI Codex should use the route-effective context window");
    let expected = 400_000usize - effective_max_output_tokens("gpt-5.5") as usize - 1_024usize;
    assert_eq!(budget, expected);
}

#[test]
fn route_context_budget_uses_shared_budget_service() {
    let _lock = lock_test_env();
    let budget = route_context_budget_for_provider(ApiProvider::OpenaiCodex, "gpt-5.5", 380_000)
        .expect("OpenAI Codex should produce a route budget");

    assert_eq!(budget.window_tokens, 400_000);
    assert_eq!(
        budget.output_cap_tokens,
        u64::from(effective_max_output_tokens("gpt-5.5"))
    );
    assert_eq!(
        budget.pressure,
        crate::context_budget::PressureLevel::Critical
    );
    assert!(!budget.fits_additional(1));
}

#[test]
fn effective_max_output_tokens_caps_api_request_for_large_window_models() {
    // Serialize with other tests that mutate DEEPSEEK_MAX_OUTPUT_TOKENS so
    // v4_cap and flash_cap below see the same env state.
    let _lock = lock_test_env();
    // V4 models have a 1M context window but the API request cap must stay
    // well below common provider limits (e.g., 131K total on self-hosted
    // vLLM/SGLang). The cap should never exceed 65K.
    let v4_cap = effective_max_output_tokens("deepseek-v4-pro");
    assert!(
        v4_cap <= 65_536,
        "V4 API request cap should be ≤64K, got {v4_cap}"
    );
    assert!(
        v4_cap > 0,
        "V4 API request cap should be positive, got {v4_cap}"
    );

    let flash_cap = effective_max_output_tokens("deepseek-v4-flash");
    assert_eq!(v4_cap, flash_cap);
}

struct ScopedDeepSeekMaxOutputTokens {
    previous: Option<OsString>,
}

impl ScopedDeepSeekMaxOutputTokens {
    fn set(value: &str) -> Self {
        let previous = std::env::var_os("DEEPSEEK_MAX_OUTPUT_TOKENS");
        // Safety: tests using this helper serialize with lock_test_env() and
        // restore the original value in Drop.
        unsafe {
            std::env::set_var("DEEPSEEK_MAX_OUTPUT_TOKENS", value);
        }
        Self { previous }
    }

    fn unset() -> Self {
        let previous = std::env::var_os("DEEPSEEK_MAX_OUTPUT_TOKENS");
        // Safety: see set().
        unsafe {
            std::env::remove_var("DEEPSEEK_MAX_OUTPUT_TOKENS");
        }
        Self { previous }
    }
}

impl Drop for ScopedDeepSeekMaxOutputTokens {
    fn drop(&mut self) {
        // Safety: tests using this helper serialize with lock_test_env().
        unsafe {
            if let Some(previous) = self.previous.take() {
                std::env::set_var("DEEPSEEK_MAX_OUTPUT_TOKENS", previous);
            } else {
                std::env::remove_var("DEEPSEEK_MAX_OUTPUT_TOKENS");
            }
        }
    }
}

#[test]
fn effective_max_output_tokens_env_override_returns_positive_value() {
    let _lock = lock_test_env();
    let _guard = ScopedDeepSeekMaxOutputTokens::set("16384");

    // Override applies regardless of model — V4 hosted, V4 flash, sub-500K
    // self-hosted all return the env value verbatim.
    assert_eq!(effective_max_output_tokens("deepseek-v4-pro"), 16_384);
    assert_eq!(effective_max_output_tokens("deepseek-v4-flash"), 16_384);
    assert_eq!(effective_max_output_tokens("qwen3-32b-256k"), 16_384);
}

#[test]
fn effective_max_output_tokens_env_override_rejects_zero_and_invalid() {
    let _lock = lock_test_env();
    // Establish the heuristic baseline with the env unset.
    let baseline = {
        let _guard = ScopedDeepSeekMaxOutputTokens::unset();
        effective_max_output_tokens("deepseek-v4-pro")
    };
    assert!(baseline > 0);

    // 0, non-numeric, and empty values must all fall through to the heuristic
    // rather than producing a zero/garbage cap that would silently break
    // request budgeting.
    for raw in ["0", "abc", "", "  ", "-1"] {
        let _guard = ScopedDeepSeekMaxOutputTokens::set(raw);
        assert_eq!(
            effective_max_output_tokens("deepseek-v4-pro"),
            baseline,
            "env={raw:?} should fall through to heuristic"
        );
    }
}

#[test]
fn internal_context_budget_tiers_reserved_output_by_window() {
    // Serialize with other tests that mutate DEEPSEEK_MAX_OUTPUT_TOKENS so
    // both branches below see a stable env.
    let _lock = lock_test_env();
    // Large-context (>=500K) models reserve the full TURN_MAX_OUTPUT_TOKENS
    // headroom so long V4 sessions don't compact prematurely.
    let internal_budget =
        context_input_budget_for_provider(ApiProvider::Deepseek, "deepseek-v4-pro")
            .expect("V4 should have a known context window");
    let v4_window: usize = 1_000_000;
    let expected_internal = v4_window - (TURN_MAX_OUTPUT_TOKENS as usize) - 1_024usize;
    assert_eq!(internal_budget, expected_internal);

    // Sub-500K windows cross into the effective-cap branch: a 256K self-hosted
    // deployment must yield a usable positive budget rather than None. The
    // previous formula reserved the full 262K and computed 256K - 262K - 1K,
    // which underflowed to None and silently disabled preflight/recovery.
    let small_window_budget =
        context_input_budget_for_provider(ApiProvider::Openai, "qwen3-32b-256k")
            .expect("a 256K-suffix model must yield Some budget via the effective-cap branch");
    let effective_output = effective_max_output_tokens("qwen3-32b-256k") as usize;
    let expected_small = 256_000 - effective_output - 1_024;
    assert_eq!(small_window_budget, expected_small);
}

#[test]
fn v4_keeps_large_file_reads_but_compacts_noisy_shell_output() {
    let content = "0123456789abcdef\n".repeat(2_000);
    let output = ToolResult::success(content.clone());

    let v4_context = compact_tool_result_for_context("deepseek-v4-pro", "read_file", &output);
    assert_eq!(v4_context, content.trim());

    let v4_shell_context =
        compact_tool_result_for_context("deepseek-v4-pro", "exec_shell", &output);
    assert!(v4_shell_context.contains("exec_shell output compacted to protect context"));
    assert!(v4_shell_context.len() < v4_context.len());

    let legacy_context =
        compact_tool_result_for_context("deepseek-v3.2-128k", "read_file", &output);
    assert!(legacy_context.contains("output compacted to protect context"));
    assert!(legacy_context.len() < v4_context.len());
}

#[test]
fn subagent_results_are_summarized_before_parent_context_insertion() {
    let long_result = "verified detail\n".repeat(1_000);
    let output = ToolResult::success(
        json!({
            "agent_id": "agent_1234abcd",
            "agent_type": "explore",
            "assignment": {
                "objective": "Inspect the RLM rendering path and report the smallest fix."
            },
            "model": "deepseek-v4-flash",
            "status": "Completed",
            "result": long_result,
            "steps_taken": 12,
            "duration_ms": 3456
        })
        .to_string(),
    );

    let context = compact_tool_result_for_context("deepseek-v4-pro", "agent", &output);

    assert!(context.contains("[sub-agent result summarized for parent context]"));
    assert!(context.contains("agent_1234abcd (explore) status=Completed"));
    assert!(context.contains("Inspect the RLM rendering path"));
    assert!(context.contains("steps=12"));
    assert!(context.len() < output.content.len());
    assert!(context.contains("self-report"));
    assert!(context.contains("verify side effects"));
    assert!(context.contains("read_file") && context.contains("list_dir"));
    assert!(context.contains("handle_read"));
}

#[test]
fn run_verifiers_results_are_structured_before_context_insertion() {
    let noisy_failure = "node lint failure detail\n".repeat(300);
    let noisy_success = "successful check output\n".repeat(300);
    let output = ToolResult::success(
        json!({
            "success": false,
            "profile": "auto",
            "level": "quick",
            "workspace": "/repo",
            "gate_count": 3,
            "passed": 1,
            "failed": 1,
            "skipped": 1,
            "summary": "1 passed, 1 failed, 1 skipped",
            "gates": [
                {
                    "name": "rust-check",
                    "ecosystem": "rust",
                    "status": "passed",
                    "command": "cargo check --workspace --locked",
                    "cwd": "/repo",
                    "exit_code": 0,
                    "duration_ms": 110,
                    "stdout": noisy_success.clone(),
                    "stderr": "",
                    "stdout_truncated": false,
                    "stderr_truncated": false,
                    "skipped_reason": null
                },
                {
                    "name": "node-lint",
                    "ecosystem": "node",
                    "status": "failed",
                    "command": "npm run lint",
                    "cwd": "/repo",
                    "exit_code": 1,
                    "duration_ms": 220,
                    "stdout": "",
                    "stderr": noisy_failure,
                    "stdout_truncated": false,
                    "stderr_truncated": false,
                    "skipped_reason": null
                },
                {
                    "name": "python-pytest",
                    "ecosystem": "python",
                    "status": "skipped",
                    "command": "",
                    "cwd": "/repo",
                    "exit_code": null,
                    "duration_ms": 0,
                    "stdout": "",
                    "stderr": "",
                    "stdout_truncated": false,
                    "stderr_truncated": false,
                    "skipped_reason": "pytest is not installed"
                }
            ]
        })
        .to_string(),
    );

    let context = compact_tool_result_for_context("deepseek-v4-pro", "run_verifiers", &output);

    assert!(context.contains("[run_verifiers result summarized for context]"));
    assert!(context.contains("summary: 1 passed, 1 failed, 1 skipped"));
    assert!(context.contains("selection: profile=auto, level=quick"));
    assert!(context.contains("- node-lint (node): failed exit=1"));
    assert!(context.contains("command: npm run lint"));
    assert!(context.contains("- python-pytest (python): skipped"));
    assert!(context.contains("pytest is not installed"));
    assert!(context.contains("- rust-check (rust): passed exit=0"));
    assert!(context.len() < output.content.len());
    assert!(
        !context.contains(&noisy_success),
        "successful gate stdout should not be copied into parent context"
    );
}

#[test]
fn run_tests_results_are_structured_before_context_insertion() {
    let stdout = "running test suite\n".repeat(500);
    let stderr = "error[E0425]: cannot find value `missing`\n".repeat(500);
    let output = ToolResult::success(
        json!({
            "success": false,
            "exit_code": 101,
            "stdout": stdout,
            "stderr": stderr,
            "command": "(cd /repo && cargo test --workspace --all-features)"
        })
        .to_string(),
    );

    let context = compact_tool_result_for_context("deepseek-v4-pro", "run_tests", &output);

    assert!(context.contains("[run_tests result summarized for context]"));
    assert!(context.contains("status: failed, exit_code: 101"));
    assert!(context.contains("cargo test --workspace --all-features"));
    assert!(context.contains("error[E0425]"));
    assert!(context.contains("running test suite"));
    assert!(context.len() < output.content.len());
}

#[test]
fn task_gate_run_results_are_structured_before_context_insertion() {
    let output = ToolResult::success(
        json!({
            "gate": {
                "id": "gate_abcd1234",
                "gate": "clippy",
                "command": "cargo clippy -p codewhale-tui --all-targets --all-features --locked -- -D warnings",
                "cwd": "/repo",
                "exit_code": 1,
                "status": "failed",
                "classification": "compile_failure",
                "duration_ms": 5000,
                "summary": "warning promoted to error in verifier.rs",
                "log_path": "/repo/.codewhale/runtime/gate.log",
                "recorded_at": "2026-06-01T12:00:00Z"
            },
            "stdout_summary": "",
            "stderr_summary": "warning promoted to error"
        })
        .to_string(),
    );

    let context = compact_tool_result_for_context("deepseek-v4-pro", "task_gate_run", &output);

    assert!(context.contains("[task_gate_run result summarized for context]"));
    assert!(context.contains("gate: clippy, status: failed, exit_code: 1"));
    assert!(context.contains("cargo clippy -p codewhale-tui"));
    assert!(context.contains("summary: warning promoted to error"));
    assert!(context.contains("log_path: /repo/.codewhale/runtime/gate.log"));
}

#[test]
fn refresh_system_prompt_leaves_working_set_out_of_system_prompt() {
    let tmp = tempdir().expect("tempdir");
    fs::create_dir_all(tmp.path().join("src")).expect("mkdir");
    fs::write(tmp.path().join("src/lib.rs"), "pub fn sample() {}").expect("write");

    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    engine
        .session
        .working_set
        .observe_user_message("please inspect src/lib.rs", tmp.path());

    engine.refresh_system_prompt();

    let prompt = match &engine.session.system_prompt {
        Some(SystemPrompt::Text(text)) => text.clone(),
        Some(SystemPrompt::Blocks(blocks)) => blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        None => panic!("expected system prompt"),
    };
    assert!(!prompt.contains(WORKING_SET_SUMMARY_MARKER));
}

#[test]
fn working_set_reaches_model_as_turn_metadata() {
    let tmp = tempdir().expect("tempdir");
    fs::create_dir_all(tmp.path().join("src")).expect("mkdir");
    fs::write(tmp.path().join("src/lib.rs"), "pub fn sample() {}").expect("write");

    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    engine
        .session
        .working_set
        .observe_user_message("please inspect src/lib.rs", tmp.path());
    let user_msg =
        engine.user_text_message_with_turn_metadata("please inspect src/lib.rs".to_string());
    engine.session.add_message(user_msg);

    let messages = engine.messages_with_turn_metadata();
    let last_block = messages
        .first()
        .and_then(|message| message.content.last())
        .expect("turn metadata block");
    let ContentBlock::Text { text, .. } = last_block else {
        panic!("expected text metadata block");
    };
    assert!(text.starts_with("<turn_meta>\n"));
    assert!(text.contains(WORKING_SET_SUMMARY_MARKER));
    assert!(text.contains("src/lib.rs"));
}

#[test]
fn turn_metadata_includes_current_local_date_without_working_set() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        model: "deepseek-v4-flash".to_string(),
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    let user_msg = engine.user_text_message_with_turn_metadata("what is today's date?".to_string());
    engine.session.add_message(user_msg);

    let messages = engine.messages_with_turn_metadata();
    let last_block = messages
        .first()
        .and_then(|message| message.content.last())
        .expect("turn metadata block");
    let ContentBlock::Text { text, .. } = last_block else {
        panic!("expected text metadata block");
    };

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    assert!(text.starts_with("<turn_meta>\n"));
    assert!(text.contains(&format!("Current local date: {today}")));
    assert!(text.contains("Current model: deepseek-v4-flash"));
    assert!(text.contains("Input provenance: external_user"));
    assert!(text.contains("Input authority: external_current_turn"));
}

#[test]
fn runtime_turn_metadata_marks_non_authoritative_input() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (engine, _handle) = Engine::new(config, &Config::default());
    let msg = engine.runtime_text_message_with_turn_metadata(
        "改吧".to_string(),
        UserInputProvenance::AssistantGenerated,
    );
    let last_block = msg.content.last().expect("turn metadata block");
    let ContentBlock::Text { text, .. } = last_block else {
        panic!("expected text metadata block");
    };

    assert!(text.contains("Input provenance: assistant_generated"));
    assert!(text.contains("Input authority: non_authoritative"));
}

#[test]
fn turn_metadata_includes_auto_model_route() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (engine, _handle) = Engine::new(config, &Config::default());

    let user_msg = engine.user_text_message_with_turn_metadata_for_route(
        "debug this regression".to_string(),
        "deepseek-v4-pro",
        true,
        Some("max"),
        true,
    );
    let last_block = user_msg.content.last().expect("turn metadata block");
    let ContentBlock::Text { text, .. } = last_block else {
        panic!("expected text metadata block");
    };

    assert!(text.contains("Current model: deepseek-v4-pro"));
    assert!(text.contains("Auto model route: deepseek-v4-pro"));
    assert!(text.contains("Auto reasoning effort: max"));
    assert!(!text.contains("debug this regression"));
}

#[test]
fn non_external_provenance_cannot_inherit_yolo_auto_approval() {
    let policy = effective_input_policy(
        UserInputProvenance::SubAgentHandoff,
        AppMode::Yolo,
        "改吧",
        true,
        true,
        true,
        crate::tui::approval::ApprovalMode::Auto,
    );

    assert_eq!(policy.mode, AppMode::Agent);
    assert!(policy.allow_shell);
    assert!(!policy.trust_mode);
    assert!(!policy.auto_approve);
    assert_eq!(
        policy.approval_mode,
        crate::tui::approval::ApprovalMode::Suggest
    );
    assert!(
        policy
            .status
            .as_deref()
            .is_some_and(|status| status.contains("not external user input"))
    );
}

#[test]
fn review_only_external_input_keeps_explicit_mode_with_advisory_hint() {
    // Review-only wording must never silently override an explicitly chosen
    // mode (Yolo/Agent) or strip its tools. The heuristic should only surface
    // an advisory hint suggesting `/mode plan` for strict read-only tools.

    let agent = effective_input_policy(
        UserInputProvenance::ExternalUser,
        AppMode::Agent,
        "你在帮我看看 外卖部分还哪里没有使用多语言",
        true,
        true,
        true,
        crate::tui::approval::ApprovalMode::Auto,
    );
    assert_eq!(agent.mode, AppMode::Agent);
    assert!(agent.allow_shell);
    assert!(agent.trust_mode);
    assert!(agent.auto_approve);
    assert!(matches!(
        agent.approval_mode,
        crate::tui::approval::ApprovalMode::Auto
    ));
    assert!(agent.status.as_deref().is_some_and(|status| {
        status.contains("Keeping your current mode") && status.contains("/mode plan")
    }));

    let yolo = effective_input_policy(
        UserInputProvenance::ExternalUser,
        AppMode::Yolo,
        "check the failing tests and review the logs",
        true,
        true,
        true,
        crate::tui::approval::ApprovalMode::Auto,
    );
    assert_eq!(yolo.mode, AppMode::Yolo);
    assert!(yolo.allow_shell);
    assert!(yolo.trust_mode);
    assert!(yolo.auto_approve);
    assert!(matches!(
        yolo.approval_mode,
        crate::tui::approval::ApprovalMode::Auto
    ));
    assert!(yolo.status.as_deref().is_some_and(|status| {
        status.contains("Keeping your current mode") && status.contains("/mode plan")
    }));
}

#[test]
fn turn_metadata_omits_mode_policy() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (engine, _handle) = Engine::new(config, &Config::default());

    let user_msg = engine.user_text_message_with_turn_metadata_for_route(
        "test mode metadata".to_string(),
        "deepseek-v4-flash",
        false,
        None,
        false,
    );
    // turn_meta was relocated to the tail of the user message in #2517
    // to keep the leading bytes (user input) stable across date / model
    // route / working-set changes.
    let last_block = user_msg.content.last().expect("turn metadata block");
    let ContentBlock::Text { text, .. } = last_block else {
        panic!("expected text metadata block");
    };

    assert!(
        !text.contains("Current mode:"),
        "turn metadata should leave runtime policy to the capability tag, got: {text}"
    );
}

#[test]
fn current_mode_field_assignment_takes_effect_synchronously() {
    // Basic unit-level invariant: the current_mode field mutates as expected.
    // Op::ChangeMode dispatch through the run loop is exercised by the
    // integration test change_mode_op_updates_current_mode_and_emits_status.
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        model: "deepseek-v4-pro".to_string(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    assert_eq!(engine.current_mode, AppMode::Agent);

    engine.current_mode = AppMode::Yolo;
    assert_eq!(engine.current_mode, AppMode::Yolo);
}

#[test]
fn user_text_message_keeps_current_turn_input_after_turn_metadata() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (engine, _handle) = Engine::new(config, &Config::default());

    let user_msg =
        engine.user_text_message_with_turn_metadata("explain the cache metrics".to_string());

    // User text is now at position 0, turn_meta at position 1.
    let first_text = user_msg
        .content
        .iter()
        .find_map(|block| {
            if let ContentBlock::Text { text, .. } = block {
                Some(text.as_str())
            } else {
                None
            }
        })
        .expect("user text block");
    assert_eq!(first_text, "explain the cache metrics");
}

#[test]
fn messages_with_turn_metadata_preserves_stored_messages_for_prefix_cache() {
    let tmp = tempdir().expect("tempdir");
    fs::create_dir_all(tmp.path().join("src")).expect("mkdir");
    fs::write(tmp.path().join("src/lib.rs"), "pub fn sample() {}").expect("write");

    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    engine
        .session
        .working_set
        .observe_user_message("inspect src/lib.rs", tmp.path());

    let first_user = engine.user_text_message_with_turn_metadata("inspect src/lib.rs".to_string());
    engine.session.add_message(first_user.clone());
    let first_request = engine.messages_with_turn_metadata();
    assert_eq!(
        &first_request[..engine.session.messages.len()],
        &engine.session.messages[..]
    );
    assert_eq!(first_request.len(), engine.session.messages.len());
    assert_eq!(first_request.first(), Some(&first_user));
    assert_eq!(
        first_request.last().map(|message| message.role.as_str()),
        Some("user")
    );

    engine.session.add_message(Message {
        role: "assistant".to_string(),
        content: vec![ContentBlock::Text {
            text: "I inspected it.".to_string(),
            cache_control: None,
        }],
    });
    engine
        .session
        .working_set
        .observe_user_message("now summarize it", tmp.path());
    let second_user = engine.user_text_message_with_turn_metadata("now summarize it".to_string());
    engine.session.add_message(second_user);

    let second_request = engine.messages_with_turn_metadata();
    assert_eq!(
        &second_request[..engine.session.messages.len()],
        &engine.session.messages[..]
    );
    assert_eq!(second_request.len(), engine.session.messages.len());
    assert_eq!(second_request.first(), Some(&first_user));
    assert_eq!(second_request.last(), engine.session.messages.last());
}

/// v0.8.11 regression: tool-result messages serialize to role="tool" on
/// the wire but are stored as role="user" internally. `<turn_meta>` must
/// be stored only on actual user-text messages. Request-time runtime metadata
/// must not mutate tool-result messages.
#[test]
fn turn_metadata_skips_tool_result_messages() {
    let tmp = tempdir().expect("tempdir");
    fs::create_dir_all(tmp.path().join("src")).expect("mkdir");
    fs::write(tmp.path().join("src/lib.rs"), "pub fn sample() {}").expect("write");

    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    engine
        .session
        .working_set
        .observe_user_message("inspect src/lib.rs", tmp.path());

    // Real user message — should be eligible for injection.
    let user_msg = engine.user_text_message_with_turn_metadata("inspect src/lib.rs".to_string());
    engine.session.add_message(user_msg);
    // Assistant tool-call.
    engine.session.add_message(Message {
        role: "assistant".to_string(),
        content: vec![ContentBlock::ToolUse {
            id: "call_42".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({"path": "src/lib.rs"}),
            caller: None,
        }],
    });
    // Tool result, stored as role="user" internally.
    engine.session.add_message(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "call_42".to_string(),
            content: "pub fn sample() {}".to_string(),
            is_error: None,
            content_blocks: None,
        }],
    });

    let messages = engine.messages_with_turn_metadata();

    // The stored trailing message is the tool result and MUST be untouched —
    // no Text block sneaking in front of the ToolResult block.
    let trailing = messages.last().expect("stored trailing message");
    assert_eq!(trailing.role, "user");
    assert_eq!(trailing.content.len(), 1);
    assert!(matches!(
        trailing.content.first(),
        Some(ContentBlock::ToolResult { .. })
    ));

    // The earlier real user message carries user text first, turn_meta last.
    let real_user = messages.first().expect("first user message");
    assert_eq!(real_user.role, "user");
    let ContentBlock::Text { text, .. } = real_user.content.first().expect("user text content")
    else {
        panic!("expected Text block on real user message");
    };
    assert_eq!(text, "inspect src/lib.rs");
    // turn_meta is at the tail of the content array.
    let last_block = real_user.content.last().expect("turn_meta block");
    let ContentBlock::Text { text: meta, .. } = last_block else {
        panic!("expected Text block for turn_meta at tail");
    };
    assert!(meta.starts_with("<turn_meta>\n"));
    assert!(meta.contains("src/lib.rs"));
}

/// User text must appear before turn_meta in the content array so that
/// the leading bytes of each user message stay stable across date changes.
/// DeepSeek's KV prefix cache matches byte sequences from the start of
/// each message; placing the volatile date-bearing turn_meta at position
/// 0 would invalidate the entire user message prefix at every date
/// boundary. Moving it to the tail preserves the user-input prefix.
#[test]
fn user_message_turn_meta_is_appended_not_prepended() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (engine, _handle) = Engine::new(config, &Config::default());

    let msg = engine.user_text_message_with_turn_metadata("hello world".to_string());
    assert_eq!(msg.role, "user");
    assert_eq!(msg.content.len(), 2);

    // First content block: user text.
    let ContentBlock::Text { text, .. } = &msg.content[0] else {
        panic!("expected Text block at position 0");
    };
    assert_eq!(text, "hello world");

    // Second content block: turn_meta.
    let ContentBlock::Text { text: meta, .. } = &msg.content[1] else {
        panic!("expected Text block for turn_meta at position 1");
    };
    assert!(
        meta.starts_with("<turn_meta>\n"),
        "turn_meta must be at the tail"
    );
    assert!(
        meta.contains("Current local date:"),
        "turn_meta must contain the date"
    );
}

/// When the turn is mid-execution and the trailing user message is a
/// tool result, no turn_meta is injected into that tool-result message. The
/// working_set surfaces again on the next stored user-text message.
#[test]
fn turn_metadata_skips_when_only_tool_results_trail() {
    let tmp = tempdir().expect("tempdir");
    fs::create_dir_all(tmp.path().join("src")).expect("mkdir");
    fs::write(tmp.path().join("src/lib.rs"), "pub fn sample() {}").expect("write");

    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    engine
        .session
        .working_set
        .observe_user_message("inspect src/lib.rs", tmp.path());

    // Only a tool-result message in history — simulates the corner case
    // where the prior real user message has already been compacted away
    // but a tool-result is still pending. We must not retroactively
    // inject.
    engine.session.add_message(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "call_42".to_string(),
            content: "pub fn sample() {}".to_string(),
            is_error: None,
            content_blocks: None,
        }],
    });

    let messages = engine.messages_with_turn_metadata();

    // Stored tool-result message is unchanged: no Text prefix, content length == 1.
    let only = messages.first().expect("stored tool result message");
    assert_eq!(only.content.len(), 1);
    assert!(matches!(
        only.content.first(),
        Some(ContentBlock::ToolResult { .. })
    ));
    assert_eq!(messages.len(), 1);
}

#[test]
fn refresh_system_prompt_is_noop_when_unchanged() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());

    engine.refresh_system_prompt();
    let first_hash = engine.session.last_system_prompt_hash;
    let first_prompt = engine.session.system_prompt.clone();
    engine.refresh_system_prompt();

    assert_eq!(engine.session.last_system_prompt_hash, first_hash);
    assert_eq!(engine.session.system_prompt, first_prompt);
}

#[test]
fn engine_prompt_respects_hidden_thinking_config() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        locale_tag: "zh-Hans".to_string(),
        show_thinking: false,
        ..Default::default()
    };
    let (engine, _handle) = Engine::new(config, &Config::default());
    let prompt = match engine.session.system_prompt.as_ref() {
        Some(SystemPrompt::Text(text)) => text,
        Some(SystemPrompt::Blocks(_)) => panic!("expected text system prompt"),
        None => panic!("expected system prompt"),
    };

    assert!(prompt.contains("## Hidden Thinking Language"));
    assert!(prompt.contains("reasoning_content"));
    assert!(prompt.contains("English"));
    assert!(!prompt.contains("## 语言再次提醒"));
}

fn sync_runtime_system_prompt_override(engine: &mut Engine, system_prompt: SystemPrompt) {
    engine.session.compaction_summary_prompt =
        extract_compaction_summary_prompt(Some(system_prompt.clone()));
    engine.session.system_prompt = Some(system_prompt);
    engine.session.system_prompt_override = true;
}

#[test]
fn text_system_prompt_override_via_runtime_sync_survives_refresh() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    let prompt = SystemPrompt::Text("TANGERINE-7".to_string());
    let expected = Some(prompt.clone());

    sync_runtime_system_prompt_override(&mut engine, prompt);
    engine.refresh_system_prompt();

    assert_eq!(engine.session.system_prompt, expected);
}

#[test]
fn blocks_system_prompt_override_via_runtime_sync_survives_mode_change_refresh() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    let prompt = SystemPrompt::Blocks(vec![SystemBlock {
        block_type: "text".to_string(),
        text: "TANGERINE-7".to_string(),
        cache_control: None,
    }]);
    let expected = Some(prompt.clone());

    sync_runtime_system_prompt_override(&mut engine, prompt);
    engine.refresh_system_prompt();

    assert_eq!(engine.session.system_prompt, expected);
}

#[test]
fn compaction_summary_stays_in_stable_system_prompt() {
    let tmp = tempdir().expect("tempdir");
    fs::create_dir_all(tmp.path().join("src")).expect("mkdir");
    fs::write(tmp.path().join("src/main.rs"), "fn main() {}").expect("write");

    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    engine
        .session
        .working_set
        .observe_user_message("continue in src/main.rs", tmp.path());
    engine.refresh_system_prompt();
    engine.merge_compaction_summary(Some(SystemPrompt::Blocks(vec![SystemBlock {
        block_type: "text".to_string(),
        text: format!("{COMPACTION_SUMMARY_MARKER}\nsummary"),
        cache_control: None,
    }])));

    let prompt = match &engine.session.system_prompt {
        Some(SystemPrompt::Text(text)) => text.clone(),
        Some(SystemPrompt::Blocks(blocks)) => blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        None => panic!("expected system prompt"),
    };

    assert!(prompt.contains(COMPACTION_SUMMARY_MARKER));
    assert!(!prompt.contains(WORKING_SET_SUMMARY_MARKER));
}

#[test]
fn caller_policy_defaults_to_direct() {
    let tool = Tool {
        tool_type: None,
        name: "read_file".to_string(),
        description: "Read".to_string(),
        input_schema: json!({"type":"object"}),
        allowed_callers: Some(vec!["direct".to_string()]),
        defer_loading: Some(false),
        input_examples: None,
        strict: None,
        cache_control: None,
    };
    let direct = ToolCaller {
        caller_type: "direct".to_string(),
        tool_id: None,
    };
    let code = ToolCaller {
        caller_type: "code_execution_20250825".to_string(),
        tool_id: Some("srvtoolu_1".to_string()),
    };
    assert!(caller_allowed_for_tool(Some(&direct), Some(&tool)));
    assert!(!caller_allowed_for_tool(Some(&code), Some(&tool)));
    assert!(caller_allowed_for_tool(None, Some(&tool)));
}

#[test]
fn tool_search_activates_discovered_deferred_tools() {
    let mut catalog = vec![
        Tool {
            tool_type: None,
            name: "read_file".to_string(),
            description: "Read files".to_string(),
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"}}}),
            allowed_callers: Some(vec!["direct".to_string()]),
            defer_loading: Some(true),
            input_examples: None,
            strict: None,
            cache_control: None,
        },
        Tool {
            tool_type: None,
            name: "grep_files".to_string(),
            description: "Search files".to_string(),
            input_schema: json!({"type":"object","properties":{"pattern":{"type":"string"}}}),
            allowed_callers: Some(vec!["direct".to_string()]),
            defer_loading: Some(true),
            input_examples: None,
            strict: None,
            cache_control: None,
        },
    ];
    let always_load = HashSet::new();
    ensure_advanced_tooling(&mut catalog, AppMode::Agent, &always_load);
    let mut active = initial_active_tools(&catalog);
    let result = execute_tool_search(
        TOOL_SEARCH_BM25_NAME,
        &json!({"query":"read file"}),
        &catalog,
        &mut active,
    )
    .expect("search succeeds");
    assert!(result.success);
    assert!(active.contains("read_file"));
}

fn tool_search_catalog_with_matches(count: usize) -> Vec<Tool> {
    let mut catalog = (0..count)
        .map(|idx| Tool {
            tool_type: None,
            name: format!("matching_tool_{idx:03}"),
            description: "Matching deferred test tool".to_string(),
            input_schema: json!({"type":"object","properties":{"query":{"type":"string"}}}),
            allowed_callers: Some(vec!["direct".to_string()]),
            defer_loading: Some(true),
            input_examples: None,
            strict: None,
            cache_control: None,
        })
        .collect::<Vec<_>>();
    let always_load = HashSet::new();
    ensure_advanced_tooling(&mut catalog, AppMode::Agent, &always_load);
    catalog
}

fn tool_search_reference_count(result: &ToolResult) -> usize {
    result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("tool_references"))
        .and_then(|references| references.as_array())
        .map_or(0, Vec::len)
}

#[test]
fn tool_search_defaults_to_twenty_results_for_regex_and_bm25() {
    let catalog = tool_search_catalog_with_matches(25);

    for tool_name in [TOOL_SEARCH_REGEX_NAME, TOOL_SEARCH_BM25_NAME] {
        let mut active = initial_active_tools(&catalog);
        let result = execute_tool_search(
            tool_name,
            &json!({"query":"matching"}),
            &catalog,
            &mut active,
        )
        .expect("search succeeds");

        assert_eq!(tool_search_reference_count(&result), 20);
    }
}

#[test]
fn tool_search_respects_and_caps_max_results() {
    let catalog = tool_search_catalog_with_matches(120);

    let mut active = initial_active_tools(&catalog);
    let limited = execute_tool_search(
        TOOL_SEARCH_BM25_NAME,
        &json!({"query":"matching","max_results":7}),
        &catalog,
        &mut active,
    )
    .expect("search succeeds");
    assert_eq!(tool_search_reference_count(&limited), 7);

    let mut active = initial_active_tools(&catalog);
    let capped = execute_tool_search(
        TOOL_SEARCH_REGEX_NAME,
        &json!({"query":"matching","max_results":999}),
        &catalog,
        &mut active,
    )
    .expect("search succeeds");
    assert_eq!(tool_search_reference_count(&capped), 100);
}

#[test]
fn tool_search_schema_exposes_max_results_default_and_cap() {
    let mut catalog = Vec::new();
    let always_load = HashSet::new();
    ensure_advanced_tooling(&mut catalog, AppMode::Agent, &always_load);

    for tool_name in [TOOL_SEARCH_REGEX_NAME, TOOL_SEARCH_BM25_NAME] {
        let tool = catalog
            .iter()
            .find(|tool| tool.name == tool_name)
            .expect("tool search definition exists");
        let schema = &tool.input_schema["properties"]["max_results"];

        assert_eq!(schema["default"], 20);
        assert_eq!(schema["maximum"], 100);
        assert_eq!(schema["minimum"], 1);
    }
}

#[tokio::test]
async fn code_execution_runs_python_and_returns_result_payload() {
    let tmp = tempdir().expect("tempdir");
    let result =
        execute_code_execution_tool(&json!({"code":"print('hello from code exec')"}), tmp.path())
            .await
            .expect("code execution should run");
    assert!(result.content.contains("hello from code exec"));
    assert!(result.content.contains("return_code"));
}

#[test]
fn plan_mode_catalog_skips_code_execution_tool_but_agent_keeps_it() {
    let mut plan_catalog = vec![api_tool("read_file")];
    let always_load = HashSet::new();
    ensure_advanced_tooling(&mut plan_catalog, AppMode::Plan, &always_load);
    assert!(
        !plan_catalog
            .iter()
            .any(|tool| tool.name == CODE_EXECUTION_TOOL_NAME),
        "Plan mode must not expose code_execution"
    );

    let mut agent_catalog = vec![api_tool("read_file")];
    ensure_advanced_tooling(&mut agent_catalog, AppMode::Agent, &always_load);
    assert!(
        agent_catalog
            .iter()
            .any(|tool| tool.name == CODE_EXECUTION_TOOL_NAME),
        "Agent mode should still expose code_execution"
    );
}

#[test]
fn deferred_tool_requests_are_auto_activated() {
    use std::collections::HashSet;

    let catalog = vec![Tool {
        tool_type: None,
        name: "exec_shell".to_string(),
        description: "Run shell commands".to_string(),
        input_schema: json!({"type":"object","properties":{"cmd":{"type":"string"}}}),
        allowed_callers: Some(vec!["direct".to_string()]),
        defer_loading: Some(true),
        input_examples: None,
        strict: None,
        cache_control: None,
    }];

    let mut active = HashSet::new();
    assert!(!active.contains("exec_shell"));
    assert!(maybe_activate_requested_deferred_tool(
        "exec_shell",
        &catalog,
        &mut active
    ));
    assert!(active.contains("exec_shell"));
}

#[test]
fn missing_tool_error_message_offers_suggestions() {
    let catalog = vec![
        Tool {
            tool_type: None,
            name: "read_file".to_string(),
            description: "Read file contents".to_string(),
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"}}}),
            allowed_callers: Some(vec!["direct".to_string()]),
            defer_loading: Some(false),
            input_examples: None,
            strict: None,
            cache_control: None,
        },
        Tool {
            tool_type: None,
            name: "grep_files".to_string(),
            description: "Search file contents".to_string(),
            input_schema: json!({"type":"object","properties":{"pattern":{"type":"string"}}}),
            allowed_callers: Some(vec!["direct".to_string()]),
            defer_loading: Some(false),
            input_examples: None,
            strict: None,
            cache_control: None,
        },
    ];

    let message = missing_tool_error_message("reed_file", &catalog);
    assert!(message.contains("Did you mean:"));
    assert!(message.contains("read_file"));
    assert!(message.contains(TOOL_SEARCH_BM25_NAME));
}

#[test]
fn missing_tool_error_message_includes_discovery_guidance_when_no_match() {
    let catalog = vec![Tool {
        tool_type: None,
        name: "read_file".to_string(),
        description: "Read file contents".to_string(),
        input_schema: json!({"type":"object","properties":{"path":{"type":"string"}}}),
        allowed_callers: Some(vec!["direct".to_string()]),
        defer_loading: Some(false),
        input_examples: None,
        strict: None,
        cache_control: None,
    }];

    let message = missing_tool_error_message("totally_unknown_tool", &catalog);
    assert!(message.contains("not available in the current tool catalog"));
    assert!(message.contains(TOOL_SEARCH_BM25_NAME));
}

#[test]
fn missing_shell_tool_error_message_names_allow_shell_gate() {
    let catalog = vec![api_tool("read_file")];

    for tool_name in [
        "exec_shell",
        "exec_shell_wait",
        "exec_shell_interact",
        "task_shell_start",
        "task_shell_wait",
    ] {
        let message = missing_tool_error_message(tool_name, &catalog);
        assert!(message.contains("not available in the current tool catalog"));
        assert!(
            message.contains("allow_shell = false"),
            "{tool_name}: {message}"
        );
        assert!(message.contains("allow_shell"), "{tool_name}: {message}");
        assert!(
            message.contains("/config allow_shell true"),
            "{tool_name}: {message}"
        );
        assert!(message.contains("--save"), "{tool_name}: {message}");
        assert!(message.contains("Agent mode"), "{tool_name}: {message}");
        assert!(
            message.contains("approval gating"),
            "{tool_name}: {message}"
        );
        assert!(!message.contains("YOLO"), "{tool_name}: {message}");
        assert!(!message.contains("auto-approve"), "{tool_name}: {message}");
        assert!(
            message.contains(TOOL_SEARCH_BM25_NAME),
            "{tool_name}: {message}"
        );
    }
}

#[test]
fn missing_shell_tool_error_message_keeps_allow_shell_hint_with_suggestions() {
    let catalog = vec![api_tool("exec")];

    let message = missing_tool_error_message("exec_shell", &catalog);

    assert!(message.contains("Did you mean:"));
    assert!(message.contains("exec"));
    assert!(message.contains("allow_shell = false"));
    assert!(message.contains("allow_shell"));
    assert!(message.contains("/config allow_shell true"));
    assert!(message.contains("--save"));
    assert!(message.contains("Agent mode"));
    assert!(!message.contains("YOLO"));
    assert!(!message.contains("auto-approve"));
    assert!(message.contains(TOOL_SEARCH_BM25_NAME));
}

#[test]
fn filter_tool_call_delta_strips_bracket_marker() {
    let mut in_block = false;
    let visible = filter_tool_call_delta(
        "intro [TOOL_CALL]\n{\"tool\":\"x\"}\n[/TOOL_CALL] outro",
        &mut in_block,
    );
    assert!(!in_block);
    assert!(!visible.contains("[TOOL_CALL]"));
    assert!(!visible.contains("[/TOOL_CALL]"));
    assert!(!visible.contains("\"tool\":\"x\""));
    assert!(visible.contains("intro"));
    assert!(visible.contains("outro"));
}

#[test]
fn filter_tool_call_delta_strips_deepseek_xml_marker() {
    let mut in_block = false;
    let visible = filter_tool_call_delta(
        "before <codewhale:tool_call name=\"x\">payload</codewhale:tool_call> after",
        &mut in_block,
    );
    assert!(!in_block);
    for marker in TOOL_CALL_START_MARKERS {
        assert!(
            !visible.contains(marker),
            "visible text leaked start marker `{marker}`: {visible:?}"
        );
    }
    assert!(visible.contains("before"));
    assert!(visible.contains("after"));
}

#[test]
fn filter_tool_call_delta_strips_generic_tool_call_marker() {
    let mut in_block = false;
    let visible = filter_tool_call_delta(
        "lead <tool_call>\n{\"name\":\"do\"}\n</tool_call> tail",
        &mut in_block,
    );
    assert!(!in_block);
    assert!(!visible.contains("<tool_call"));
    assert!(!visible.contains("</tool_call>"));
    assert!(visible.contains("lead"));
    assert!(visible.contains("tail"));
}

#[test]
fn filter_tool_call_delta_strips_invoke_marker() {
    let mut in_block = false;
    let visible = filter_tool_call_delta(
        "alpha <invoke name=\"x\"><parameter name=\"k\">v</parameter></invoke> beta",
        &mut in_block,
    );
    assert!(!in_block);
    assert!(!visible.contains("<invoke "));
    assert!(!visible.contains("</invoke>"));
    assert!(visible.contains("alpha"));
    assert!(visible.contains("beta"));
}

#[test]
fn filter_tool_call_delta_strips_function_calls_marker() {
    let mut in_block = false;
    let visible = filter_tool_call_delta(
        "head <function_calls>\n{\"name\":\"x\"}\n</function_calls> tail",
        &mut in_block,
    );
    assert!(!in_block);
    assert!(!visible.contains("<function_calls>"));
    assert!(!visible.contains("</function_calls>"));
    assert!(visible.contains("head"));
    assert!(visible.contains("tail"));
}

#[test]
fn filter_tool_call_delta_handles_chunk_split_marker() {
    let mut in_block = false;
    // First chunk opens the wrapper but does not close it.
    let visible_a = filter_tool_call_delta("hello <tool_call>partial", &mut in_block);
    assert!(in_block, "filter must remember it is mid-wrapper");
    assert_eq!(visible_a, "hello ");

    // Second chunk continues inside the wrapper, then closes it and adds tail.
    let visible_b = filter_tool_call_delta("payload</tool_call> tail", &mut in_block);
    assert!(!in_block);
    assert_eq!(visible_b, " tail");
}

#[test]
fn filter_tool_call_delta_unmatched_open_suppresses_remainder() {
    let mut in_block = false;
    let visible = filter_tool_call_delta("ok [TOOL_CALL]rest of stream", &mut in_block);
    assert_eq!(visible, "ok ");
    assert!(
        in_block,
        "unmatched open must leave filter in tool-call mode"
    );
}

#[test]
fn filter_tool_call_delta_passes_through_clean_text() {
    let mut in_block = false;
    let input = "no markers here, just prose with code `<not a tag>`.";
    let visible = filter_tool_call_delta(input, &mut in_block);
    assert!(!in_block);
    assert_eq!(visible, input);
}

#[test]
fn contains_fake_tool_wrapper_detects_each_marker() {
    for marker in TOOL_CALL_START_MARKERS {
        let needle = format!("noise {marker} more noise");
        assert!(
            contains_fake_tool_wrapper(&needle),
            "marker `{marker}` should be detected"
        );
    }
}

#[test]
fn contains_fake_tool_wrapper_returns_false_on_clean_text() {
    assert!(!contains_fake_tool_wrapper(
        "plain assistant text without wrappers"
    ));
    assert!(!contains_fake_tool_wrapper(
        "`<tool` lookalike but not a real start marker"
    ));
}

#[test]
fn fake_wrapper_notice_is_compact_and_actionable() {
    // Keep this short so it fits cleanly in a single status line.
    assert!(FAKE_WRAPPER_NOTICE.len() < 120);
    assert!(FAKE_WRAPPER_NOTICE.contains("API tool channel"));
}

// ---- final_tool_input: bug-class regression for "<command>" placeholder ----
//
// Background: a streamed tool block carries its `input` in two pieces — an
// initial value at `ContentBlockStart` (often `{}`), then `InputJsonDelta`
// chunks that build up `input_buffer`. The TUI used to fire `ToolCallStarted`
// from `ContentBlockStart` with the empty initial input and never re-emit
// once args were known, so cells rendered the literal text `<command>` /
// `<file>` placeholders. The fix relocates the emission to `ContentBlockStop`
// and routes the input through `final_tool_input`, which prefers the parsed
// buffer over a stale empty placeholder.
fn tool_state(initial: serde_json::Value, buffer: &str) -> ToolUseState {
    ToolUseState {
        id: "t1".into(),
        name: "exec_shell".into(),
        input: initial,
        caller: None,
        input_buffer: buffer.into(),
    }
}

#[test]
fn final_tool_input_prefers_parsed_buffer_over_empty_initial() {
    // The exact regression: ContentBlockStart delivered `{}`, then args
    // streamed in via InputJsonDelta. The emitted ToolCallStarted must
    // carry the parsed buffer, not the placeholder.
    let state = tool_state(json!({}), r#"{"command": "ls -la"}"#);
    assert_eq!(final_tool_input(&state), json!({"command": "ls -la"}));
}

#[test]
fn final_tool_input_falls_back_to_initial_when_buffer_empty() {
    // Models occasionally embed args directly in the start frame and never
    // send any InputJsonDelta. We must still report those args.
    let state = tool_state(json!({"command": "echo hi"}), "");
    assert_eq!(final_tool_input(&state), json!({"command": "echo hi"}));
}

#[test]
fn final_tool_input_repairs_unparseable_buffer() {
    // The arg_repair module converts unparseable input to an empty object
    // {} so dispatch always proceeds. The buffer wins over the initial input.
    let state = tool_state(json!({"command": "echo hi"}), "{not json");
    assert_eq!(final_tool_input(&state), json!({}));
}

// === #103 transparent stream-retry policy =====================================

#[test]
fn stream_retry_zero_content_then_error_is_transparently_retried() {
    // Case 2 from issue #103: stream yielded ZERO content then errored.
    // The decoder hit Err on the very first poll → engine should retry
    // because DeepSeek hasn't billed and the user has seen nothing.
    assert!(
        super::should_transparently_retry_stream(false, 0, false),
        "first attempt with no content must be eligible for transparent retry"
    );
    assert!(
        super::should_transparently_retry_stream(false, 1, false),
        "second attempt (one prior retry) with no content must still be eligible"
    );
}

#[test]
fn stream_retry_after_content_received_surfaces_error() {
    // Case 3 from issue #103: stream yielded content then errored. We must
    // NOT transparently retry — the model has emitted billed output tokens
    // and the UI has streamed deltas; resending would double-bill and the
    // user would see the same prefix twice.
    assert!(
        !super::should_transparently_retry_stream(true, 0, false),
        "any content received → no transparent retry, even with full budget"
    );
    assert!(
        !super::should_transparently_retry_stream(true, 1, false),
        "any content received → no transparent retry on subsequent attempts"
    );
}

#[test]
fn stream_read_error_message_explains_retry_before_output() {
    let message = super::stream_read_error_user_message(
        "Stream read error: error decoding response body",
        false,
    );

    assert!(message.contains("Provider stream connection dropped"));
    assert!(message.contains("No output had streamed yet"));
    assert!(message.contains("retry automatically"));
    assert!(message.contains("Stream read error: error decoding response body"));
}

#[test]
fn stream_read_error_message_explains_no_replay_after_output() {
    let message = super::stream_read_error_user_message(
        "Stream read error: error decoding response body",
        true,
    );

    assert!(message.contains("Provider stream connection dropped"));
    assert!(message.contains("Some output had already streamed"));
    assert!(message.contains("risking duplicated output"));
    assert!(message.contains("Stream read error: error decoding response body"));
    assert_eq!(
        crate::error_taxonomy::classify_error_message(&message),
        crate::error_taxonomy::ErrorCategory::Network
    );
}

#[test]
fn stream_retry_budget_caps_transparent_retries_at_two() {
    // Case 4 from issue #103: after MAX_TRANSPARENT_STREAM_RETRIES attempts
    // we stop trying transparently and let the outer error path surface.
    // (The outer per-turn `stream_retry_attempts` retry is a separate layer
    // and is still in effect at the whole-turn level.)
    assert!(
        super::should_transparently_retry_stream(
            false,
            super::MAX_TRANSPARENT_STREAM_RETRIES - 1,
            false,
        ),
        "one short of the cap should still retry"
    );
    assert!(
        !super::should_transparently_retry_stream(
            false,
            super::MAX_TRANSPARENT_STREAM_RETRIES,
            false,
        ),
        "at the cap, no further transparent retries"
    );
    assert!(
        !super::should_transparently_retry_stream(
            false,
            super::MAX_TRANSPARENT_STREAM_RETRIES + 5,
            false,
        ),
        "well past the cap, definitely no transparent retries"
    );
}

#[test]
fn stream_retry_respects_cancellation() {
    // Cancellation overrides every other condition. If the user pressed
    // Esc / Ctrl-C, do not silently re-issue the request behind their back.
    assert!(
        !super::should_transparently_retry_stream(false, 0, true),
        "cancelled turn must not be transparently retried"
    );
    assert!(
        !super::should_transparently_retry_stream(false, 1, true),
        "cancelled turn must not be transparently retried even with budget"
    );
}

// === #2990 sleep-resume policy ================================================

#[test]
fn sleep_gap_requires_wallclock_to_outrun_monotonic_clock() {
    use std::time::Duration;
    // No divergence: ordinary network failure, clocks agree.
    assert!(
        !super::sleep_gap_detected(Duration::from_secs(30), Duration::from_secs(30)),
        "equal elapsed times must not register as a sleep gap"
    );
    // Divergence below the threshold: NTP slew / scheduling jitter.
    assert!(
        !super::sleep_gap_detected(Duration::from_secs(5), Duration::from_secs(14)),
        "9s of divergence is below the 10s threshold"
    );
    // Divergence above the threshold: the host was suspended.
    assert!(
        super::sleep_gap_detected(Duration::from_secs(5), Duration::from_secs(16)),
        "11s of divergence must register as a sleep gap"
    );
    // Wall clock went backwards (NTP step): saturating_sub → zero gap.
    assert!(
        !super::sleep_gap_detected(Duration::from_secs(60), Duration::from_secs(5)),
        "wall clock behind monotonic must never register as a sleep gap"
    );
}

#[test]
fn sleep_resume_retries_even_after_content_streamed() {
    // The whole point of #2990: unlike the #103 transparent retry, a
    // detected sleep gap retries regardless of streamed content — the
    // partial output predates the sleep and the user was not watching.
    assert!(
        super::should_resume_after_sleep(true, 0, false),
        "detected sleep with full budget must resume"
    );
    assert!(
        super::should_resume_after_sleep(true, super::MAX_STREAM_RETRIES - 1, false),
        "detected sleep one short of the budget must still resume"
    );
}

#[test]
fn sleep_resume_requires_a_detected_gap() {
    // Without a sleep gap this layer stays out of the way entirely, so the
    // deliberate no-retry-after-content policy for ordinary flakes (#103)
    // is preserved.
    assert!(
        !super::should_resume_after_sleep(false, 0, false),
        "no sleep gap → never resume via this layer"
    );
}

#[test]
fn sleep_resume_respects_budget_and_cancellation() {
    assert!(
        !super::should_resume_after_sleep(true, super::MAX_STREAM_RETRIES, false),
        "budget exhausted → surface the failure instead of looping"
    );
    assert!(
        !super::should_resume_after_sleep(true, 0, true),
        "cancelled turn must not be resumed behind the user's back"
    );
}

#[test]
fn stream_retry_threshold_relaxed_to_five() {
    // Case 1+4 from issue #103: the consecutive-error threshold for marking
    // the turn failed was relaxed from 3 → 5 in v0.6.7 because the new
    // HTTP/2 keepalive defaults make spurious decode errors rarer.
    // This test pins the constant so a future regression to 3 fails loudly.
    assert_eq!(
        super::MAX_STREAM_ERRORS_BEFORE_FAIL,
        5,
        "the consecutive-stream-error threshold should be 5; \
         lowering it back to 3 will fail mid-turn under transient flakiness"
    );
    // And a regression guard on the transparent-retry cap.
    assert_eq!(
        super::MAX_TRANSPARENT_STREAM_RETRIES,
        2,
        "transparent-retry cap should be 2; raising it risks hammering the \
         provider on real outages"
    );
}

// === Issue #66: error taxonomy wired through engine + audit + capacity ===

/// A failed-tool audit entry must carry the typed `category` and `severity`
/// fields derived from the underlying `ToolError`. This is what makes
/// downstream tooling able to bucket failures without scraping the message
/// string.
#[test]
fn tool_failure_audit_payload_carries_category_and_severity() {
    use crate::error_taxonomy::ErrorEnvelope;
    use crate::tools::spec::ToolError;

    let error = ToolError::Timeout { seconds: 30 };
    let envelope: ErrorEnvelope = error.clone().into();
    let payload = json!({
        "event": "tool.result",
        "tool_id": "tool-1",
        "tool_name": "exec_shell",
        "success": false,
        "error": error.to_string(),
        "category": envelope.category.to_string(),
        "severity": envelope.severity.to_string(),
    });

    assert_eq!(payload["category"], "timeout");
    assert_eq!(payload["severity"], "warning");
    assert_eq!(payload["success"], false);
}

// ── #136: post-edit LSP diagnostics hook ─────────────────────────────────

#[test]
fn edited_paths_for_edit_file_returns_path() {
    let input = json!({ "path": "src/foo.rs", "search": "x", "replace": "y" });
    let paths = edited_paths_for_tool("edit_file", &input);
    assert_eq!(paths, vec![PathBuf::from("src/foo.rs")]);
}

#[test]
fn edited_paths_for_write_file_returns_path() {
    let input = json!({ "path": "src/bar.rs", "content": "fn main() {}" });
    let paths = edited_paths_for_tool("write_file", &input);
    assert_eq!(paths, vec![PathBuf::from("src/bar.rs")]);
}

#[test]
fn edited_paths_for_apply_patch_with_changes_returns_each_path() {
    let input = json!({
        "changes": [
            { "path": "a.rs", "content": "" },
            { "path": "b.rs", "content": "" }
        ]
    });
    let paths = edited_paths_for_tool("apply_patch", &input);
    assert_eq!(paths, vec![PathBuf::from("a.rs"), PathBuf::from("b.rs")]);
}

#[test]
fn edited_paths_for_apply_patch_with_diff_text_extracts_paths() {
    let input = json!({
        "patch": "--- a/foo.rs\n+++ b/foo.rs\n@@ -1 +1 @@\n-let x: i32 = 0;\n+let x: i32 = \"oops\";\n"
    });
    let paths = edited_paths_for_tool("apply_patch", &input);
    assert_eq!(paths, vec![PathBuf::from("foo.rs")]);
}

#[test]
fn edited_paths_for_apply_patch_with_invalid_diff_returns_empty() {
    let input = json!({
        "patch": "@@ -1 +1 @@\n-old\n+new\n"
    });
    let paths = edited_paths_for_tool("apply_patch", &input);
    assert!(paths.is_empty());
}

#[test]
fn edited_paths_for_unknown_tool_returns_empty() {
    let input = json!({ "path": "irrelevant.rs" });
    let paths = edited_paths_for_tool("read_file", &input);
    assert!(paths.is_empty());
    let paths = edited_paths_for_tool("grep_files", &input);
    assert!(paths.is_empty());
}

#[test]
fn parse_patch_paths_skips_dev_null() {
    let patch = "--- a/keep.rs\n+++ b/keep.rs\n@@ -1 +1 @@\n-old\n+new\n--- a/deleted.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-delete me\n";
    let paths = edited_paths_for_tool("apply_patch", &json!({ "patch": patch }));
    assert_eq!(paths, vec![PathBuf::from("keep.rs")]);
}

#[tokio::test]
async fn post_edit_hook_injects_diagnostics_message_before_next_request() {
    use crate::lsp::{Diagnostic, Language, Severity};
    use std::sync::Arc;

    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().to_path_buf();
    let target = workspace.join("src").join("main.rs");
    fs::create_dir_all(workspace.join("src")).unwrap();
    fs::write(&target, "let x: i32 = \"not a number\";").unwrap();

    let lsp_config = crate::lsp::LspConfig::default();
    let engine_config = EngineConfig {
        workspace: workspace.clone(),
        lsp_config: Some(lsp_config),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(engine_config, &Config::default());

    // Install a fake transport that always reports a type error.
    let fake = Arc::new(crate::lsp::tests::FakeTransport::new(vec![Diagnostic {
        line: 1,
        column: 14,
        severity: Severity::Error,
        message: "expected i32, found &str".to_string(),
    }]));
    engine
        .lsp_manager
        .install_test_transport(Language::Rust, fake)
        .await;

    // Simulate the success path of an edit_file tool call.
    let input = json!({ "path": "src/main.rs", "search": "0", "replace": "\"not a number\"" });
    engine.run_post_edit_lsp_hook("edit_file", &input).await;
    assert_eq!(engine.pending_lsp_blocks.len(), 1);

    // Flush prepares the synthetic message.
    let messages_before = engine.session.messages.len();
    engine.flush_pending_lsp_diagnostics().await;
    assert_eq!(engine.session.messages.len(), messages_before + 1);

    let last = engine.session.messages.last().expect("message appended");
    assert_eq!(last.role, "user");
    // turn_meta is now at the tail of the content array (PR #2517).
    let meta = match last.content.last() {
        Some(crate::models::ContentBlock::Text { text, .. }) => text.clone(),
        other => panic!("expected text block at tail, got {other:?}"),
    };
    assert!(meta.starts_with("<turn_meta>\n"));
    let diagnostic_text = last
        .content
        .iter()
        .find_map(|block| match block {
            crate::models::ContentBlock::Text { text, .. }
                if text.contains("<diagnostics file=\"") =>
            {
                Some(text)
            }
            _ => None,
        })
        .expect("diagnostics text block");
    assert!(diagnostic_text.contains("ERROR [1:14] expected i32, found &str"));
}

#[tokio::test]
async fn post_edit_hook_is_silent_when_lsp_disabled() {
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().to_path_buf();
    let target = workspace.join("src").join("main.rs");
    fs::create_dir_all(workspace.join("src")).unwrap();
    fs::write(&target, "fn main() {}").unwrap();

    let lsp_config = crate::lsp::LspConfig {
        enabled: false,
        ..Default::default()
    };
    let engine_config = EngineConfig {
        workspace: workspace.clone(),
        lsp_config: Some(lsp_config),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(engine_config, &Config::default());

    let input = json!({ "path": "src/main.rs", "search": "x", "replace": "y" });
    engine.run_post_edit_lsp_hook("edit_file", &input).await;
    assert!(engine.pending_lsp_blocks.is_empty());

    let messages_before = engine.session.messages.len();
    engine.flush_pending_lsp_diagnostics().await;
    assert_eq!(engine.session.messages.len(), messages_before);
}

#[tokio::test]
async fn post_edit_hook_skips_unknown_tool_names() {
    use crate::lsp::{Diagnostic, Language, Severity};
    use std::sync::Arc;

    let tmp = tempdir().expect("tempdir");
    let engine_config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        lsp_config: Some(crate::lsp::LspConfig::default()),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(engine_config, &Config::default());
    let fake = Arc::new(crate::lsp::tests::FakeTransport::new(vec![Diagnostic {
        line: 1,
        column: 1,
        severity: Severity::Error,
        message: "should not be reported".to_string(),
    }]));
    engine
        .lsp_manager
        .install_test_transport(Language::Rust, fake.clone())
        .await;

    let input = json!({ "path": "src/main.rs" });
    engine.run_post_edit_lsp_hook("read_file", &input).await;
    assert!(engine.pending_lsp_blocks.is_empty());
    assert_eq!(fake.call_count(), 0);
}
