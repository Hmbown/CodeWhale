use super::cache::{cache, format_tokens, format_warmup_status};
use super::tokens::{context, cost, system_prompt, tokens};
use super::undo::{patch_undo, prune_undone_tool_context, retry, undo_conversation};
use crate::client::CacheWarmupKey;
use crate::config::Config;
use crate::models::{ContentBlock, Message, SystemBlock, SystemPrompt, Tool};
use crate::tui::app::{App, AppAction, TuiOptions, TurnCacheRecord};
use crate::tui::history::{GenericToolCell, HistoryCell, ToolCell, ToolStatus};
use std::path::PathBuf;
use std::time::Instant;

fn create_test_app() -> App {
    let options = TuiOptions {
        model: "deepseek-v4-pro".to_string(),
        workspace: PathBuf::from("/tmp/test-workspace"),
        config_path: None,
        config_profile: None,
        allow_shell: false,
        use_alt_screen: true,
        use_mouse_capture: false,
        use_bracketed_paste: true,
        max_subagents: 1,
        skills_dir: PathBuf::from("/tmp/test-skills"),
        memory_path: PathBuf::from("memory.md"),
        notes_path: PathBuf::from("notes.txt"),
        mcp_config_path: PathBuf::from("mcp.json"),
        use_memory: false,
        start_in_agent_mode: false,
        skip_onboarding: true,
        yolo: false,
        resume_session_id: None,
        initial_input: None,
    };
    let mut app = App::new(options, &Config::default());
    app.ui_locale = crate::localization::Locale::En;
    app.cost_currency = crate::pricing::CostCurrency::Usd;
    app.api_provider = crate::config::ApiProvider::Deepseek;
    app
}

fn test_tool(name: &str) -> Tool {
    Tool {
        tool_type: Some("function".to_string()),
        name: name.to_string(),
        description: format!("{name} test tool"),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"}
            }
        }),
        allowed_callers: None,
        defer_loading: Some(false),
        input_examples: None,
        strict: Some(true),
        cache_control: None,
    }
}

#[test]
fn test_tokens_shows_usage_info() {
    let mut app = create_test_app();
    app.session.total_tokens = 1234;
    app.session.session_cost = 0.05;
    app.session.last_prompt_tokens = Some(100);
    app.session.last_completion_tokens = Some(25);
    app.session.last_prompt_cache_hit_tokens = Some(70);
    app.session.last_prompt_cache_miss_tokens = Some(30);
    app.api_messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: "test".to_string(),
            cache_control: None,
        }],
    });
    app.history.push(HistoryCell::User {
        content: "test".to_string(),
    });

    let result = tokens(&mut app);
    assert!(result.message.is_some());
    let msg = result.message.unwrap();
    assert!(msg.contains("Token Usage"));
    assert!(msg.contains("Active context:"));
    assert!(msg.contains("Last API input:"));
    assert!(msg.contains("Last API output:"));
    assert!(msg.contains("Cache hit/miss:"));
    assert!(msg.contains("70 hit / 30 miss"));
    assert!(msg.contains("Cumulative tokens:"));
    assert!(msg.contains("Approx session cost:"));
    assert!(msg.contains("API messages:"));
    assert!(msg.contains("Chat messages:"));
    assert!(msg.contains("Model:"));
}

#[test]
fn tokens_report_uses_codex_oauth_route_context() {
    let mut app = create_test_app();
    app.api_provider = crate::config::ApiProvider::OpenaiCodex;
    app.set_model_selection("gpt-5.5".to_string());
    app.active_route_limits = Some(codewhale_config::route::RouteLimits {
        context_tokens: Some(272_000),
        input_tokens: None,
        output_tokens: None,
    });

    let message = tokens(&mut app).message.expect("tokens report");

    assert!(message.contains("/ 272000"), "{message}");
    assert!(!message.contains("1050000"), "{message}");
}

#[test]
fn test_cost_shows_spending_info() {
    let mut app = create_test_app();
    app.session.session_cost = 0.1234;
    let result = cost(&mut app);
    assert!(result.message.is_some());
    let msg = result.message.unwrap();
    assert!(msg.contains("Session Cost"));
    assert!(msg.contains("Approx total spent:"));
    assert!(msg.contains("approximate"));
    assert!(msg.contains("$0.1234"));
}

#[test]
fn test_system_prompt_displays_text() {
    let mut app = create_test_app();
    app.system_prompt = Some(SystemPrompt::Text("Test system prompt".to_string()));
    let result = system_prompt(&mut app);
    assert!(result.message.is_some());
    let msg = result.message.unwrap();
    assert!(msg.contains("System Prompt"));
    assert!(msg.contains("Test system prompt"));
}

#[test]
fn test_system_prompt_displays_blocks() {
    let mut app = create_test_app();
    app.system_prompt = Some(SystemPrompt::Blocks(vec![
        SystemBlock {
            block_type: "text".to_string(),
            text: "Block 1".to_string(),
            cache_control: None,
        },
        SystemBlock {
            block_type: "text".to_string(),
            text: "Block 2".to_string(),
            cache_control: None,
        },
    ]));
    let result = system_prompt(&mut app);
    assert!(result.message.is_some());
    let msg = result.message.unwrap();
    assert!(msg.contains("System Prompt"));
    assert!(msg.contains("Block 1"));
    assert!(msg.contains("Block 2"));
}

#[test]
fn test_system_prompt_none() {
    let mut app = create_test_app();
    app.system_prompt = None;
    let result = system_prompt(&mut app);
    assert!(result.message.is_some());
    let msg = result.message.unwrap();
    assert!(msg.contains("(no system prompt)"));
}

