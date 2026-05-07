use super::*;

use crate::models::SystemBlock;
use crate::test_support::lock_test_env;
use serde_json::json;
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Instant;
use tempfile::tempdir;

const WORKING_SET_SUMMARY_MARKER: &str = "## Repo Working Set";
static CAPACITY_MEMORY_ENV_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

struct ScopedCapacityMemoryDir {
    previous: Option<OsString>,
}

impl ScopedCapacityMemoryDir {
    fn set(path: &Path) -> Self {
        let previous = std::env::var_os("DEEPSEEK_CAPACITY_MEMORY_DIR");
        // Safety: capacity-memory tests serialize access with CAPACITY_MEMORY_ENV_LOCK
        // and restore the original value in Drop.
        unsafe {
            std::env::set_var("DEEPSEEK_CAPACITY_MEMORY_DIR", path);
        }
        Self { previous }
    }
}

impl Drop for ScopedCapacityMemoryDir {
    fn drop(&mut self) {
        // Safety: capacity-memory tests serialize access with CAPACITY_MEMORY_ENV_LOCK.
        unsafe {
            if let Some(previous) = self.previous.take() {
                std::env::set_var("DEEPSEEK_CAPACITY_MEMORY_DIR", previous);
            } else {
                std::env::remove_var("DEEPSEEK_CAPACITY_MEMORY_DIR");
            }
        }
    }
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

fn build_engine_with_capacity(capacity: CapacityControllerConfig) -> Engine {
    let engine_config = EngineConfig {
        capacity,
        ..Default::default()
    };
    let (engine, _handle) = Engine::new(engine_config, &Config::default());
    engine
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
    assert!(message.contains("deepseek auth status"));
    assert!(message.contains("deepseek auth set --provider deepseek"));
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

fn make_plan(
    read_only: bool,
    supports_parallel: bool,
    approval_required: bool,
    interactive: bool,
) -> ToolExecutionPlan {
    ToolExecutionPlan {
        index: 0,
        id: "tool-1".to_string(),
        name: "grep_files".to_string(),
        input: json!({"pattern": "test"}),
        caller: None,
        interactive,
        approval_required,
        approval_description: "desc".to_string(),
        supports_parallel,
        read_only,
        blocked_error: None,
        guard_result: None,
    }
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
fn yolo_mode_keeps_tools_preloaded() {
    assert!(!should_default_defer_tool("exec_shell", AppMode::Yolo));
    assert!(!should_default_defer_tool(
        "mcp_read_resource",
        AppMode::Yolo
    ));
}

#[test]
fn non_yolo_mode_retains_default_defer_policy() {
    // Shell tools are kept loaded in action modes so the model can verify
    // work without an extra ToolSearch round-trip; non-action tools (e.g.
    // MCP) still defer.
    assert!(!should_default_defer_tool("exec_shell", AppMode::Agent));
    assert!(should_default_defer_tool("exec_shell", AppMode::Plan));
    assert!(!should_default_defer_tool("read_file", AppMode::Agent));
    assert!(should_default_defer_tool(
        "mcp_read_resource",
        AppMode::Agent
    ));
}

#[test]
fn model_tool_catalog_applies_native_and_mcp_deferral() {
    let catalog = build_model_tool_catalog(
        vec![
            api_tool("read_file"),
            api_tool("exec_shell"),
            api_tool("project_map"),
        ],
        vec![api_tool("list_mcp_resources"), api_tool("mcp_server_write")],
        AppMode::Agent,
    );

    let defer_loading = |name: &str| {
        catalog
            .iter()
            .find(|tool| tool.name == name)
            .and_then(|tool| tool.defer_loading)
    };

    assert_eq!(defer_loading("read_file"), Some(false));
    assert_eq!(defer_loading("exec_shell"), Some(false));
    assert_eq!(defer_loading("project_map"), Some(true));
    assert_eq!(defer_loading("list_mcp_resources"), Some(false));
    assert_eq!(defer_loading("mcp_server_write"), Some(true));
}

#[test]
fn model_tool_catalog_keeps_everything_loaded_in_yolo_mode() {
    let catalog = build_model_tool_catalog(
        vec![api_tool("project_map")],
        vec![api_tool("mcp_server_write")],
        AppMode::Yolo,
    );

    assert!(catalog.iter().all(|tool| tool.defer_loading == Some(false)));
}

#[test]
fn model_tool_catalog_sorts_each_partition_for_prefix_cache_stability() {
    // Regression for #263: deterministic byte order of the tools array is a
    // hard requirement for DeepSeek's KV prefix cache. Built-ins stay as a
    // contiguous prefix; MCP tools follow. Within each partition: alphabetical.
    let catalog = build_model_tool_catalog(
        vec![
            api_tool("read_file"),
            api_tool("apply_patch"),
            api_tool("exec_shell"),
        ],
        vec![api_tool("mcp_zoo_b"), api_tool("mcp_aardvark_a")],
        AppMode::Yolo,
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
    assert!(registry.contains("update_plan"));
    assert!(registry.contains("task_create"));
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

    // Plan mode can still expose shell tools for local read-only inspection,
    // but it keeps outbound network blocked and attaches an explicit hint so
    // network command failures are not mistaken for DNS/proxy issues.
    let plan_ctx = engine.build_tool_context(AppMode::Plan, false);
    let plan_policy = plan_ctx
        .elevated_sandbox_policy
        .as_ref()
        .expect("Plan mode should make the shell sandbox policy explicit");
    assert!(!plan_policy.has_network_access());
    assert!(
        plan_ctx
            .shell_network_denied_hint
            .as_deref()
            .is_some_and(|hint| hint.contains("Plan mode")),
    );
}

#[tokio::test]
async fn session_update_preserves_reasoning_tool_only_turn() {
    let (mut engine, handle) = Engine::new(EngineConfig::default(), &Config::default());
    let assistant = Message {
        role: "assistant".to_string(),
        content: vec![
            ContentBlock::Thinking {
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
    // V4 has a 1M context window — the only family that comfortably hosts
    // a 256K output reservation without saturating the input budget to 0.
    let budget = context_input_budget("deepseek-v4-pro", TURN_MAX_OUTPUT_TOKENS)
        .expect("deepseek-v4-pro should have a known context window");
    let v4_window: usize = 1_000_000;
    let expected = v4_window - (TURN_MAX_OUTPUT_TOKENS as usize) - 1_024usize;
    assert_eq!(budget, expected);
}

#[test]
fn effective_max_output_tokens_caps_api_request_for_large_window_models() {
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

#[test]
fn internal_context_budget_unaffected_by_api_request_cap() {
    // The internal context budget (used for compaction/preflight/recovery)
    // must still use the full TURN_MAX_OUTPUT_TOKENS headroom, NOT the
    // smaller API request cap. This ensures long-context V4 sessions don't
    // compact prematurely.
    let internal_budget = context_input_budget("deepseek-v4-pro", TURN_MAX_OUTPUT_TOKENS)
        .expect("V4 should have a known context window");
    let api_cap_budget = context_input_budget(
        "deepseek-v4-pro",
        effective_max_output_tokens("deepseek-v4-pro"),
    )
    .expect("V4 should have a known context window");

    // Internal budget reserves 262K for output; API-cap budget would only
    // reserve 64K. Internal budget must be smaller (more conservative).
    assert!(
        internal_budget < api_cap_budget,
        "Internal budget ({internal_budget}) should be smaller than API-cap budget ({api_cap_budget}) \
         because it reserves more headroom for output"
    );

    // Verify the internal budget is what the compaction logic actually uses.
    let v4_window: usize = 1_000_000;
    let expected_internal = v4_window - (TURN_MAX_OUTPUT_TOKENS as usize) - 1_024usize;
    assert_eq!(internal_budget, expected_internal);
}

#[test]
fn v4_tool_outputs_keep_large_file_reads_in_context() {
    let content = "0123456789abcdef\n".repeat(2_000);
    let output = ToolResult::success(content.clone());

    let v4_context = compact_tool_result_for_context("deepseek-v4-pro", "exec_shell", &output);
    assert_eq!(v4_context, content.trim());

    let legacy_context =
        compact_tool_result_for_context("deepseek-v3.2-128k", "exec_shell", &output);
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

    let context = compact_tool_result_for_context("deepseek-v4-pro", "agent_result", &output);

    assert!(context.contains("[sub-agent result summarized for parent context]"));
    assert!(context.contains("agent_1234abcd (explore) status=Completed"));
    assert!(context.contains("Inspect the RLM rendering path"));
    assert!(context.contains("steps=12"));
    assert!(context.len() < output.content.len());
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

    engine.refresh_system_prompt(AppMode::Agent);

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
    let first_block = messages
        .last()
        .and_then(|message| message.content.first())
        .expect("turn metadata block");
    let ContentBlock::Text { text, .. } = first_block else {
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
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());
    let user_msg = engine.user_text_message_with_turn_metadata("what is today's date?".to_string());
    engine.session.add_message(user_msg);

    let messages = engine.messages_with_turn_metadata();
    let first_block = messages
        .last()
        .and_then(|message| message.content.first())
        .expect("turn metadata block");
    let ContentBlock::Text { text, .. } = first_block else {
        panic!("expected text metadata block");
    };

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    assert!(text.starts_with("<turn_meta>\n"));
    assert!(text.contains(&format!("Current local date: {today}")));
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
    assert_eq!(first_request, engine.session.messages);

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
    assert_eq!(second_request, engine.session.messages);
    assert_eq!(second_request.first(), Some(&first_user));
}

/// v0.8.11 regression: tool-result messages serialize to role="tool" on
/// the wire but are stored as role="user" internally. `<turn_meta>` must
/// be stored only on actual user-text messages, not retroactively added
/// to tool-result messages at request time.
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

    // The trailing message is the tool result and MUST be untouched —
    // no Text block sneaking in front of the ToolResult block.
    let trailing = messages.last().expect("trailing message");
    assert_eq!(trailing.role, "user");
    assert_eq!(trailing.content.len(), 1);
    assert!(matches!(
        trailing.content.first(),
        Some(ContentBlock::ToolResult { .. })
    ));

    // The earlier real user message already carries the turn_meta prefix.
    let real_user = messages.first().expect("first user message");
    assert_eq!(real_user.role, "user");
    let ContentBlock::Text { text, .. } = real_user.content.first().expect("user text content")
    else {
        panic!("expected Text block on real user message");
    };
    assert!(text.starts_with("<turn_meta>\n"));
    assert!(text.contains("src/lib.rs"));
}

/// When the turn is mid-execution and the trailing user message is a
/// tool result, no turn_meta is injected at request time. The working_set
/// surfaces again on the next stored user-text message.
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

    // Returned unchanged: the single tool-result message, no Text
    // prefix, content length == 1.
    let only = messages.last().expect("trailing message");
    assert_eq!(only.content.len(), 1);
    assert!(matches!(
        only.content.first(),
        Some(ContentBlock::ToolResult { .. })
    ));
}

#[test]
fn refresh_system_prompt_is_noop_when_unchanged() {
    let tmp = tempdir().expect("tempdir");
    let config = EngineConfig {
        workspace: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let (mut engine, _handle) = Engine::new(config, &Config::default());

    engine.refresh_system_prompt(AppMode::Agent);
    let first_hash = engine.session.last_system_prompt_hash;
    let first_prompt = engine.session.system_prompt.clone();
    engine.refresh_system_prompt(AppMode::Agent);

    assert_eq!(engine.session.last_system_prompt_hash, first_hash);
    assert_eq!(engine.session.system_prompt, first_prompt);
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
    engine.refresh_system_prompt(AppMode::Agent);
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

#[tokio::test]
async fn pre_request_refresh_skips_compaction_below_normal_threshold() {
    let capacity = CapacityControllerConfig {
        enabled: true,
        low_risk_max: 0.0,
        medium_risk_max: 1.0,
        min_turns_before_guardrail: 0,
        ..Default::default()
    };

    let mut engine = build_engine_with_capacity(capacity.clone());
    engine.config.capacity = capacity.clone();
    engine.capacity_controller = CapacityController::new(capacity);
    engine.turn_counter = 5;
    engine
        .capacity_controller
        .mark_turn_start(engine.turn_counter);
    engine.session.model = "deepseek-v4-pro".to_string();
    engine.config.model = "deepseek-v4-pro".to_string();

    for i in 0..20 {
        engine.session.messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: format!("small message {i}"),
                cache_control: None,
            }],
        });
    }

    let before = engine.estimated_input_tokens();
    let before_len = engine.session.messages.len();
    let turn = TurnContext::new(10);
    let applied = engine
        .run_capacity_pre_request_checkpoint(&turn, None, AppMode::Agent)
        .await;
    let after = engine.estimated_input_tokens();

    assert!(!applied);
    assert_eq!(after, before);
    assert_eq!(engine.session.messages.len(), before_len);
}

#[tokio::test]
async fn pre_request_refresh_invoked_when_medium_risk() {
    let capacity = CapacityControllerConfig {
        enabled: true,
        low_risk_max: 0.0,
        medium_risk_max: 1.0,
        min_turns_before_guardrail: 0,
        ..Default::default()
    };

    let mut engine = build_engine_with_capacity(capacity.clone());
    engine.config.capacity = capacity.clone();
    engine.capacity_controller = CapacityController::new(capacity);
    engine.turn_counter = 5;
    engine
        .capacity_controller
        .mark_turn_start(engine.turn_counter);

    // Pin the model to an explicit 128k-context variant so the pressure ratio stays
    // stable regardless of changes to the workspace-wide default model.
    engine.session.model = "deepseek-v3.2-128k".to_string();
    engine.config.model = "deepseek-v3.2-128k".to_string();

    let long = "x".repeat(5_000);
    for _ in 0..900 {
        engine.session.messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: long.clone(),
                cache_control: None,
            }],
        });
    }