#[test]
fn test_system_prompt_truncates_long_text() {
    let mut app = create_test_app();
    let long_text = "x".repeat(600);
    app.system_prompt = Some(SystemPrompt::Text(long_text));
    let result = system_prompt(&mut app);
    assert!(result.message.is_some());
    let msg = result.message.unwrap();
    assert!(msg.contains("..."));
    assert!(msg.contains("chars total"));
}

#[test]
fn cache_command_reports_no_data_before_first_turn() {
    let mut app = create_test_app();
    let result = cache(&mut app, None);
    let msg = result.message.expect("cache produces a message");
    assert!(msg.contains("no turns recorded yet"), "got: {msg}");
}

#[test]
fn cache_inspect_reports_hashes_without_prompt_text() {
    let mut app = create_test_app();
    app.system_prompt = Some(SystemPrompt::Text(
            "Base policy\n\n<project_instructions source=\"AGENTS.md\">\nSECRET_PROJECT_RULE\n</project_instructions>"
                .to_string(),
        ));
    app.api_messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: "SECRET_USER_TASK".to_string(),
            cache_control: None,
        }],
    });

    let result = cache(&mut app, Some("inspect"));
    let msg = result.message.expect("inspect output");

    assert!(msg.contains("Cache Inspect"));
    assert!(msg.contains("Base static prefix hash:"));
    assert!(msg.contains("Full request prefix hash:"));
    assert!(msg.contains("Static base prefix stability: no previous request"));
    assert!(msg.contains("First divergence from previous request: unavailable"));
    assert!(msg.contains("Global system prefix: static"));
    assert!(msg.contains("Project context: static"));
    assert!(msg.contains("User task: dynamic"));
    assert!(!msg.contains("SECRET_PROJECT_RULE"));
    assert!(!msg.contains("SECRET_USER_TASK"));
}

#[test]
fn cache_inspect_uses_last_request_tool_catalog() {
    let mut app = create_test_app();
    app.system_prompt = Some(SystemPrompt::Text("Base policy".to_string()));
    app.session.last_tool_catalog = Some(vec![test_tool("read_file")]);
    app.api_messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: "Current task".to_string(),
            cache_control: None,
        }],
    });

    let msg = cache(&mut app, Some("inspect"))
        .message
        .expect("inspect output");

    assert!(msg.contains("Tool catalog hash: "), "got: {msg}");
    assert!(!msg.contains("(no tools registered)"), "got: {msg}");
    assert!(msg.contains("Tool catalog: static"), "got: {msg}");
    assert!(msg.contains("bytes="), "got: {msg}");
    assert!(msg.contains("~"), "got: {msg}");
}

#[test]
fn cache_inspect_json_reports_tool_catalog_hash_and_layer_sizes() {
    let mut app = create_test_app();
    app.system_prompt = Some(SystemPrompt::Text("Base policy".to_string()));
    app.session.last_tool_catalog = Some(vec![test_tool("read_file")]);
    app.api_messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: "Current task".to_string(),
            cache_control: None,
        }],
    });

    let msg = cache(&mut app, Some("inspect --json"))
        .message
        .expect("inspect json output");
    let parsed: serde_json::Value = serde_json::from_str(&msg).expect("valid json");

    assert_eq!(parsed["tool_catalog_hash"].as_str().unwrap().len(), 64);
    assert!(
        parsed["warmup_status"]
            .as_str()
            .is_some_and(|status| status.starts_with("Warmup status: no previous warmup"))
    );
    assert!(parsed["current_warmup_key"].is_object());
    let tool_layer = parsed["layers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|layer| layer["name"] == "Tool catalog")
        .expect("tool catalog layer");
    assert!(tool_layer["byte_len"].as_u64().unwrap() > 0);
    assert!(tool_layer["token_estimate"].as_u64().unwrap() > 0);
}

fn warmup_key(model: &str, static_hash: &str) -> CacheWarmupKey {
    CacheWarmupKey {
        provider: "Deepseek".to_string(),
        model: model.to_string(),
        base_url: "https://api.deepseek.com".to_string(),
        static_prefix_hash: static_hash.to_string(),
        tool_catalog_hash: "tool".to_string(),
        project_pack_hash: "project".to_string(),
        skills_hash: "skills".to_string(),
    }
}

#[test]
fn warmup_status_reports_valid_matching_key() {
    let key = warmup_key("deepseek-v4-pro", "static-a");
    let result = format_warmup_status(Some(&key), &key);
    assert!(result.contains("Warmup status: valid"), "got: {result}");
}

#[test]
fn warmup_status_reports_invalidation_reason() {
    let previous = warmup_key("deepseek-v4-pro", "static-a");
    let current = warmup_key("deepseek-v4-flash", "static-b");
    let result = format_warmup_status(Some(&previous), &current);
    assert!(result.contains("Warmup status: invalid"), "got: {result}");
    assert!(result.contains("model changed"), "got: {result}");
    assert!(result.contains("static prefix changed"), "got: {result}");
}

#[test]
fn warmup_status_reports_project_and_skills_reasons() {
    let previous = warmup_key("deepseek-v4-pro", "static-a");
    let mut current = previous.clone();
    current.project_pack_hash = "project-b".to_string();
    current.skills_hash = "skills-b".to_string();

    let result = format_warmup_status(Some(&previous), &current);

    assert!(result.contains("project pack changed"), "got: {result}");
    assert!(result.contains("skills changed"), "got: {result}");
    assert!(!result.contains("; )"), "got: {result}");
}