    let before = engine.estimated_input_tokens();
    let turn = TurnContext::new(10);
    let applied = engine
        .run_capacity_pre_request_checkpoint(&turn, None, AppMode::Agent)
        .await;
    let after = engine.estimated_input_tokens();

    assert!(applied);
    assert!(after < before);
}

#[tokio::test]
async fn post_tool_replay_invoked_when_high_non_severe_risk() {
    let tmp = tempdir().expect("tempdir");
    fs::write(tmp.path().join("sample.txt"), "hello replay").expect("write");

    let capacity = CapacityControllerConfig {
        enabled: true,
        low_risk_max: 0.0,
        medium_risk_max: 0.0,
        severe_min_slack: -10.0,
        severe_violation_ratio: 2.0,
        min_turns_before_guardrail: 0,
        ..Default::default()
    };

    let mut engine = build_engine_with_capacity(capacity.clone());
    engine.session.workspace = tmp.path().to_path_buf();
    engine.config.workspace = tmp.path().to_path_buf();
    engine.config.capacity = capacity.clone();
    engine.capacity_controller = CapacityController::new(capacity);
    engine.turn_counter = 4;
    engine
        .capacity_controller
        .mark_turn_start(engine.turn_counter);

    let mut turn = TurnContext::new(10);
    let mut tool_call = TurnToolCall::new(
        "tool_read_1".to_string(),
        "read_file".to_string(),
        json!({ "path": "sample.txt" }),
    );
    tool_call.set_result(
        "hello replay".to_string(),
        std::time::Duration::from_millis(1),
    );
    turn.record_tool_call(tool_call);

    let registry = ToolRegistryBuilder::new()
        .with_read_only_file_tools()
        .build(engine.build_tool_context(AppMode::Agent, false));

    let restarted = engine
        .run_capacity_post_tool_checkpoint(
            &turn,
            AppMode::Agent,
            Some(&registry),
            Arc::new(RwLock::new(())),
            None,
            0,
            0,
        )
        .await;

    assert!(!restarted);
    let has_verification_note = engine.session.messages.iter().any(|msg| {
        msg.content.iter().any(|block| match block {
            ContentBlock::ToolResult { content, .. } => content.contains("[verification replay]"),
            _ => false,
        })
    });
    assert!(has_verification_note);
}

#[tokio::test]
async fn error_escalation_triggers_replan_when_severe_or_repeated_failures() {
    let _env_lock = CAPACITY_MEMORY_ENV_LOCK.lock().await;
    let tmp = tempdir().expect("tempdir");
    let _env = ScopedCapacityMemoryDir::set(tmp.path());

    let capacity = CapacityControllerConfig {
        enabled: true,
        low_risk_max: 0.0,
        medium_risk_max: 0.0,
        min_turns_before_guardrail: 0,
        ..Default::default()
    };

    let mut engine = build_engine_with_capacity(capacity.clone());
    engine.config.capacity = capacity.clone();
    engine.capacity_controller = CapacityController::new(capacity);
    engine.turn_counter = 6;
    engine
        .capacity_controller
        .mark_turn_start(engine.turn_counter);

    for i in 0..10 {
        engine.session.messages.push(Message {
            role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
            content: vec![ContentBlock::Text {
                text: format!("noise message {i}"),
                cache_control: None,
            }],
        });
    }
    engine.session.messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: "Please finish task".to_string(),
            cache_control: None,
        }],
    });

    let before_len = engine.session.messages.len();
    let turn = TurnContext::new(10);
    let restarted = engine
        .run_capacity_error_escalation_checkpoint(&turn, AppMode::Agent, 2, 2, &[])
        .await;

    assert!(restarted);
    assert!(engine.session.messages.len() < before_len);
    assert!(engine.session.messages.len() <= 2);

    let records = load_last_k_capacity_records(&engine.session.id, 1).expect("load memory");
    assert!(!records.is_empty());
    assert!(!records[0].canonical_state.goal.is_empty());
}