#[test]
fn cache_inspect_rejects_json_verbose_combo() {
    let mut app = create_test_app();
    let msg = cache(&mut app, Some("inspect --json --verbose"))
        .message
        .expect("inspect output");

    assert_eq!(
        msg,
        "cache inspect: --json and --verbose cannot be combined"
    );
}

#[test]
fn cache_inspect_json_uses_cjk_aware_token_estimate() {
    let mut app = create_test_app();
    app.system_prompt = Some(SystemPrompt::Text("缓存命中测试".to_string()));

    let msg = cache(&mut app, Some("inspect --json"))
        .message
        .expect("inspect json output");
    let parsed: serde_json::Value = serde_json::from_str(&msg).expect("valid json");
    let system_layer = parsed["layers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|layer| layer["name"] == "Global system prefix")
        .expect("system layer");

    assert_eq!(
        system_layer["token_estimate"].as_u64(),
        system_layer["char_len"].as_u64()
    );
}

#[test]
fn cache_inspect_reports_divergence_from_previous_request() {
    let mut app = create_test_app();
    app.system_prompt = Some(SystemPrompt::Text(
        "Base policy\n\n## Environment\n\n- shell: powershell".to_string(),
    ));
    app.api_messages.push(Message {
        role: "assistant".to_string(),
        content: vec![crate::models::ContentBlock::Text {
            text: "Prior answer".to_string(),
            cache_control: None,
        }],
    });
    app.api_messages.push(Message {
        role: "user".to_string(),
        content: vec![crate::models::ContentBlock::Text {
            text: "First task".to_string(),
            cache_control: None,
        }],
    });

    let first = cache(&mut app, Some("inspect"))
        .message
        .expect("first inspect output");
    assert!(first.contains("Static base prefix stability: no previous request"));

    if let Some(last) = app.api_messages.last_mut()
        && let Some(crate::models::ContentBlock::Text { text, .. }) = last.content.first_mut()
    {
        *text = "Second task".to_string();
    }

    let second = cache(&mut app, Some("inspect"))
        .message
        .expect("second inspect output");
    assert!(second.contains("Static base prefix stability: OK"));
    assert!(second.contains("First divergence from previous request: User task"));
    assert!(second.contains("Message #1 assistant: history"));
}

#[test]
fn cache_inspect_displays_tool_result_budget_metadata() {
    let mut app = create_test_app();
    let long_output = format!("{}{}", "A".repeat(7_000), "Z".repeat(7_000));
    app.api_messages.push(Message {
        role: "assistant".to_string(),
        content: vec![ContentBlock::ToolUse {
            id: "tool-1".to_string(),
            name: "shell_command".to_string(),
            input: serde_json::json!({"command": "cargo test"}),
            caller: None,
        }],
    });
    app.api_messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "tool-1".to_string(),
            content: long_output.clone(),
            is_error: None,
            content_blocks: None,
        }],
    });
    app.api_messages.push(Message {
        role: "assistant".to_string(),
        content: vec![ContentBlock::ToolUse {
            id: "tool-2".to_string(),
            name: "shell_command".to_string(),
            input: serde_json::json!({"command": "cargo test"}),
            caller: None,
        }],
    });
    app.api_messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "tool-2".to_string(),
            content: long_output,
            is_error: None,
            content_blocks: None,
        }],
    });

    let result = cache(&mut app, Some("inspect"));
    let msg = result.message.expect("inspect output");

    let tool_budget_lines: Vec<_> = msg
        .lines()
        .filter(|line| line.contains("original_chars=14000"))
        .collect();
    assert_eq!(tool_budget_lines.len(), 2, "got: {msg}");

    for sighting in tool_budget_lines {
        assert!(sighting.contains("sent_chars="), "got: {msg}");
        assert!(sighting.contains("truncated=true"), "got: {msg}");
        assert!(sighting.contains("deduplicated=false"), "got: {msg}");
    }
}

#[test]
fn cache_inspect_displays_turn_meta_dedup_metadata() {
    let mut app = create_test_app();
    let turn_meta = format!(
        "<turn_meta>\nCurrent local date: 2026-05-09\n{}\n</turn_meta>",
        "Working set: src/lib.rs\n".repeat(20)
    );
    app.api_messages.push(Message {
        role: "user".to_string(),
        content: vec![
            ContentBlock::Text {
                text: turn_meta.clone(),
                cache_control: None,
            },
            ContentBlock::Text {
                text: "first task".to_string(),
                cache_control: None,
            },
        ],
    });
    app.api_messages.push(Message {
        role: "user".to_string(),
        content: vec![
            ContentBlock::Text {
                text: turn_meta,
                cache_control: None,
            },
            ContentBlock::Text {
                text: "second task".to_string(),
                cache_control: None,
            },
        ],
    });

    let result = cache(&mut app, Some("inspect"));
    let msg = result.message.expect("inspect output");

    assert!(msg.contains("turn_meta_original_chars="), "got: {msg}");
    assert!(msg.contains("turn_meta_sent_chars="), "got: {msg}");
    assert!(msg.contains("turn_meta_deduplicated=false"), "got: {msg}");
    assert!(msg.contains("turn_meta_deduplicated=true"), "got: {msg}");
    assert!(msg.contains("turn_meta_sha256="), "got: {msg}");
    assert!(!msg.contains("Working set: src/lib.rs"), "got: {msg}");
}