/// v0.8.11: `CapacityControllerConfig::default()` ships with
/// `enabled = false`. The capacity controller's destructive
/// interventions (TargetedContextRefresh silently runs compaction;
/// VerifyAndReplan clears the session message log) silently rewrote
/// or nuked the user's transcript ("resetting plan" footer +
/// black-screen symptom). v0.8.11 commits to "trust the model with
/// the full 1M-token context, only compact on explicit user
/// /compact" — auto-managing the prefix contradicts that posture.
/// Power users can still opt in via `capacity.enabled = true`.
#[tokio::test]
async fn capacity_disabled_by_default_keeps_messages_intact() {
    let _env_lock = CAPACITY_MEMORY_ENV_LOCK.lock().await;
    let tmp = tempdir().expect("tempdir");
    let _env = ScopedCapacityMemoryDir::set(tmp.path());

    // Default config — what real users get.
    let mut engine = build_engine_with_capacity(CapacityControllerConfig::default());
    assert!(
        !engine.config.capacity.enabled,
        "capacity controller must be off by default in v0.8.11+"
    );
    engine.turn_counter = 6;
    engine
        .capacity_controller
        .mark_turn_start(engine.turn_counter);

    for i in 0..10 {
        engine.session.messages.push(Message {
            role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
            content: vec![ContentBlock::Text {
                text: format!("noise message {i}"),
                cache_control: None,
            }],
        });
    }
    engine.session.messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: "Please finish task".to_string(),
            cache_control: None,
        }],
    });

    let before_len = engine.session.messages.len();
    let turn = TurnContext::new(10);
    let restarted = engine
        .run_capacity_error_escalation_checkpoint(&turn, AppMode::Agent, 2, 2, &[])
        .await;

    // Capacity is disabled → no replan, no message clear.
    assert!(!restarted);
    assert_eq!(engine.session.messages.len(), before_len);
}