#[test]
fn cache_command_renders_recorded_turns_with_ratio() {
    let mut app = create_test_app();
    let now = Instant::now();
    // Three turns: 75% hit, 50% hit, miss-only (provider didn't report hit).
    app.push_turn_cache_record(TurnCacheRecord {
        provider: Some(crate::config::ApiProvider::Deepseek),
        provider_identity: Some("deepseek".to_string()),
        model: Some("deepseek-v4-pro".to_string()),
        auto_model: true,
        input_tokens: 4_000,
        output_tokens: 200,
        cache_hit_tokens: Some(3_000),
        cache_miss_tokens: Some(1_000),
        reasoning_replay_tokens: None,
        recorded_at: now,
    });
    app.push_turn_cache_record(TurnCacheRecord {
        provider: None,
        provider_identity: None,
        model: None,
        auto_model: false,
        input_tokens: 6_000,
        output_tokens: 250,
        cache_hit_tokens: Some(3_000),
        cache_miss_tokens: Some(3_000),
        reasoning_replay_tokens: Some(150),
        recorded_at: now,
    });
    // Turn 3: hit reported but provider didn't report miss separately —
    // infer miss = input − hit and mark with `*`.
    app.push_turn_cache_record(TurnCacheRecord {
        provider: None,
        provider_identity: None,
        model: None,
        auto_model: false,
        input_tokens: 5_000,
        output_tokens: 100,
        cache_hit_tokens: Some(2_500),
        cache_miss_tokens: None,
        reasoning_replay_tokens: None,
        recorded_at: now,
    });
    // Turn 4: no telemetry at all — must not pollute aggregate ratios.
    app.push_turn_cache_record(TurnCacheRecord {
        provider: None,
        provider_identity: None,
        model: None,
        auto_model: false,
        input_tokens: 1_000,
        output_tokens: 50,
        cache_hit_tokens: None,
        cache_miss_tokens: None,
        reasoning_replay_tokens: None,
        recorded_at: now,
    });

    let result = cache(&mut app, None);
    let msg = result.message.expect("cache produces a message");

    // Header reflects total rows and model.
    assert!(msg.contains("last 4 of 4 turn(s)"), "got: {msg}");
    // Per-turn ratios are rendered.
    assert!(msg.contains("75.0%"), "got: {msg}");
    assert!(msg.contains("50.0%"), "got: {msg}");
    assert!(msg.contains("auto:deepseek/deepsee..."), "got: {msg}");
    // Turn 3: hit=2500, inferred miss=2500 → 50.0% with `*`-marked miss.
    assert!(msg.contains("2500*"), "got: {msg}");
    // Turn 4 (no telemetry) shows em-dashes and is excluded from totals.
    // Aggregate over turns 1-3: hit=8500, miss=6500 → 56.7%.
    assert!(msg.contains("avg hit ratio: 56.7%"), "got: {msg}");
    // Footer guidance is present.
    assert!(msg.contains("70%"), "got: {msg}");
}

#[test]
fn cache_command_replays_reported_1177_low_hit_fixture() {
    let mut app = create_test_app();
    let now = Instant::now();
    // Fixture from #1177 / douglarek's 2026-05-10 `/cache` report.
    // It captures a real low-hit sequence with one 56.8% tail turn.
    for (input, output, hit, miss) in [
        (25_839, 12, 4_608, 21_231),
        (25_906, 288, 25_728, 178),
        (264_500, 2_528, 235_648, 28_852),
        (202_230, 3_191, 193_536, 8_694),
        (45_982, 294, 26_112, 19_870),
    ] {
        app.push_turn_cache_record(TurnCacheRecord {
            provider: None,
            provider_identity: None,
            model: None,
            auto_model: false,
            input_tokens: input,
            output_tokens: output,
            cache_hit_tokens: Some(hit),
            cache_miss_tokens: Some(miss),
            reasoning_replay_tokens: None,
            recorded_at: now,
        });
    }

    let result = cache(&mut app, None);
    let msg = result.message.expect("cache produces a message");

    assert!(msg.contains("last 5 of 5 turn(s)"), "got: {msg}");
    assert!(msg.contains("56.8%"), "got: {msg}");
    assert!(msg.contains("Σ in: 564457"), "got: {msg}");
    assert!(msg.contains("Σ hit: 485632"), "got: {msg}");
    assert!(msg.contains("Σ miss: 78825"), "got: {msg}");
    assert!(msg.contains("avg hit ratio: 86.0%"), "got: {msg}");
}

#[test]
fn cache_command_count_argument_clamps_to_history() {
    let mut app = create_test_app();
    for _ in 0..3 {
        app.push_turn_cache_record(TurnCacheRecord {
            provider: None,
            provider_identity: None,
            model: None,
            auto_model: false,
            input_tokens: 1_000,
            output_tokens: 100,
            cache_hit_tokens: Some(500),
            cache_miss_tokens: Some(500),
            reasoning_replay_tokens: None,
            recorded_at: Instant::now(),
        });
    }
    let result = cache(&mut app, Some("100"));
    let msg = result.message.expect("cache produces a message");
    // Asked for 100 turns, only 3 exist — should report "last 3 of 3".
    assert!(msg.contains("last 3 of 3 turn(s)"), "got: {msg}");
}

#[test]
fn turn_cache_history_is_capped_at_50() {
    let mut app = create_test_app();
    for i in 0..(crate::tui::app::App::TURN_CACHE_HISTORY_CAP + 12) {
        app.push_turn_cache_record(TurnCacheRecord {
            provider: None,
            provider_identity: None,
            model: None,
            auto_model: false,
            input_tokens: i as u32,
            output_tokens: 1,
            cache_hit_tokens: Some(i as u32),
            cache_miss_tokens: Some(0),
            reasoning_replay_tokens: None,
            recorded_at: Instant::now(),
        });
    }
    assert_eq!(
        app.session.turn_cache_history.len(),
        crate::tui::app::App::TURN_CACHE_HISTORY_CAP
    );
    // Oldest record was evicted; newest record is still at the back.
    assert_eq!(
        app.session.turn_cache_history.back().unwrap().input_tokens,
        (crate::tui::app::App::TURN_CACHE_HISTORY_CAP + 11) as u32
    );
}

#[test]
fn test_context_shows_usage_stats() {
    let mut app = create_test_app();
    app.api_messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: "Hello".to_string(),
            cache_control: None,
        }],
    });
    app.history.push(HistoryCell::User {
        content: "Hello".to_string(),
    });

    let result = context(&mut app, None);
    assert!(matches!(
        result.action,
        Some(AppAction::OpenContextInspector)
    ));
    assert!(result.message.is_none());
}

#[test]
fn test_context_report_subcommands_return_source_map() {
    let mut app = create_test_app();
    app.api_messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: "Hello".to_string(),
            cache_control: None,
        }],
    });
    app.session.last_tool_catalog = Some(vec![test_tool("read_file")]);

    let report = context(&mut app, Some("report"))
        .message
        .expect("report text");
    assert!(report.contains("Context Source Map"));
    assert!(report.contains("Tool schemas"));

    let summary = context(&mut app, Some("summary"))
        .message
        .expect("summary text");
    assert!(summary.contains("Context Summary"));

    let json = context(&mut app, Some("json")).message.expect("json text");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid context json");
    assert!(!parsed["entries"].as_array().unwrap().is_empty());
}

#[test]
fn test_undo_conversation_removes_last_exchange() {
    let mut app = create_test_app();
    app.history.push(HistoryCell::User {
        content: "Hello".to_string(),
    });
    app.history.push(HistoryCell::Assistant {
        content: "Hi".to_string(),
        streaming: false,
    });
    app.api_messages.push(Message {
        role: "user".to_string(),
        content: vec![],
    });
    app.api_messages.push(Message {
        role: "assistant".to_string(),
        content: vec![],
    });

    let initial_history_len = app.history.len();
    let initial_api_len = app.api_messages.len();
    let result = undo_conversation(&mut app);

    assert!(result.message.is_some());
    let msg = result.message.unwrap();
    assert!(msg.contains("Removed"));
    assert!(app.history.len() < initial_history_len);
    assert!(app.api_messages.len() < initial_api_len);
}

#[test]
fn test_undo_conversation_nothing_to_undo() {
    let mut app = create_test_app();
    // Clear any default history
    app.history.clear();
    app.api_messages.clear();
    let result = undo_conversation(&mut app);
    assert!(result.message.is_some());
    let msg = result.message.unwrap();
    assert!(msg.contains("Nothing to undo") || msg.contains("Removed"));
}

#[test]
fn test_retry_with_previous_message() {
    let mut app = create_test_app();
    app.history.push(HistoryCell::User {
        content: "Test message".to_string(),
    });
    app.history.push(HistoryCell::Assistant {
        content: "Response".to_string(),
        streaming: false,
    });

    let result = retry(&mut app);
    assert!(result.message.is_some());
    let msg = result.message.unwrap();
    assert!(msg.contains("Retrying"));
    assert!(msg.contains("Test message"));
    assert!(matches!(result.action, Some(AppAction::SendMessage(_))));
}

#[test]
fn test_retry_no_previous_message() {
    let mut app = create_test_app();
    let result = retry(&mut app);
    assert!(result.message.is_some());
    let msg = result.message.unwrap();
    assert!(msg.contains("No previous request to retry"));
    assert!(result.action.is_none());
}

#[test]
fn test_retry_truncates_long_input() {
    let mut app = create_test_app();
    let long_input = "x".repeat(100);
    app.history.push(HistoryCell::User {
        content: long_input.clone(),
    });
    app.history.push(HistoryCell::Assistant {
        content: "Response".to_string(),
        streaming: false,
    });

    let result = retry(&mut app);
    assert!(result.message.is_some());
    let msg = result.message.unwrap();
    assert!(msg.contains("Retrying"));
    assert!(msg.contains("..."));
}

#[test]
fn test_patch_undo_requests_session_resync_after_restore() {
    use crate::snapshot::SnapshotRepo;
    use crate::test_support::lock_test_env;
    use tempfile::tempdir;

    struct HomeGuard {
        prev: Option<std::ffi::OsString>,
        _lock: crate::test_support::TestEnvLock,
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            // SAFETY: process-wide lock still held.
            unsafe {
                match self.prev.take() {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }

    fn scoped_home(home: &std::path::Path) -> HomeGuard {
        let lock = lock_test_env();
        let prev = std::env::var_os("HOME");
        // SAFETY: serialized by the global env lock.
        unsafe {
            std::env::set_var("HOME", home);
        }
        HomeGuard { prev, _lock: lock }
    }

    let tmp = tempdir().unwrap();
    let workspace = tmp.path().join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let _guard = scoped_home(tmp.path());

    let repo = SnapshotRepo::open_or_init(&workspace).unwrap();
    std::fs::write(workspace.join("a.txt"), b"original").unwrap();
    repo.snapshot("pre-turn:1").unwrap();
    std::fs::write(workspace.join("a.txt"), b"modified").unwrap();
    repo.snapshot("post-turn:1").unwrap();

    let mut app = create_test_app();
    app.workspace = workspace.clone();
    app.api_messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: "please edit a.txt".to_string(),
            cache_control: None,
        }],
    });

    let result = patch_undo(&mut app);

    assert!(!result.is_error);
    assert!(matches!(
        result.action,
        Some(AppAction::SyncSession {
            ref messages,
            ref workspace,
            ..
        }) if messages == &app.api_messages && workspace == &app.workspace
    ));
}