#[tokio::test]
async fn controller_disabled_keeps_behavior_unchanged() {
    let capacity = CapacityControllerConfig {
        enabled: false,
        ..Default::default()
    };

    let mut engine = build_engine_with_capacity(capacity.clone());
    engine.config.capacity = capacity.clone();
    engine.capacity_controller = CapacityController::new(capacity);
    engine.turn_counter = 3;
    engine
        .capacity_controller
        .mark_turn_start(engine.turn_counter);

    let long = "y".repeat(5_000);
    for _ in 0..120 {
        engine.session.messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: long.clone(),
                cache_control: None,
            }],
        });
    }

    let before = engine.estimated_input_tokens();
    let before_len = engine.session.messages.len();
    let turn = TurnContext::new(10);
    let applied = engine
        .run_capacity_pre_request_checkpoint(&turn, None, AppMode::Agent)
        .await;
    let after = engine.estimated_input_tokens();
    let after_len = engine.session.messages.len();

    assert!(!applied);
    assert_eq!(before, after);
    assert_eq!(before_len, after_len);
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
    ensure_advanced_tooling(&mut catalog);
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
        "before <deepseek:tool_call name=\"x\">payload</deepseek:tool_call> after",
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

/// Capacity escalation sees `ErrorCategory::InvalidInput` as a context-overflow
/// signal that must escalate even on the first failure (no consecutive
/// requirement). The previous string-matching path scanned the message for
/// "context length" — categories give us a typed contract instead.
#[test]
fn capacity_escalation_treats_invalid_input_as_overflow_signal() {
    use crate::error_taxonomy::ErrorCategory;

    // Replays the categorization branches inside
    // `run_capacity_error_escalation_checkpoint`. Keeping the assertions on
    // the typed surface (slice of `ErrorCategory`) means this test fails
    // loudly if a future refactor reverts to substring matching.
    let categories: &[ErrorCategory] = &[ErrorCategory::InvalidInput];
    let has_context_overflow = categories.contains(&ErrorCategory::InvalidInput);
    assert!(has_context_overflow);

    let only_transient = !categories.is_empty()
        && categories.iter().all(|c| {
            matches!(
                c,
                ErrorCategory::Network | ErrorCategory::RateLimit | ErrorCategory::Timeout
            )
        });
    assert!(!only_transient);
}