#[test]
fn test_patch_undo_walks_back_to_older_snapshot_on_repeat() {
    use crate::snapshot::SnapshotRepo;
    use crate::test_support::lock_test_env;
    use tempfile::tempdir;

    struct HomeGuard {
        prev: Option<std::ffi::OsString>,
        _lock: crate::test_support::TestEnvLock,
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            // SAFETY: process-wide lock still held.
            unsafe {
                match self.prev.take() {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }

    fn scoped_home(home: &std::path::Path) -> HomeGuard {
        let lock = lock_test_env();
        let prev = std::env::var_os("HOME");
        // SAFETY: serialized by the global env lock.
        unsafe {
            std::env::set_var("HOME", home);
        }
        HomeGuard { prev, _lock: lock }
    }

    let tmp = tempdir().unwrap();
    let workspace = tmp.path().join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let _guard = scoped_home(tmp.path());

    let repo = SnapshotRepo::open_or_init(&workspace).unwrap();
    let file = workspace.join("a.txt");
    std::fs::write(&file, b"zero").unwrap();
    repo.snapshot("tool:first").unwrap();
    std::fs::write(&file, b"one").unwrap();
    repo.snapshot("tool:second").unwrap();
    std::fs::write(&file, b"two").unwrap();

    let mut app = create_test_app();
    app.workspace = workspace.clone();

    let first = patch_undo(&mut app);
    assert!(!first.is_error);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "one");

    let second = patch_undo(&mut app);
    assert!(!second.is_error);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "zero");
}

#[test]
fn test_patch_undo_prunes_tool_turn_context() {
    use crate::snapshot::SnapshotRepo;
    use crate::test_support::lock_test_env;
    use tempfile::tempdir;

    struct HomeGuard {
        prev: Option<std::ffi::OsString>,
        _lock: crate::test_support::TestEnvLock,
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            // SAFETY: process-wide lock still held.
            unsafe {
                match self.prev.take() {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }

    fn scoped_home(home: &std::path::Path) -> HomeGuard {
        let lock = lock_test_env();
        let prev = std::env::var_os("HOME");
        // SAFETY: serialized by the global env lock.
        unsafe {
            std::env::set_var("HOME", home);
        }
        HomeGuard { prev, _lock: lock }
    }

    let tmp = tempdir().unwrap();
    let workspace = tmp.path().join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let _guard = scoped_home(tmp.path());

    let repo = SnapshotRepo::open_or_init(&workspace).unwrap();
    let file = workspace.join("a.txt");
    std::fs::write(&file, b"alpha").unwrap();
    repo.snapshot("tool:call-1").unwrap();
    std::fs::write(&file, b"alpha-fixed").unwrap();

    let mut app = create_test_app();
    app.workspace = workspace.clone();
    app.history.push(HistoryCell::User {
        content: "please edit a.txt".to_string(),
    });
    app.history.push(HistoryCell::Assistant {
        content: "I will update the file.".to_string(),
        streaming: false,
    });
    app.history
        .push(HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "write_file".to_string(),
            status: ToolStatus::Success,
            input_summary: Some("a.txt".to_string()),
            output: Some("updated".to_string()),
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        })));
    app.history.push(HistoryCell::Assistant {
        content: "Done, file is fixed now.".to_string(),
        streaming: false,
    });
    app.tool_cells.insert("call-1".to_string(), 2);

    app.api_messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: "please edit a.txt".to_string(),
            cache_control: None,
        }],
    });
    app.api_messages.push(Message {
        role: "assistant".to_string(),
        content: vec![
            ContentBlock::Text {
                text: "I will update the file.".to_string(),
                cache_control: None,
            },
            ContentBlock::ToolUse {
                id: "call-1".to_string(),
                name: "write_file".to_string(),
                input: serde_json::json!({"path": "a.txt"}),
                caller: None,
            },
        ],
    });
    app.api_messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "call-1".to_string(),
            content: "updated".to_string(),
            is_error: None,
            content_blocks: None,
        }],
    });
    app.api_messages.push(Message {
        role: "assistant".to_string(),
        content: vec![ContentBlock::Text {
            text: "Done, file is fixed now.".to_string(),
            cache_control: None,
        }],
    });

    let result = patch_undo(&mut app);

    assert!(!result.is_error);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "alpha");
    assert_eq!(app.history.len(), 3);
    assert!(matches!(
        app.history.last(),
        Some(HistoryCell::System { content }) if content.contains("/undo reverted workspace")
    ));
    assert_eq!(app.api_messages.len(), 2);
    assert!(matches!(
        &app.api_messages[0].content[0],
        ContentBlock::Text { text, .. } if text == "please edit a.txt"
    ));
    assert_eq!(app.api_messages[1].content.len(), 1);
    assert!(matches!(
        &app.api_messages[1].content[0],
        ContentBlock::Text { text, .. } if text == "I will update the file."
    ));
}