/// Transient categories (network / rate limit / timeout) must NOT escalate by
/// themselves — those resolve via the existing retry loop and shouldn't
/// trigger a capacity-driven replan.
#[test]
fn capacity_escalation_skips_pure_transient_categories() {
    use crate::error_taxonomy::ErrorCategory;

    let categories: &[ErrorCategory] = &[
        ErrorCategory::Network,
        ErrorCategory::RateLimit,
        ErrorCategory::Timeout,
    ];
    let has_context_overflow = categories.contains(&ErrorCategory::InvalidInput);
    assert!(!has_context_overflow);

    let only_transient = !categories.is_empty()
        && categories.iter().all(|c| {
            matches!(
                c,
                ErrorCategory::Network | ErrorCategory::RateLimit | ErrorCategory::Timeout
            )
        });
    assert!(only_transient);
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
fn edited_paths_for_apply_patch_with_files_returns_each_path() {
    let input = json!({
        "files": [
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
fn edited_paths_for_unknown_tool_returns_empty() {
    let input = json!({ "path": "irrelevant.rs" });
    let paths = edited_paths_for_tool("read_file", &input);
    assert!(paths.is_empty());
    let paths = edited_paths_for_tool("grep_files", &input);
    assert!(paths.is_empty());
}

#[test]
fn parse_patch_paths_skips_dev_null() {
    let patch = "--- a/keep.rs\n+++ b/keep.rs\n--- a/deleted.rs\n+++ /dev/null\n";
    let paths = parse_patch_paths(patch);
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
    let meta = match &last.content[0] {
        crate::models::ContentBlock::Text { text, .. } => text.clone(),
        other => panic!("expected text block, got {other:?}"),
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