#[test]
fn test_patch_undo_prunes_pre_turn_context() {
    use crate::snapshot::SnapshotRepo;
    use crate::test_support::lock_test_env;
    use tempfile::tempdir;

    struct HomeGuard {
        prev: Option<std::ffi::OsString>,
        _lock: crate::test_support::TestEnvLock,
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            // SAFETY: process-wide lock still held.
            unsafe {
                match self.prev.take() {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }

    fn scoped_home(home: &std::path::Path) -> HomeGuard {
        let lock = lock_test_env();
        let prev = std::env::var_os("HOME");
        // SAFETY: serialized by the global env lock.
        unsafe {
            std::env::set_var("HOME", home);
        }
        HomeGuard { prev, _lock: lock }
    }

    let tmp = tempdir().unwrap();
    let workspace = tmp.path().join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let _guard = scoped_home(tmp.path());

    let repo = SnapshotRepo::open_or_init(&workspace).unwrap();
    let file = workspace.join("a.txt");
    std::fs::write(&file, b"alpha").unwrap();
    repo.snapshot("pre-turn:1").unwrap();
    std::fs::write(&file, b"alpha-fixed").unwrap();

    let mut app = create_test_app();
    app.workspace = workspace.clone();
    app.history.push(HistoryCell::User {
        content: "please edit a.txt".to_string(),
    });
    app.history.push(HistoryCell::Assistant {
        content: "Done, file is fixed now.".to_string(),
        streaming: false,
    });
    app.api_messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: "please edit a.txt".to_string(),
            cache_control: None,
        }],
    });
    app.api_messages.push(Message {
        role: "assistant".to_string(),
        content: vec![ContentBlock::Text {
            text: "Done, file is fixed now.".to_string(),
            cache_control: None,
        }],
    });

    let result = patch_undo(&mut app);

    assert!(!result.is_error);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "alpha");
    assert_eq!(app.history.len(), 1);
    assert!(matches!(
        app.history.last(),
        Some(HistoryCell::System { content }) if content.contains("/undo reverted workspace")
    ));
    assert!(app.api_messages.is_empty());
}

#[test]
fn test_prune_undone_tool_context_preserves_prior_tool_pairs() {
    let mut app = create_test_app();
    app.history.push(HistoryCell::User {
        content: "edit two files".to_string(),
    });
    app.history.push(HistoryCell::Assistant {
        content: "I will update both files.".to_string(),
        streaming: false,
    });
    app.history
        .push(HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "write_file".to_string(),
            status: ToolStatus::Success,
            input_summary: Some("a.txt".to_string()),
            output: Some("updated a".to_string()),
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        })));
    app.history
        .push(HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "write_file".to_string(),
            status: ToolStatus::Success,
            input_summary: Some("b.txt".to_string()),
            output: Some("updated b".to_string()),
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        })));
    app.history.push(HistoryCell::Assistant {
        content: "Done.".to_string(),
        streaming: false,
    });
    app.tool_cells.insert("call-a".to_string(), 2);
    app.tool_cells.insert("call-b".to_string(), 3);

    app.api_messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: "edit two files".to_string(),
            cache_control: None,
        }],
    });
    app.api_messages.push(Message {
        role: "assistant".to_string(),
        content: vec![
            ContentBlock::Text {
                text: "I will update both files.".to_string(),
                cache_control: None,
            },
            ContentBlock::ToolUse {
                id: "call-a".to_string(),
                name: "write_file".to_string(),
                input: serde_json::json!({"path": "a.txt"}),
                caller: None,
            },
            ContentBlock::ToolUse {
                id: "call-b".to_string(),
                name: "write_file".to_string(),
                input: serde_json::json!({"path": "b.txt"}),
                caller: None,
            },
        ],
    });
    app.api_messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "call-a".to_string(),
            content: "updated a".to_string(),
            is_error: None,
            content_blocks: None,
        }],
    });
    app.api_messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "call-b".to_string(),
            content: "updated b".to_string(),
            is_error: None,
            content_blocks: None,
        }],
    });
    app.api_messages.push(Message {
        role: "assistant".to_string(),
        content: vec![ContentBlock::Text {
            text: "Done.".to_string(),
            cache_control: None,
        }],
    });

    prune_undone_tool_context(&mut app, "call-b");

    assert_eq!(app.history.len(), 3);
    assert_eq!(app.api_messages.len(), 3);
    assert!(matches!(
        &app.api_messages[1].content[..],
        [
            ContentBlock::Text { .. },
            ContentBlock::ToolUse { id, .. }
        ] if id == "call-a"
    ));
    assert!(matches!(
        &app.api_messages[2].content[0],
        ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call-a"
    ));
}

// ── /cache stats tests ──────────────────────────────────────────────

#[test]
fn cache_stats_no_data_before_first_turn() {
    let mut app = create_test_app();
    let result = cache(&mut app, Some("stats"));
    let msg = result.message.expect("cache stats produces a message");
    assert!(msg.contains("Cache Stats"), "got: {msg}");
    assert!(
        msg.contains("unknown (no checks recorded yet)"),
        "got: {msg}"
    );
    assert!(msg.contains("Pinned hash: unavailable"), "got: {msg}");
    assert!(msg.contains("No turn telemetry recorded yet"), "got: {msg}");
}

#[test]
fn cache_stats_shows_stable_prefix_with_hash() {
    let mut app = create_test_app();
    app.prefix_stability_pct = Some(100);
    app.prefix_checks_total = 5;
    app.prefix_change_count = 0;
    app.last_pinned_prefix_hash =
        Some("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2".to_string());

    let result = cache(&mut app, Some("stats"));
    let msg = result.message.expect("cache stats produces a message");

    assert!(msg.contains("Stability: 100%"), "got: {msg}");
    assert!(msg.contains("stable (no prefix changes"), "got: {msg}");
    assert!(msg.contains("Pinned hash: a1b2c3d4e5f6"), "got: {msg}");
    assert!(
        msg.contains("Drift:       none (hash stable)"),
        "got: {msg}"
    );
}

#[test]
fn cache_stats_warns_on_prefix_change() {
    let mut app = create_test_app();
    app.prefix_stability_pct = Some(67);
    app.prefix_checks_total = 3;
    app.prefix_change_count = 1;
    app.last_prefix_change_desc =
        Some("prefix cache invalidated: system prompt changed".to_string());
    app.last_pinned_prefix_hash =
        Some("deadbeef0000deadbeef0000deadbeef0000deadbeef0000deadbeef0000deadbeef".to_string());

    let result = cache(&mut app, Some("stats"));
    let msg = result.message.expect("cache stats produces a message");

    assert!(msg.contains("Stability: 67%"), "got: {msg}");
    assert!(msg.contains("WARNING — prefix has changed"), "got: {msg}");
    assert!(msg.contains("system prompt changed"), "got: {msg}");
    assert!(msg.contains("Drift:       WARNING"), "got: {msg}");
    assert!(msg.contains("1 change detected"), "got: {msg}");
}

#[test]
fn cache_stats_shows_cache_hit_summary() {
    let mut app = create_test_app();
    app.prefix_stability_pct = Some(100);
    app.prefix_checks_total = 1;
    app.last_pinned_prefix_hash =
        Some("abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234".to_string());

    app.push_turn_cache_record(TurnCacheRecord {
        provider: None,
        provider_identity: None,
        model: None,
        auto_model: false,
        input_tokens: 10_000,
        output_tokens: 1_000,
        cache_hit_tokens: Some(8_000),
        cache_miss_tokens: Some(2_000),
        reasoning_replay_tokens: None,
        recorded_at: Instant::now(),
    });
    app.push_turn_cache_record(TurnCacheRecord {
        provider: None,
        provider_identity: None,
        model: None,
        auto_model: false,
        input_tokens: 5_000,
        output_tokens: 500,
        cache_hit_tokens: Some(4_500),
        cache_miss_tokens: Some(500),
        reasoning_replay_tokens: None,
        recorded_at: Instant::now(),
    });

    let result = cache(&mut app, Some("stats"));
    let msg = result.message.expect("cache stats produces a message");

    assert!(msg.contains("Turns recorded: 2"), "got: {msg}");
    // Total: 12,500 hit out of 15,000 cache-aware = 83.3%
    assert!(msg.contains("83.3%"), "got: {msg}");
}

#[test]
fn cache_stats_low_hit_rate_shows_note() {
    let mut app = create_test_app();
    app.prefix_stability_pct = Some(100);
    app.prefix_checks_total = 1;
    app.last_pinned_prefix_hash =
        Some("abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234".to_string());

    app.push_turn_cache_record(TurnCacheRecord {
        provider: None,
        provider_identity: None,
        model: None,
        auto_model: false,
        input_tokens: 10_000,
        output_tokens: 1_000,
        cache_hit_tokens: Some(1_000),
        cache_miss_tokens: Some(9_000),
        reasoning_replay_tokens: None,
        recorded_at: Instant::now(),
    });

    let result = cache(&mut app, Some("stats"));
    let msg = result.message.expect("cache stats produces a message");

    // 10% hit rate → below 80% threshold
    assert!(msg.contains("10.0%"), "got: {msg}");
    assert!(
        msg.contains("cache hit rate is low"),
        "should show low-hit-rate advisory, got: {msg}"
    );
}

#[test]
fn cache_stats_flags_reported_1747_low_hit_fixture() {
    let mut app = create_test_app();
    app.prefix_stability_pct = Some(100);
    app.prefix_checks_total = 1;
    app.last_pinned_prefix_hash =
        Some("abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234".to_string());

    // Fixture from #1747 / Amund's DeepSeek-TUI session aggregate:
    // hit=21,356,928, miss=8,470,281, output=165,624.
    app.push_turn_cache_record(TurnCacheRecord {
        provider: None,
        provider_identity: None,
        model: None,
        auto_model: false,
        input_tokens: 29_827_209,
        output_tokens: 165_624,
        cache_hit_tokens: Some(21_356_928),
        cache_miss_tokens: Some(8_470_281),
        reasoning_replay_tokens: None,
        recorded_at: Instant::now(),
    });

    let result = cache(&mut app, Some("stats"));
    let msg = result.message.expect("cache stats produces a message");

    assert!(msg.contains("71.6%"), "got: {msg}");
    assert!(msg.contains("Cache hit tokens:  21.4M"), "got: {msg}");
    assert!(msg.contains("Cache miss tokens: 8.5M"), "got: {msg}");
    assert!(
        msg.contains("cache hit rate is low"),
        "reported #1747 fixture should remain below the advisory threshold: {msg}"
    );
}

#[test]
fn format_tokens_handles_all_scales() {
    assert_eq!(format_tokens(0), "0");
    assert_eq!(format_tokens(999), "999");
    assert_eq!(format_tokens(1_000), "1.0K");
    assert_eq!(format_tokens(15_500), "15.5K");
    assert_eq!(format_tokens(1_000_000), "1.0M");
    assert_eq!(format_tokens(2_500_000), "2.5M");
}
