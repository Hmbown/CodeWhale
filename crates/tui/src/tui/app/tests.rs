use super::*;
use crate::config::{
    ApiProvider, CompactionRuntimeConfig, Config, ProviderConfig, ProvidersConfig,
};
use crate::test_support::{EnvVarGuard, lock_test_env};
use crate::tools::plan::{PlanItemArg, StepStatus, UpdatePlanArgs};
use crate::tools::todo::TodoStatus;
use crate::tui::clipboard::PastedImage;
use crate::tui::history::{GenericToolCell, ToolCell, ToolStatus};

fn test_options(yolo: bool) -> TuiOptions {
    TuiOptions {
        model: "test-model".to_string(),
        workspace: PathBuf::from("."),
        config_path: None,
        config_profile: None,
        allow_shell: yolo,
        use_alt_screen: true,
        use_mouse_capture: false,
        use_bracketed_paste: true,
        max_subagents: 1,
        skills_dir: PathBuf::from("."),
        memory_path: PathBuf::from("memory.md"),
        notes_path: PathBuf::from("notes.txt"),
        mcp_config_path: PathBuf::from("mcp.json"),
        use_memory: false,
        // Keep unit tests independent from the developer's saved
        // `default_mode` setting.
        start_in_agent_mode: true,
        skip_onboarding: false,
        yolo,
        resume_session_id: None,
        initial_input: None,
    }
}

#[cfg(unix)]
fn create_dir_symlink(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_dir_symlink(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

#[test]
fn initial_input_prefill_waits_for_manual_submit() {
    let mut options = test_options(false);
    options.initial_input = Some(InitialInput::Prefill("review this PR".to_string()));

    let app = App::new(options, &Config::default());

    assert_eq!(app.input, "review this PR");
    assert_eq!(app.cursor_position, "review this PR".chars().count());
    assert!(!app.auto_submit_initial_input);
}

#[test]
fn initial_input_submit_marks_startup_dispatch() {
    let mut options = test_options(false);
    options.initial_input = Some(InitialInput::Submit(
        "阅读项目 and wait for instructions".to_string(),
    ));

    let app = App::new(options, &Config::default());

    assert_eq!(app.input, "阅读项目 and wait for instructions");
    assert_eq!(
        app.cursor_position,
        "阅读项目 and wait for instructions".chars().count()
    );
    assert!(app.auto_submit_initial_input);
}

#[test]
fn composer_arrows_scroll_default_is_true_without_mouse_capture() {
    assert!(default_composer_arrows_scroll_for_platform(false, false));
}

#[test]
fn composer_arrows_scroll_default_is_false_with_mouse_capture_on_non_windows() {
    assert!(!default_composer_arrows_scroll_for_platform(true, false));
}

#[test]
fn composer_arrows_scroll_default_is_false_with_mouse_capture_on_windows() {
    assert!(!default_composer_arrows_scroll_for_platform(true, true));
}

#[test]
fn composer_arrows_scroll_default_is_true_without_mouse_capture_on_windows() {
    assert!(default_composer_arrows_scroll_for_platform(false, true));
}

#[test]
fn move_cursor_line_start_multiline() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "abc\ndef\nghi".to_string();
    app.cursor_position = "abc\ndef\nghi".chars().count(); // absolute end
    app.move_cursor_line_start();
    assert_eq!(app.cursor_position, "abc\ndef\n".len()); // start of "ghi"
}

#[test]
fn move_cursor_line_start_singleline() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "hello".to_string();
    app.cursor_position = 3;
    app.move_cursor_line_start();
    assert_eq!(app.cursor_position, 0);
}

#[test]
fn move_cursor_line_end_multiline() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "abc\ndef\nghi".to_string();
    app.cursor_position = 0; // start of first line
    app.move_cursor_line_end();
    assert_eq!(app.cursor_position, "abc".len()); // before first '\n'
}

#[test]
fn move_cursor_line_end_at_newline_stays_at_line_end() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "abc\ndef\nghi".to_string();
    app.cursor_position = "abc".len(); // on the '\n'
    app.move_cursor_line_end();
    assert_eq!(app.cursor_position, "abc".len()); // stays at line end
}

#[test]
fn move_cursor_line_end_last_line() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "abc\ndef".to_string();
    app.cursor_position = "abc\n".len(); // start of last line
    app.move_cursor_line_end();
    assert_eq!(app.cursor_position, "abc\ndef".chars().count()); // absolute end
}

#[test]
fn move_cursor_line_start_already_at_start() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "abc\ndef".to_string();
    app.cursor_position = "abc\n".len(); // start of second line
    app.move_cursor_line_start();
    assert_eq!(app.cursor_position, "abc\n".len()); // unchanged
}

#[test]
fn test_trust_mode_follows_yolo_on_startup() {
    let app = App::new(test_options(true), &Config::default());
    assert!(app.trust_mode);
}

#[test]
fn reasoning_effort_display_label_uses_codex_xhigh() {
    assert_eq!(
        ReasoningEffort::Off.display_label_for_provider(ApiProvider::OpenaiCodex),
        "low"
    );
    assert_eq!(
        ReasoningEffort::Medium.display_label_for_provider(ApiProvider::OpenaiCodex),
        "medium"
    );
    assert_eq!(
        ReasoningEffort::Max.display_label_for_provider(ApiProvider::OpenaiCodex),
        "xhigh"
    );
    assert_eq!(
        ReasoningEffort::Max.display_label_for_provider(ApiProvider::Deepseek),
        "max"
    );
    assert_eq!(
        ReasoningEffort::High.display_label_for_provider(ApiProvider::OpenaiCodex),
        "high"
    );

    let mut app = App::new(test_options(false), &Config::default());
    app.api_provider = ApiProvider::OpenaiCodex;
    app.reasoning_effort = ReasoningEffort::Max;
    app.auto_model = false;
    assert_eq!(app.reasoning_effort_display_label(), "xhigh");

    app.reasoning_effort = ReasoningEffort::Auto;
    app.last_effective_reasoning_effort = Some(ReasoningEffort::Max);
    assert_eq!(app.reasoning_effort_display_label(), "auto: xhigh");
}

#[test]
fn mode_and_thinking_are_locked_while_a_turn_is_running() {
    // #2982: while a turn is in flight, user-initiated mode/thinking changes
    // are refused with a concise message instead of shifting the surface the
    // engine is acting on.
    let mut app = App::new(test_options(false), &Config::default());
    app.mode = AppMode::Agent;
    app.reasoning_effort = ReasoningEffort::Max;
    app.is_loading = true;

    app.cycle_mode();
    assert_eq!(app.mode, AppMode::Agent, "mode must not change while busy");
    assert!(
        app.status_message
            .as_deref()
            .unwrap_or_default()
            .contains("locked"),
        "expected a 'locked' status message, got {:?}",
        app.status_message
    );

    let before_effort = app.reasoning_effort;
    app.cycle_effort();
    assert_eq!(
        app.reasoning_effort, before_effort,
        "thinking must not change while busy"
    );

    // Once the turn finishes, the same gesture works again.
    app.is_loading = false;
    app.cycle_mode();
    assert_ne!(app.mode, AppMode::Agent, "mode should change when idle");
}

#[test]
fn reasoning_effort_api_values_are_provider_aware_for_codex() {
    assert_eq!(
        ReasoningEffort::Off.normalize_for_provider(ApiProvider::OpenaiCodex),
        ReasoningEffort::Low
    );
    assert_eq!(
        ReasoningEffort::Auto.normalize_for_provider(ApiProvider::OpenaiCodex),
        ReasoningEffort::Medium
    );
    assert_eq!(
        ReasoningEffort::Max.api_value_for_provider(ApiProvider::OpenaiCodex),
        Some("xhigh")
    );
    assert_eq!(
        ReasoningEffort::Off.api_value_for_provider(ApiProvider::OpenaiCodex),
        Some("low")
    );
    assert_eq!(
        ReasoningEffort::Max.api_value_for_provider(ApiProvider::Deepseek),
        Some("max")
    );
    assert_eq!(
        ReasoningEffort::from_setting("ultracode"),
        ReasoningEffort::Max
    );
}

#[test]
fn set_model_selection_normalizes_codex_fixed_model_effort() {
    let mut app = App::new(test_options(false), &Config::default());
    app.api_provider = ApiProvider::OpenaiCodex;
    app.reasoning_effort = ReasoningEffort::Off;

    app.set_model_selection("gpt-5.5-codex".to_string());

    assert_eq!(app.reasoning_effort, ReasoningEffort::Low);
    assert!(!app.auto_model);
    assert_eq!(app.reasoning_effort_display_label(), "low");
}

#[test]
fn app_new_normalizes_saved_codex_reasoning_effort() {
    let _lock = lock_test_env();
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let config_path = tmp.path().join("config.toml");
    let _config_path = EnvVarGuard::set("DEEPSEEK_CONFIG_PATH", &config_path);
    let _token = EnvVarGuard::set("OPENAI_CODEX_ACCESS_TOKEN", "test-codex-startup-token");
    let config = Config {
        provider: Some("openai-codex".to_string()),
        providers: Some(ProvidersConfig {
            openai_codex: ProviderConfig {
                model: Some(crate::config::DEFAULT_OPENAI_CODEX_MODEL.to_string()),
                ..ProviderConfig::default()
            },
            ..ProvidersConfig::default()
        }),
        ..Config::default()
    };

    for (raw, expected, display) in [
        ("off", ReasoningEffort::Low, "low"),
        ("auto", ReasoningEffort::Medium, "medium"),
        ("max", ReasoningEffort::Max, "xhigh"),
    ] {
        std::fs::write(
            tmp.path().join("settings.toml"),
            format!("reasoning_effort = \"{raw}\"\n"),
        )
        .expect("settings");

        let app = App::new(test_options(false), &config);

        assert_eq!(app.api_provider, ApiProvider::OpenaiCodex);
        assert_eq!(app.reasoning_effort, expected, "raw setting {raw}");
        assert_eq!(app.reasoning_effort_display_label(), display);
    }
}

#[test]
fn settings_default_provider_auth_check_uses_provider_scoped_key() {
    let _lock = lock_test_env();
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let config_path = tmp.path().join("config.toml");
    std::fs::write(
        tmp.path().join("settings.toml"),
        "default_provider = \"openai\"\n",
    )
    .expect("settings");
    let _config_path = EnvVarGuard::set("DEEPSEEK_CONFIG_PATH", &config_path);
    let _deepseek_key = EnvVarGuard::remove("DEEPSEEK_API_KEY");
    let _openai_key = EnvVarGuard::remove("OPENAI_API_KEY");

    let config = Config {
        providers: Some(ProvidersConfig {
            openai: ProviderConfig {
                api_key: Some("openai-config-key".to_string()),
                ..ProviderConfig::default()
            },
            ..ProvidersConfig::default()
        }),
        ..Config::default()
    };

    let app = App::new(test_options(false), &config);

    assert_eq!(app.api_provider, ApiProvider::Openai);
    assert!(
        !app.onboarding_needs_api_key,
        "OpenAI provider config key should satisfy startup auth without a DeepSeek key"
    );
    assert_ne!(app.onboarding, OnboardingState::ApiKey);
    assert!(!app.api_key_env_only);
}

#[test]
fn explicit_config_provider_wins_over_saved_default_provider() {
    let _lock = lock_test_env();
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let config_path = tmp.path().join("config.toml");
    std::fs::write(
        tmp.path().join("settings.toml"),
        "default_provider = \"deepseek\"\ndefault_model = \"deepseek-v4-pro\"\n",
    )
    .expect("settings");
    let _config_path = EnvVarGuard::set("DEEPSEEK_CONFIG_PATH", &config_path);

    let config = Config {
        provider: Some("xiaomi-mimo".to_string()),
        providers: Some(ProvidersConfig {
            xiaomi_mimo: ProviderConfig {
                api_key: Some("mimo-config-key".to_string()),
                model: Some("mimo-v2.5-pro".to_string()),
                ..ProviderConfig::default()
            },
            ..ProvidersConfig::default()
        }),
        ..Config::default()
    };

    let mut options = test_options(false);
    options.model = "mimo-v2.5-pro".to_string();
    let app = App::new(options, &config);

    assert_eq!(app.api_provider, ApiProvider::XiaomiMimo);
    assert_eq!(app.model, "mimo-v2.5-pro");
    assert!(
        !app.onboarding_needs_api_key,
        "Xiaomi MiMo provider config key should satisfy startup auth"
    );
}

#[test]
fn app_new_defaults_auto_compact_on_for_256k_class_models_when_unset() {
    let _lock = lock_test_env();
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let config_path = tmp.path().join("config.toml");
    let _config_path = EnvVarGuard::set("DEEPSEEK_CONFIG_PATH", &config_path);

    let mut options = test_options(false);
    options.model = "trinity-large-thinking".to_string();
    let app = App::new(options, &Config::default());

    assert!(app.auto_compact);
    assert!(!app.auto_compact_user_configured);
    assert_eq!(app.auto_compact_threshold_percent, 80.0);
    assert_eq!(app.compact_threshold, 209_715);
}

#[test]
fn app_new_defaults_auto_compact_on_for_v4_class_models_when_unset() {
    let _lock = lock_test_env();
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let config_path = tmp.path().join("config.toml");
    let _config_path = EnvVarGuard::set("DEEPSEEK_CONFIG_PATH", &config_path);

    let mut options = test_options(false);
    options.model = "deepseek-v4-pro".to_string();
    let app = App::new(options, &Config::default());

    assert!(app.auto_compact);
    assert!(!app.auto_compact_user_configured);
    assert_eq!(app.auto_compact_threshold_percent, 80.0);
    assert_eq!(app.compact_threshold, 800_000);
}

#[test]
fn app_new_respects_explicit_auto_compact_false_for_256k_class_models() {
    let _lock = lock_test_env();
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let config_path = tmp.path().join("config.toml");
    std::fs::write(tmp.path().join("settings.toml"), "auto_compact = false\n").expect("settings");
    let _config_path = EnvVarGuard::set("DEEPSEEK_CONFIG_PATH", &config_path);

    let mut options = test_options(false);
    options.model = "trinity-large-thinking".to_string();
    let app = App::new(options, &Config::default());

    assert!(!app.auto_compact);
    assert!(app.auto_compact_user_configured);
    assert_eq!(app.compact_threshold, 209_715);
}

#[test]
fn app_new_respects_explicit_auto_compact_false_for_v4_class_models() {
    let _lock = lock_test_env();
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let config_path = tmp.path().join("config.toml");
    std::fs::write(tmp.path().join("settings.toml"), "auto_compact = false\n").expect("settings");
    let _config_path = EnvVarGuard::set("DEEPSEEK_CONFIG_PATH", &config_path);

    let mut options = test_options(false);
    options.model = "deepseek-v4-pro".to_string();
    let app = App::new(options, &Config::default());

    assert!(!app.auto_compact);
    assert!(app.auto_compact_user_configured);
    assert_eq!(app.compact_threshold, 800_000);
}

#[test]
fn cny_display_falls_back_to_usd_for_usd_only_costs() {
    let mut app = App::new(test_options(false), &Config::default());
    app.cost_currency = CostCurrency::Cny;
    app.accrue_session_cost_estimate(CostEstimate::usd_only(0.42));

    let displayed = app.displayed_session_cost_for_currency(CostCurrency::Cny);

    assert_eq!(displayed, 0.42);
    assert_eq!(app.session_cost_for_currency(CostCurrency::Cny), 0.42);
    assert_eq!(app.format_cost_amount(displayed), "$0.42");
}

#[test]
fn cny_display_keeps_cny_when_costs_have_cny_rates() {
    let mut app = App::new(test_options(false), &Config::default());
    app.cost_currency = CostCurrency::Cny;
    app.accrue_session_cost_estimate(CostEstimate {
        usd: 0.42,
        cny: 2.5,
    });

    let displayed = app.displayed_session_cost_for_currency(CostCurrency::Cny);

    assert_eq!(displayed, 2.5);
    assert_eq!(app.format_cost_amount(displayed), "¥2.50");
}

#[test]
fn cny_cache_savings_falls_back_to_usd_for_usd_only_models() {
    let mut app = App::new(test_options(false), &Config::default());
    app.cost_currency = CostCurrency::Cny;
    app.model = "kimi-k2.6".to_string();
    app.session.last_prompt_cache_hit_tokens = Some(1_000_000);

    assert_eq!(app.last_turn_cache_savings(), Some(0.34));
}

#[test]
fn sidebar_focus_accepts_pinned_and_maps_legacy_trackers_to_pinned() {
    assert_eq!(SidebarFocus::from_setting("auto"), SidebarFocus::Auto);
    assert_eq!(SidebarFocus::from_setting("pinned"), SidebarFocus::Pinned);
    assert_eq!(SidebarFocus::from_setting("work"), SidebarFocus::Pinned);
    assert_eq!(SidebarFocus::from_setting("plan"), SidebarFocus::Pinned);
    assert_eq!(SidebarFocus::from_setting("todos"), SidebarFocus::Pinned);
    assert_eq!(SidebarFocus::from_setting("tasks"), SidebarFocus::Tasks);
    assert_eq!(SidebarFocus::from_setting("agents"), SidebarFocus::Agents);
    assert_eq!(SidebarFocus::from_setting("context"), SidebarFocus::Context);
    assert_eq!(SidebarFocus::from_setting("hidden"), SidebarFocus::Hidden);
    assert_eq!(SidebarFocus::from_setting("off"), SidebarFocus::Hidden);
    assert_eq!(SidebarFocus::Pinned.as_setting(), "pinned");
    assert_eq!(SidebarFocus::Hidden.as_setting(), "hidden");
}

#[test]
fn slash_command_classifier_treats_absolute_path_as_message() {
    assert!(looks_like_slash_command_input("/"));
    assert!(looks_like_slash_command_input("/help"));
    assert!(looks_like_slash_command_input("/model deepseek-v4-pro"));
    assert!(!looks_like_slash_command_input("/ hello"));
    assert!(!looks_like_slash_command_input("  / hello"));
    assert!(!looks_like_slash_command_input(
        "/usr/lib/x86_64-linux-gnu/ 是标准路径吗？"
    ));
}

#[test]
fn bang_shell_prefix_parses_compact_and_spaced_forms() {
    assert_eq!(shell_command_from_bang_input("!pwd"), Ok(Some("pwd")));
    assert_eq!(shell_command_from_bang_input("! pwd"), Ok(Some("pwd")));
    assert_eq!(
        shell_command_from_bang_input("  !  cargo test -p codewhale-tui sidebar"),
        Ok(Some("cargo test -p codewhale-tui sidebar"))
    );
    assert_eq!(shell_command_from_bang_input("normal message"), Ok(None));
}

#[test]
fn bang_shell_prefix_rejects_empty_command() {
    assert_eq!(
        shell_command_from_bang_input("!"),
        Err("Usage: ! <shell command>")
    );
    assert_eq!(
        shell_command_from_bang_input("!   "),
        Err("Usage: ! <shell command>")
    );
}

#[test]
fn submit_input_records_absolute_slash_path_as_message_history() {
    let mut app = App::new(test_options(false), &Config::default());
    let input = "/usr/lib/x86_64-linux-gnu/ 是标准路径吗？";
    app.input = input.to_string();
    app.cursor_position = input.chars().count();

    let submitted = app.submit_input().expect("expected submitted input");

    assert_eq!(submitted, input);
    assert_eq!(app.input_history.last().map(String::as_str), Some(input));
}

#[test]
fn restore_last_submitted_prompt_rehydrates_empty_composer() {
    let mut app = App::new(test_options(false), &Config::default());
    app.last_submitted_prompt = Some("fix the typo\nand retry".to_string());

    assert!(app.restore_last_submitted_prompt_if_empty());

    assert_eq!(app.input, "fix the typo\nand retry");
    assert_eq!(app.cursor_position, app.input.chars().count());
    assert!(app.needs_redraw);
}

#[test]
fn restore_last_submitted_prompt_preserves_existing_draft() {
    let mut app = App::new(test_options(false), &Config::default());
    app.last_submitted_prompt = Some("previous prompt".to_string());
    app.input = "new draft".to_string();
    app.cursor_position = app.input.chars().count();

    assert!(!app.restore_last_submitted_prompt_if_empty());

    assert_eq!(app.input, "new draft");
    assert_eq!(app.cursor_position, "new draft".chars().count());
}

#[test]
fn composer_strips_raw_sgr_mouse_report_when_mouse_capture_is_enabled() {
    let mut app = App::new(test_options(false), &Config::default());
    app.use_mouse_capture = true;

    app.insert_str("[<35;44;18M");

    assert_eq!(app.input, "");
    assert_eq!(app.cursor_position, 0);
}

#[test]
fn composer_strips_corrupted_mouse_report_burst() {
    let mut app = App::new(test_options(false), &Config::default());
    app.use_mouse_capture = true;
    app.insert_str("draft ");
    let leaked = "43;19M[<35;44;18M[<35;45;18M5;46;18M;48;18M";

    app.insert_str(leaked);

    assert_eq!(app.input, "draft ");
    assert_eq!(app.cursor_position, "draft ".chars().count());
}

#[test]
fn composer_preserves_draft_suffix_when_stripping_mouse_report() {
    let mut app = App::new(test_options(false), &Config::default());
    app.use_mouse_capture = true;
    app.insert_str("commit -m");

    app.insert_str("[<65;44;18M");

    assert_eq!(app.input, "commit -m");
    assert_eq!(app.cursor_position, "commit -m".chars().count());
}

#[test]
fn composer_preserves_numeric_draft_when_stripping_mouse_report() {
    let mut app = App::new(test_options(false), &Config::default());
    app.use_mouse_capture = true;
    app.insert_str("123");

    app.insert_str("[<65;44;18M");

    assert_eq!(app.input, "123");
    assert_eq!(app.cursor_position, 3);
}

#[test]
fn composer_strips_raw_sgr_mouse_report_when_mouse_capture_is_disabled() {
    let mut app = App::new(test_options(false), &Config::default());

    app.insert_str("[<35;44;18M");

    assert_eq!(app.input, "");
    assert_eq!(app.cursor_position, 0);
}

#[test]
fn composer_strips_tail_only_mouse_report_burst_when_mouse_capture_is_disabled() {
    let mut app = App::new(test_options(false), &Config::default());
    app.insert_str("draft ");

    app.insert_str(";76;20M35;74;22M35;73;23M");

    assert_eq!(app.input, "draft ");
    assert_eq!(app.cursor_position, "draft ".chars().count());
}

#[test]
fn composer_keeps_coordinate_like_text_when_mouse_capture_is_disabled() {
    let mut app = App::new(test_options(false), &Config::default());

    app.insert_str("Size 12;34M");

    assert_eq!(app.input, "Size 12;34M");
    assert_eq!(app.cursor_position, "Size 12;34M".chars().count());
}

#[test]
fn composer_keeps_normal_bracket_text_with_mouse_capture_enabled() {
    let mut app = App::new(test_options(false), &Config::default());
    app.use_mouse_capture = true;

    app.insert_str("Use [<tag>] normally");

    assert_eq!(app.input, "Use [<tag>] normally");
}

#[test]
fn composer_keeps_coordinate_like_text_with_mouse_capture_enabled() {
    let mut app = App::new(test_options(false), &Config::default());
    app.use_mouse_capture = true;

    app.insert_str("Size 12;34M");

    assert_eq!(app.input, "Size 12;34M");
}

// === Bug #1915: broader terminal control-sequence fragments leaking
// into the composer during dense streaming output. The narrow SGR
// mouse-report filter installed in e63a4ba4a covers `[<…M` style
// bursts, but not OSC 8 hyperlink fragments (`]8;;http…`) or Kitty
// keyboard protocol responses (`[?u`, `[>1u`). These can arrive when
// crossterm's event reader is mid-sequence and the unparsed tail is
// delivered as individual Char(c) keystrokes that land in the input.

#[test]
fn composer_strips_osc8_hyperlink_fragment() {
    let mut app = App::new(test_options(false), &Config::default());
    app.use_mouse_capture = true;
    app.insert_str("draft ");

    // OSC 8 prefix with URL body but no terminator delivered yet —
    // exactly what crossterm hands us if its event reader is
    // interrupted mid-sequence and the leading ESC is consumed by the
    // parser before the rest gets reclassified as Char(c).
    app.insert_str("]8;;https://example.com");

    assert_eq!(app.input, "draft ");
    assert_eq!(app.cursor_position, "draft ".chars().count());
}

#[test]
fn composer_strips_closing_osc8_fragment() {
    let mut app = App::new(test_options(false), &Config::default());
    app.use_mouse_capture = true;
    app.insert_str("hello ");

    // The closing wrapper `]8;;` (with a stray ST `\\` from a
    // chopped escape) can arrive on its own when the parser ate
    // the start of the sequence in a previous read but caught the
    // tail as keystrokes.
    app.insert_str("]8;;\\");

    assert_eq!(app.input, "hello ");
    assert_eq!(app.cursor_position, "hello ".chars().count());
}

#[test]
fn composer_strips_kitty_keyboard_protocol_fragment() {
    let mut app = App::new(test_options(false), &Config::default());
    app.use_mouse_capture = true;
    app.insert_str("ready ");

    // Kitty keyboard protocol responses look like `\x1b[?1u`,
    // `\x1b[>1u`, `\x1b[<1u`, or `\x1b[?u`. With the ESC consumed,
    // the tail shape is `[?…u`, `[>…u`, or `[<…u`.
    app.insert_str("[?1u[>1u[<1u[?u");

    assert_eq!(app.input, "ready ");
    assert_eq!(app.cursor_position, "ready ".chars().count());
}

#[test]
fn composer_strips_dec_private_mode_set_reset_fragments() {
    let mut app = App::new(test_options(false), &Config::default());
    app.use_mouse_capture = true;
    app.insert_str("ok ");

    // Regression for #2592: DEC private mode set/reset chatter ends in
    // `h`/`l`, not `u`, so the `u`-only terminator used to leak the
    // leading `[`. Bracketed paste, mouse capture, focus reporting, and
    // synchronized output all leak during dense streaming.
    app.insert_str("[?2004h[?2004l[?1000h[?1004h[?2026h[?25l");

    assert_eq!(app.input, "ok ");
    assert_eq!(app.cursor_position, "ok ".chars().count());
}

#[test]
fn composer_keeps_bracket_question_word_text() {
    let mut app = App::new(test_options(false), &Config::default());
    app.use_mouse_capture = true;

    // The `h`/`l` terminator only counts after a numeric parameter, so
    // ordinary prose where a letter follows `[?` directly is preserved.
    app.insert_str("[?help] and [?later]");

    assert_eq!(app.input, "[?help] and [?later]");
}

#[test]
fn composer_strips_mixed_control_sequence_burst() {
    let mut app = App::new(test_options(false), &Config::default());
    app.use_mouse_capture = true;
    app.insert_str("hi");

    // Mixed dense burst combining all three fragment families
    // described in #1915.
    app.insert_str("[<35;44;18M]8;;https://example.com[?1u");

    assert_eq!(app.input, "hi");
    assert_eq!(app.cursor_position, 2);
}

#[test]
fn composer_keeps_legitimate_url_text_with_mouse_capture_enabled() {
    let mut app = App::new(test_options(false), &Config::default());
    app.use_mouse_capture = true;

    // URLs typed by the user must survive the filter — only
    // recognized control-sequence shapes are stripped.
    app.insert_str("see https://example.com/path?a=1&b=2 for info");

    assert_eq!(app.input, "see https://example.com/path?a=1&b=2 for info");
}

#[test]
fn composer_keeps_legitimate_bracket_question_text() {
    let mut app = App::new(test_options(false), &Config::default());
    app.use_mouse_capture = true;

    // Text that uses brackets, question marks, and lowercase `u` —
    // shapes that overlap Kitty fragments — must not be eaten.
    app.insert_str("[is this ok?] sure");

    assert_eq!(app.input, "[is this ok?] sure");
}

#[test]
fn composer_keeps_legitimate_closing_bracket_digit_text() {
    let mut app = App::new(test_options(false), &Config::default());
    app.use_mouse_capture = true;

    // Plain `]8` followed by spaces and words must survive — only
    // the OSC 8 shape `]8;` (with the mandatory `;` separator)
    // should be treated as a fragment.
    app.insert_str("array[]8 elements");

    assert_eq!(app.input, "array[]8 elements");
}

// initial_onboarding_state tests
// These pin the logic that decides whether the TUI shows the
// onboarding flow (Welcome → Language → ApiKey → …) or goes
// straight to the chat view.  Getting this wrong either locks
// first-run users out of the API-key prompt or nags returning
// users whose key is already configured.

#[test]
fn skip_onboarding_suppresses_all_onboarding_states() {
    assert_eq!(
        initial_onboarding_state(true, false, true, true),
        OnboardingState::None
    );
    assert_eq!(
        initial_onboarding_state(true, true, true, true),
        OnboardingState::None
    );
}

#[test]
fn fully_configured_returning_user_skips_onboarding() {
    assert_eq!(
        initial_onboarding_state(false, true, false, false),
        OnboardingState::None
    );
}

#[test]
fn returning_user_missing_api_key_goes_to_api_key_screen() {
    assert_eq!(
        initial_onboarding_state(false, true, true, false),
        OnboardingState::ApiKey
    );
    // workspace trust doesn't affect the api-key gate
    assert_eq!(
        initial_onboarding_state(false, true, true, true),
        OnboardingState::ApiKey
    );
}

#[test]
fn first_run_user_always_starts_at_welcome() {
    assert_eq!(
        initial_onboarding_state(false, false, false, false),
        OnboardingState::Welcome
    );
    assert_eq!(
        initial_onboarding_state(false, false, true, false),
        OnboardingState::Welcome
    );
    assert_eq!(
        initial_onboarding_state(false, false, false, true),
        OnboardingState::Welcome
    );
}

#[test]
fn onboarding_workspace_trust_gate_only_fires_for_onboarded_user() {
    assert!(onboarding_is_workspace_trust_gate(false, true, false, true));
    assert!(!onboarding_is_workspace_trust_gate(true, true, false, true));
    assert!(!onboarding_is_workspace_trust_gate(false, true, true, true));
    assert!(!onboarding_is_workspace_trust_gate(
        false, false, false, true
    ));
}

#[test]
fn onboarded_user_still_gets_workspace_trust_prompt_when_needed() {
    assert_eq!(
        initial_onboarding_state(false, true, false, true),
        OnboardingState::TrustDirectory
    );
}

// App::new tests: missing key is detected

#[test]
fn app_new_detects_missing_api_key_with_default_config() {
    let _lock = lock_test_env();
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let config_path = tmp.path().join("config.toml");
    let _config_path = EnvVarGuard::set("DEEPSEEK_CONFIG_PATH", &config_path);
    let _provider_env = EnvVarGuard::remove("CODEWHALE_PROVIDER");
    let _legacy_provider_env = EnvVarGuard::remove("DEEPSEEK_PROVIDER");
    let _api_key_envs: Vec<_> = [
        "DEEPSEEK_API_KEY",
        "NVIDIA_API_KEY",
        "NVIDIA_NIM_API_KEY",
        "OPENAI_API_KEY",
        "ATLASCLOUD_API_KEY",
        "WANJIE_ARK_API_KEY",
        "WANJIE_API_KEY",
        "WANJIE_MAAS_API_KEY",
        "OPENROUTER_API_KEY",
        "NOVITA_API_KEY",
        "FIREWORKS_API_KEY",
        "SILICONFLOW_API_KEY",
        "MOONSHOT_API_KEY",
        "KIMI_API_KEY",
        "SGLANG_API_KEY",
        "VLLM_API_KEY",
        "OLLAMA_API_KEY",
    ]
    .into_iter()
    .map(EnvVarGuard::remove)
    .collect();

    // Config::default() carries no api_key, and this test isolates process
    // env/settings so previous tests or developer shells cannot satisfy it.
    let app = App::new(test_options(false), &Config::default());
    assert!(
        app.onboarding_needs_api_key,
        "default config (no key) must set onboarding_needs_api_key"
    );
}

#[test]
fn app_new_with_explicit_api_key_does_not_trigger_onboarding() {
    let _lock = lock_test_env();
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let config_path = tmp.path().join("config.toml");
    let _config_path = EnvVarGuard::set("DEEPSEEK_CONFIG_PATH", &config_path);
    let _provider_env = EnvVarGuard::remove("CODEWHALE_PROVIDER");
    let _legacy_provider_env = EnvVarGuard::remove("DEEPSEEK_PROVIDER");

    let config = Config {
        api_key: Some("sk-test-onboarding-key".to_string()),
        ..Config::default()
    };
    let app = App::new(test_options(false), &config);
    assert!(
        !app.onboarding_needs_api_key,
        "explicit config.api_key must satisfy the onboarding check"
    );
}

#[test]
fn new_caches_workspace_skills_for_slash_menu() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspace");
    let skill_dir = workspace.join(".agents").join("skills").join("local-skill");
    std::fs::create_dir_all(&skill_dir).expect("skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: local-skill\ndescription: Local workspace skill\n---\nUse the local skill.\n",
    )
    .expect("skill file");

    let mut options = test_options(false);
    options.workspace = workspace.clone();
    options.skills_dir = tmp.path().join("global-skills");
    let app = App::new(options, &Config::default());

    assert_eq!(app.skills_dir, workspace.join(".agents").join("skills"));
    assert!(app.cached_skills.iter().any(|(name, description)| {
        name == "local-skill" && description == "Local workspace skill"
    }));
}

#[test]
fn cached_skills_merges_across_candidate_directories() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspace");

    // Higher-precedence directory contains a stale empty dir for `foo`
    // (no SKILL.md). This used to shadow the real definition further
    // down the candidate list when the cache only scanned a single dir.
    std::fs::create_dir_all(workspace.join(".agents").join("skills").join("foo"))
        .expect("stale empty dir");

    // Lower-precedence directory has the real skill.
    let real_dir = workspace.join(".claude").join("skills").join("foo");
    std::fs::create_dir_all(&real_dir).expect("real skill dir");
    std::fs::write(
        real_dir.join("SKILL.md"),
        "---\nname: foo\ndescription: Real foo skill\n---\nbody\n",
    )
    .expect("skill file");

    let mut options = test_options(false);
    options.workspace = workspace.clone();
    options.skills_dir = tmp.path().join("global-skills");
    let app = App::new(options, &Config::default());

    assert!(
        app.cached_skills
            .iter()
            .any(|(name, description)| name == "foo" && description == "Real foo skill"),
        "cached_skills should fall through to lower-precedence dir when higher-precedence one has an empty stub: {:?}",
        app.cached_skills,
    );
}

#[test]
fn cached_skills_respect_codewhale_only_scan_config() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspace");

    let claude_dir = workspace
        .join(".claude")
        .join("skills")
        .join("claude-skill");
    std::fs::create_dir_all(&claude_dir).expect("claude skill dir");
    std::fs::write(
        claude_dir.join("SKILL.md"),
        "---\nname: claude-skill\ndescription: Claude skill\n---\nbody\n",
    )
    .expect("write claude skill");

    let codewhale_dir = workspace
        .join(".codewhale")
        .join("skills")
        .join("codewhale-skill");
    std::fs::create_dir_all(&codewhale_dir).expect("codewhale skill dir");
    std::fs::write(
        codewhale_dir.join("SKILL.md"),
        "---\nname: codewhale-skill\ndescription: CodeWhale skill\n---\nbody\n",
    )
    .expect("write codewhale skill");

    let mut options = test_options(false);
    options.workspace = workspace.clone();
    options.skills_dir = tmp.path().join("global-skills");
    let app = App::new(
        options,
        &Config {
            skills: Some(crate::config::SkillsConfig {
                scan_codewhale_only: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        },
    );

    assert_eq!(app.skills_dir, workspace.join(".codewhale").join("skills"));
    assert!(
        app.cached_skills
            .iter()
            .any(|(name, _)| name == "codewhale-skill"),
        "CodeWhale skill should be cached: {:?}",
        app.cached_skills
    );
    assert!(
        !app.cached_skills
            .iter()
            .any(|(name, _)| name == "claude-skill"),
        "strict scan should not cache Claude skills: {:?}",
        app.cached_skills
    );
}

#[test]
fn resolve_skills_dir_requires_codewhale_skills_to_be_directory() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(workspace.join(".codewhale")).expect("codewhale dir");
    std::fs::write(
        workspace.join(".codewhale").join("skills"),
        "not a directory",
    )
    .expect("skills file");

    let global_skills_dir = tmp.path().join("global-skills");
    let config = Config {
        skills: Some(crate::config::SkillsConfig {
            scan_codewhale_only: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    };

    let resolved = resolve_skills_dir(&workspace, &global_skills_dir, &config);

    assert_eq!(resolved, global_skills_dir);
}

#[test]
fn cached_skills_include_configured_directory() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspace");

    let configured_dir = tmp.path().join("configured-skills");
    let configured_skill_dir = configured_dir.join("configured-skill");
    std::fs::create_dir_all(&configured_skill_dir).expect("configured skill dir");
    std::fs::write(
        configured_skill_dir.join("SKILL.md"),
        "---\nname: configured-skill\ndescription: Configured skill\n---\nbody\n",
    )
    .expect("write configured skill");

    let mut options = test_options(false);
    options.workspace = workspace.clone();
    options.skills_dir = configured_dir.clone();
    let config = Config {
        skills_dir: Some(configured_dir.to_string_lossy().into_owned()),
        ..Default::default()
    };
    let app = App::new(options, &config);

    assert!(
        app.cached_skills
            .iter()
            .any(|(name, description)| name == "configured-skill"
                && description == "Configured skill"),
        "configured skill dir should be merged: {:?}",
        app.cached_skills
    );
}

#[test]
fn cached_skills_preserve_configured_directory_in_codewhale_only_scan() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspace");

    let codewhale_skill_dir = workspace
        .join(".codewhale")
        .join("skills")
        .join("workspace-codewhale");
    std::fs::create_dir_all(&codewhale_skill_dir).expect("workspace codewhale skill dir");
    std::fs::write(
        codewhale_skill_dir.join("SKILL.md"),
        "---\nname: workspace-codewhale\ndescription: Workspace CodeWhale skill\n---\nbody\n",
    )
    .expect("write workspace codewhale skill");

    let configured_dir = tmp.path().join("configured-skills");
    let configured_skill_dir = configured_dir.join("configured-skill");
    std::fs::create_dir_all(&configured_skill_dir).expect("configured skill dir");
    std::fs::write(
        configured_skill_dir.join("SKILL.md"),
        "---\nname: configured-skill\ndescription: Configured skill\n---\nbody\n",
    )
    .expect("write configured skill");

    let mut options = test_options(false);
    options.workspace = workspace.clone();
    options.skills_dir = configured_dir.clone();
    let config = Config {
        skills_dir: Some(configured_dir.to_string_lossy().into_owned()),
        skills: Some(crate::config::SkillsConfig {
            scan_codewhale_only: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    };
    let app = App::new(options, &config);

    assert_eq!(app.skills_dir, configured_dir);
    assert!(
        app.cached_skills
            .iter()
            .any(|(name, _)| name == "workspace-codewhale"),
        "workspace CodeWhale skill should still be cached: {:?}",
        app.cached_skills
    );
    assert!(
        app.cached_skills
            .iter()
            .any(|(name, _)| name == "configured-skill"),
        "explicit configured skills_dir should still be cached: {:?}",
        app.cached_skills
    );
}

#[test]
fn cached_skills_reject_codewhale_only_workspace_symlink_escape() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspace");
    let escape_target = tmp.path().join("escape-target");
    let escaped_skill_dir = escape_target.join("escaped-skill");
    std::fs::create_dir_all(workspace.join(".codewhale")).expect("codewhale dir");
    std::fs::create_dir_all(&escaped_skill_dir).expect("escaped skill dir");
    std::fs::write(
        escaped_skill_dir.join("SKILL.md"),
        "---\nname: escaped-skill\ndescription: Escaped skill\n---\nbody\n",
    )
    .expect("write escaped skill");

    let link_path = workspace.join(".codewhale").join("skills");
    if create_dir_symlink(&escape_target, &link_path).is_err() {
        return;
    }

    let global_skills_dir = tmp.path().join("global-skills");
    let mut options = test_options(false);
    options.workspace = workspace.clone();
    options.skills_dir = global_skills_dir.clone();
    let config = Config {
        skills: Some(crate::config::SkillsConfig {
            scan_codewhale_only: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    };
    let app = App::new(options, &config);

    assert_eq!(app.skills_dir, global_skills_dir);
    assert!(
        !app.cached_skills
            .iter()
            .any(|(name, _)| name == "escaped-skill"),
        "strict app cache must not follow escaped workspace CodeWhale symlinks: {:?}",
        app.cached_skills
    );
}

#[test]
fn paste_defers_oversized_text_consolidation_until_submit() {
    // (#3263): a large paste stays inline so the user can still edit it.
    // At submit time, the full text is sent to the model with the @mention
    // appended so the model can also read the paste file backup.
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let mut opts = test_options(false);
    opts.workspace = tmp.path().to_path_buf();
    let mut app = App::new(opts, &Config::default());
    let full_content = "y".repeat(MAX_SUBMITTED_INPUT_CHARS + 256);

    app.insert_paste_text(&full_content);

    assert_eq!(app.input, full_content);
    assert_eq!(app.cursor_position, app.input.chars().count());
    let pastes_dir = tmp.path().join(".codewhale/pastes");
    assert!(
        !pastes_dir.exists() || std::fs::read_dir(&pastes_dir).unwrap().next().is_none(),
        "paste file should not be written before submit"
    );
    assert!(
        app.status_toasts
            .iter()
            .all(|toast| !toast.text.contains("backed up")),
        "backup toast should not appear before submit"
    );

    let submitted = app.submit_input().expect("expected submitted input");
    // The submitted text should contain the original content with the
    // @mention appended at the end (#3263).
    assert!(
        submitted.starts_with(&full_content),
        "submitted should contain full content, got: {}",
        &submitted[..submitted.len().min(80)]
    );
    let mention_start = full_content.len();
    assert!(
        submitted[mention_start..].starts_with("\n@.codewhale/pastes/paste-"),
        "expected @mention suffix, got: {}",
        &submitted[mention_start..]
    );
    assert!(submitted.ends_with(".md"), "expected .md extension");
    let mention = &submitted[mention_start + 2..]; // strip '\n@'
    let abs = tmp.path().join(mention);
    assert!(abs.is_file(), "paste file must exist at {abs:?}");
    let written = std::fs::read_to_string(&abs).expect("read");
    assert_eq!(written, full_content);
    assert!(
        app.status_toasts
            .iter()
            .any(|toast| toast.text.contains("backed up")),
        "expected backup toast after submit"
    );
}

#[test]
fn paste_under_threshold_does_not_consolidate() {
    // Negative path: a small paste must NOT spawn a paste file. The
    // input stays inline so the user can edit it freely.
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let mut opts = test_options(false);
    opts.workspace = tmp.path().to_path_buf();
    let mut app = App::new(opts, &Config::default());
    let small = "hello world\nthis is fine".to_string();

    app.insert_paste_text(&small);

    assert_eq!(app.input, small);
    assert!(!app.input.starts_with("@.codewhale/pastes/"));
    // No paste file gets written for under-cap pastes.
    let pastes_dir = tmp.path().join(".codewhale/pastes");
    assert!(
        !pastes_dir.exists() || std::fs::read_dir(&pastes_dir).unwrap().next().is_none(),
        "no paste file should be written for under-cap content"
    );
}

#[test]
fn submit_input_consolidates_oversized_input_into_paste_file() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let mut opts = test_options(false);
    opts.workspace = tmp.path().to_path_buf();
    let mut app = App::new(opts, &Config::default());
    let full_content = "x".repeat(MAX_SUBMITTED_INPUT_CHARS + 128);
    app.input = full_content.clone();
    app.cursor_position = app.input.chars().count();

    let submitted = app.submit_input().expect("expected submitted input");

    // The submitted text should still contain the original content, with
    // the @mention appended at the end so the model can read the file
    // while the composer stays editable for the user (#3263).
    assert!(
        submitted.starts_with(&full_content),
        "submitted text should contain original content, got: {}",
        &submitted[..submitted.len().min(80)]
    );
    let mention_start = full_content.len();
    assert!(
        submitted[mention_start..].starts_with("\n@.codewhale/pastes/paste-"),
        "submitted text should end with @mention, got suffix: {}",
        &submitted[mention_start..]
    );
    assert!(
        submitted.ends_with(".md"),
        "expected .md extension, got: {submitted}"
    );

    // The paste file must exist on disk with the full original content.
    let mention = &submitted[mention_start + 2..]; // strip leading '\n@'
    let abs_path = tmp.path().join(mention);
    assert!(abs_path.is_file(), "paste file must exist at {abs_path:?}");
    let written = std::fs::read_to_string(&abs_path).expect("read paste file");
    assert_eq!(written, full_content);

    // A status toast should have been pushed.
    assert!(
        app.status_toasts
            .iter()
            .any(|toast| toast.text.contains("backed up")),
        "expected backup toast, got: {:?}",
        app.status_toasts
            .iter()
            .map(|t| &t.text)
            .collect::<Vec<_>>()
    );

    // The composer must be clear after submit.
    assert!(app.input.is_empty());
}

#[test]
fn app_starts_without_seeded_transcript_messages() {
    let app = App::new(test_options(false), &Config::default());
    assert!(app.history.is_empty());
    assert_eq!(app.history_version, 0);
}

#[test]
fn clear_todos_resets_todos_list() {
    let mut app = App::new(test_options(false), &Config::default());

    // Seed some todos.
    {
        let mut todos = app.todos.try_lock().expect("todos lock");
        todos.add("buy milk".to_string(), TodoStatus::Pending);
        todos.add("write code".to_string(), TodoStatus::InProgress);
        assert_eq!(todos.snapshot().items.len(), 2);
    }

    assert!(app.clear_todos());

    let todos = app.todos.try_lock().expect("todos lock");
    assert!(todos.snapshot().items.is_empty());
}

#[test]
fn clear_todos_resets_plan_state() {
    let mut app = App::new(test_options(false), &Config::default());

    {
        let mut plan = app
            .plan_state
            .try_lock()
            .expect("plan lock should be available");
        plan.update(UpdatePlanArgs {
            explanation: Some("test plan".to_string()),
            plan: vec![PlanItemArg {
                step: "step 1".to_string(),
                status: StepStatus::InProgress,
            }],
            ..UpdatePlanArgs::default()
        });
        assert!(!plan.is_empty());
    }

    assert!(app.clear_todos());

    let plan = app
        .plan_state
        .try_lock()
        .expect("plan lock should be available");
    assert!(plan.is_empty());
}

#[test]
fn app_mode_helpers_centralize_parse_labels_and_cycle_order() {
    assert_eq!(AppMode::parse("agent"), Some(AppMode::Agent));
    assert_eq!(AppMode::parse("2"), Some(AppMode::Plan));
    assert_eq!(AppMode::parse("auto"), Some(AppMode::Agent));
    assert_eq!(AppMode::parse("3"), None);
    assert_eq!(AppMode::parse("YOLO"), Some(AppMode::Yolo));
    assert_eq!(AppMode::parse("4"), Some(AppMode::Yolo));
    assert_eq!(AppMode::parse("fast"), None);

    assert_eq!(AppMode::Agent.as_setting(), "agent");
    assert_eq!(AppMode::Auto.as_setting(), "auto");
    assert_eq!(AppMode::Plan.display_name(), "Plan");
    assert_eq!(AppMode::Auto.label(), "AUTO");
    assert_eq!(AppMode::Yolo.label(), "YOLO");
    assert_eq!(AppMode::Agent.number(), '1');
    assert_eq!(AppMode::Auto.number(), '3');
    assert_eq!(AppMode::Yolo.number(), '4');
    assert_eq!(
        AppMode::CHOICES,
        [AppMode::Agent, AppMode::Plan, AppMode::Yolo]
    );
    assert_eq!(
        AppMode::CYCLE,
        [AppMode::Plan, AppMode::Agent, AppMode::Yolo]
    );

    assert_eq!(AppMode::Plan.next(), AppMode::Agent);
    assert_eq!(AppMode::Agent.next(), AppMode::Yolo);
    assert_eq!(AppMode::Auto.next(), AppMode::Agent);
    assert_eq!(AppMode::Yolo.next(), AppMode::Plan);
    assert_eq!(AppMode::Plan.previous(), AppMode::Yolo);
    assert_eq!(AppMode::Agent.previous(), AppMode::Plan);
    assert_eq!(AppMode::Auto.previous(), AppMode::Agent);
    assert_eq!(AppMode::Yolo.previous(), AppMode::Agent);
}

#[test]
fn test_cycle_mode_transitions() {
    let mut app = App::new(test_options(false), &Config::default());
    let initial_mode = app.mode;
    app.cycle_mode();
    // Mode should have changed
    assert_ne!(app.mode, initial_mode);
}

#[test]
fn test_cycle_mode_reverse_transitions() {
    let mut app = App::new(test_options(false), &Config::default());

    app.mode = AppMode::Plan;
    app.cycle_mode_reverse();
    assert_eq!(app.mode, AppMode::Yolo);

    app.mode = AppMode::Yolo;
    app.cycle_mode_reverse();
    assert_eq!(app.mode, AppMode::Agent);

    app.mode = AppMode::Agent;
    app.cycle_mode_reverse();
    assert_eq!(app.mode, AppMode::Plan);

    app.mode = AppMode::Auto;
    app.cycle_mode_reverse();
    assert_eq!(app.mode, AppMode::Agent);
}

#[test]
fn test_mode_switch_toasts_replace_previous_mode_switch_toast() {
    let mut app = App::new(test_options(false), &Config::default());
    let first_mode = app.mode.next();
    let second_mode = first_mode.next();
    let third_mode = second_mode.next();

    app.set_mode(first_mode);
    app.sync_status_message_to_toasts();
    assert_eq!(app.status_toasts.len(), 1);
    assert_eq!(
        app.status_toasts.back().expect("mode toast").text,
        format!("Switched to {} mode", first_mode.label())
    );

    app.set_mode(second_mode);
    app.sync_status_message_to_toasts();
    assert_eq!(app.status_toasts.len(), 1);
    assert_eq!(
        app.status_toasts.back().expect("mode toast").text,
        format!("Switched to {} mode", second_mode.label())
    );

    app.set_mode(third_mode);
    app.sync_status_message_to_toasts();
    assert_eq!(app.status_toasts.len(), 1);
    assert_eq!(
        app.status_toasts.back().expect("mode toast").text,
        format!("Switched to {} mode", third_mode.label())
    );
}

#[test]
fn test_mode_switch_toasts_do_not_disrupt_non_mode_toasts() {
    let mut app = App::new(test_options(false), &Config::default());
    app.status_message = Some("Task queued".to_string());
    app.sync_status_message_to_toasts();

    app.set_mode(AppMode::Agent);
    app.sync_status_message_to_toasts();
    app.set_mode(AppMode::Yolo);
    app.sync_status_message_to_toasts();

    assert_eq!(app.status_toasts.len(), 2);
    assert!(
        app.status_toasts
            .iter()
            .any(|toast| toast.text == "Task queued")
    );
    assert!(
        app.status_toasts
            .iter()
            .any(|toast| toast.text == "Switched to YOLO mode")
    );
}

#[test]
fn test_clear_input() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "test input".to_string();
    app.cursor_position = app.input.len();
    app.clear_input();
    assert!(app.input.is_empty());
    assert_eq!(app.cursor_position, 0);
}

#[test]
fn test_queue_message() {
    let mut app = App::new(test_options(false), &Config::default());
    app.queue_message(QueuedMessage::new("test message".to_string(), None));
    assert_eq!(app.queued_message_count(), 1);
    assert!(app.queued_messages.front().is_some());
}

#[test]
fn test_remove_queued_message() {
    let mut app = App::new(test_options(false), &Config::default());
    app.queue_message(QueuedMessage::new("first".to_string(), None));
    app.queue_message(QueuedMessage::new("second".to_string(), None));

    // Remove first (index 0)
    let removed = app.remove_queued_message(0);
    assert!(removed.is_some());
    assert_eq!(app.queued_message_count(), 1);

    // Remove second (now at index 0)
    let removed = app.remove_queued_message(0);
    assert!(removed.is_some());
    assert_eq!(app.queued_message_count(), 0);
}

#[test]
fn test_remove_queued_message_invalid_index() {
    let mut app = App::new(test_options(false), &Config::default());
    app.queue_message(QueuedMessage::new("test".to_string(), None));

    // Try to remove non-existent index
    let removed = app.remove_queued_message(100);
    assert!(removed.is_none());
}

#[test]
fn test_set_mode_updates_state() {
    let mut app = App::new(test_options(false), &Config::default());
    let initial_mode = app.mode;
    app.set_mode(AppMode::Yolo);
    assert_eq!(app.mode, AppMode::Yolo);
    assert_ne!(app.mode, initial_mode);
    // Yolo mode should enable trust and shell
    assert!(app.trust_mode);
    assert!(app.allow_shell);
}

#[test]
fn app_new_respects_allow_shell_option_when_not_yolo() {
    let mut options = test_options(false);
    options.allow_shell = false;
    options.start_in_agent_mode = true; // avoid coupling to settings.default_mode
    let app = App::new(options, &Config::default());
    assert!(!app.allow_shell);
}

#[test]
fn set_mode_yolo_restores_previous_policies_on_exit() {
    let mut options = test_options(false);
    options.allow_shell = false;
    options.start_in_agent_mode = true; // avoid coupling to settings.default_mode
    let mut app = App::new(options, &Config::default());
    app.allow_shell = false;
    app.trust_mode = false;
    app.approval_mode = ApprovalMode::Never;

    app.set_mode(AppMode::Yolo);
    assert!(app.allow_shell);
    assert!(app.trust_mode);
    assert_eq!(app.approval_mode, ApprovalMode::Bypass);

    app.set_mode(AppMode::Agent);
    assert!(!app.allow_shell);
    assert!(!app.trust_mode);
    assert_eq!(app.approval_mode, ApprovalMode::Never);
}

#[test]
fn set_mode_plan_restores_previous_approval_on_agent_exit() {
    let config = Config {
        approval_policy: Some("never".to_string()),
        ..Default::default()
    };
    let mut options = test_options(false);
    options.start_in_agent_mode = true; // avoid coupling to settings.default_mode
    let mut app = App::new(options, &config);
    assert_eq!(app.mode, AppMode::Agent);
    assert_eq!(app.approval_mode, ApprovalMode::Never);

    app.set_mode(AppMode::Plan);
    app.approval_mode = ApprovalMode::Suggest;

    app.set_mode(AppMode::Agent);
    assert_eq!(app.mode, AppMode::Agent);
    assert_eq!(app.approval_mode, ApprovalMode::Never);
}

#[test]
fn set_mode_plan_to_yolo_keeps_yolo_permissions_and_restores_agent_baseline() {
    let mut options = test_options(false);
    options.allow_shell = false;
    options.start_in_agent_mode = true; // avoid coupling to settings.default_mode
    let mut app = App::new(options, &Config::default());
    app.allow_shell = false;
    app.trust_mode = false;
    app.approval_mode = ApprovalMode::Never;

    app.set_mode(AppMode::Plan);
    app.approval_mode = ApprovalMode::Suggest;

    app.set_mode(AppMode::Yolo);
    assert_eq!(app.mode, AppMode::Yolo);
    assert!(app.allow_shell);
    assert!(app.trust_mode);
    assert_eq!(app.approval_mode, ApprovalMode::Bypass);

    app.set_mode(AppMode::Agent);
    assert_eq!(app.mode, AppMode::Agent);
    assert!(!app.allow_shell);
    assert!(!app.trust_mode);
    assert_eq!(app.approval_mode, ApprovalMode::Never);
}

#[test]
fn base_policy_for_mode_projects_the_mode_permission_table() {
    // Pure projection of (mode, prefs) — the single source of truth for #3386.
    let prefs = ModeSessionPrefs {
        agent_allow_shell: true,
        agent_trust_mode: true,
        agent_approval_mode: ApprovalMode::Never,
    };

    // Plan: read-only, no shell, no trust, Suggest — and it never inherits the
    // (here elevated) Agent baseline.
    let plan = base_policy_for_mode(AppMode::Plan, &prefs);
    assert_eq!(plan.mode, AppMode::Plan);
    assert!(!plan.allow_shell);
    assert!(!plan.trust_mode);
    assert_eq!(plan.approval_mode, ApprovalMode::Suggest);

    // Agent: exactly the durable baseline.
    let agent = base_policy_for_mode(AppMode::Agent, &prefs);
    assert_eq!(agent.mode, AppMode::Agent);
    assert!(agent.allow_shell);
    assert!(agent.trust_mode);
    assert_eq!(agent.approval_mode, ApprovalMode::Never);

    // Auto: shell-enabled smart review, no trust authority.
    let auto = base_policy_for_mode(AppMode::Auto, &prefs);
    assert_eq!(auto.mode, AppMode::Auto);
    assert!(auto.allow_shell);
    assert!(!auto.trust_mode);
    assert_eq!(auto.approval_mode, ApprovalMode::Auto);

    // YOLO: full authority is represented by Bypass, not a separate
    // auto-approve field (#3736).
    let yolo = base_policy_for_mode(AppMode::Yolo, &prefs);
    assert_eq!(yolo.mode, AppMode::Yolo);
    assert!(yolo.allow_shell);
    assert!(yolo.trust_mode);
    assert_eq!(yolo.approval_mode, ApprovalMode::Bypass);

    // A minimal Agent baseline projects through Agent unchanged.
    let minimal = ModeSessionPrefs {
        agent_allow_shell: false,
        agent_trust_mode: false,
        agent_approval_mode: ApprovalMode::Suggest,
    };
    let agent_min = base_policy_for_mode(AppMode::Agent, &minimal);
    assert!(!agent_min.allow_shell);
    assert!(!agent_min.trust_mode);
    assert_eq!(agent_min.approval_mode, ApprovalMode::Suggest);
}

#[test]
fn set_mode_agent_to_yolo_to_agent_restores_baseline_without_yolo_leak() {
    // Round-trip Agent -> YOLO -> Agent must not leave YOLO's elevated authority
    // (shell/trust/Auto) bleeding into the restored Agent surface (#3386).
    let mut options = test_options(false);
    options.allow_shell = false;
    options.start_in_agent_mode = true;
    let mut app = App::new(options, &Config::default());
    // User's chosen Agent surface: shell on, trust off, Suggest approvals.
    app.allow_shell = true;
    app.trust_mode = false;
    app.approval_mode = ApprovalMode::Suggest;

    app.set_mode(AppMode::Yolo);
    assert!(app.allow_shell);
    assert!(app.trust_mode);
    assert_eq!(app.approval_mode, ApprovalMode::Bypass);
    assert!(app.yolo);

    app.set_mode(AppMode::Agent);
    assert_eq!(app.mode, AppMode::Agent);
    assert!(app.allow_shell, "shell baseline preserved");
    assert!(
        !app.trust_mode,
        "YOLO trust authority must not leak into Agent"
    );
    assert_eq!(
        app.approval_mode,
        ApprovalMode::Suggest,
        "YOLO Auto approvals must not leak into Agent"
    );
    assert!(!app.yolo);
}

#[test]
fn set_mode_plan_to_yolo_to_agent_does_not_bleed_yolo_into_agent() {
    // Plan -> YOLO -> Agent: the Agent baseline captured before leaving Agent is
    // what we land on, untouched by the transient Plan or YOLO policies (#3386).
    let mut options = test_options(false);
    options.allow_shell = false;
    options.start_in_agent_mode = true;
    let mut app = App::new(options, &Config::default());
    app.allow_shell = false;
    app.trust_mode = false;
    app.approval_mode = ApprovalMode::Never;

    app.set_mode(AppMode::Plan);
    // Plan is read-only regardless of the baseline.
    assert!(!app.allow_shell);
    assert!(!app.trust_mode);
    assert_eq!(app.approval_mode, ApprovalMode::Suggest);

    app.set_mode(AppMode::Yolo);
    assert!(app.allow_shell);
    assert!(app.trust_mode);
    assert_eq!(app.approval_mode, ApprovalMode::Bypass);

    app.set_mode(AppMode::Agent);
    assert_eq!(app.mode, AppMode::Agent);
    assert!(!app.allow_shell);
    assert!(!app.trust_mode);
    assert_eq!(app.approval_mode, ApprovalMode::Never);
}

#[test]
fn set_mode_captures_agent_edits_as_the_durable_baseline() {
    // Editing the permission surface in Agent updates the baseline that a later
    // Plan -> Agent (or YOLO -> Agent) restores to (#3386).
    let mut options = test_options(false);
    options.allow_shell = false;
    options.start_in_agent_mode = true;
    let mut app = App::new(options, &Config::default());
    assert_eq!(app.mode, AppMode::Agent);

    // Initial baseline restores to no-shell / Suggest.
    app.set_mode(AppMode::Plan);
    app.set_mode(AppMode::Agent);
    assert!(!app.allow_shell);
    assert_eq!(app.approval_mode, ApprovalMode::Suggest);

    // User now turns shell on and tightens approvals while in Agent.
    app.allow_shell = true;
    app.approval_mode = ApprovalMode::Never;

    // A Plan hop and back must restore the *edited* baseline, not the original.
    app.set_mode(AppMode::Plan);
    assert!(!app.allow_shell, "Plan is read-only");
    app.set_mode(AppMode::Agent);
    assert!(app.allow_shell, "edited shell baseline restored");
    assert_eq!(app.approval_mode, ApprovalMode::Never);
}

#[test]
fn leaving_yolo_after_startup_restores_baseline_policies() {
    let config = Config {
        allow_shell: Some(false),
        ..Default::default()
    };

    let mut app = App::new(test_options(true), &config);
    assert_eq!(app.mode, AppMode::Yolo);
    assert!(app.allow_shell);
    assert!(app.trust_mode);
    assert_eq!(app.approval_mode, ApprovalMode::Bypass);

    app.set_mode(AppMode::Agent);
    assert!(!app.allow_shell);
    assert!(!app.trust_mode);
    assert_eq!(app.approval_mode, ApprovalMode::Suggest);
}

#[test]
fn configured_approval_policy_initializes_live_approval_mode() {
    let config = Config {
        approval_policy: Some("never".to_string()),
        ..Default::default()
    };
    let mut options = test_options(false);
    options.start_in_agent_mode = true;

    let app = App::new(options, &config);

    assert_eq!(app.mode, AppMode::Agent);
    assert_eq!(app.approval_mode, ApprovalMode::Never);
}

#[test]
fn test_mark_history_updated() {
    let mut app = App::new(test_options(false), &Config::default());
    let initial_version = app.history_version;
    app.mark_history_updated();
    assert!(app.history_version > initial_version);
}

#[test]
fn expanded_tool_runs_rebase_when_history_prefix_shifts() {
    let mut app = App::new(test_options(false), &Config::default());
    app.expanded_tool_runs = std::collections::HashSet::from([2usize, 6usize]);

    app.shift_history_maps_down(3);

    assert_eq!(app.expanded_tool_runs, std::collections::HashSet::from([3]));
}

#[test]
fn expanded_tool_runs_prune_when_history_is_truncated() {
    let mut app = App::new(test_options(false), &Config::default());
    for idx in 0..5 {
        app.add_message(HistoryCell::System {
            content: format!("cell {idx}"),
        });
    }
    app.expanded_tool_runs = std::collections::HashSet::from([1usize, 4usize]);

    app.truncate_history_to(3);

    assert_eq!(app.expanded_tool_runs, std::collections::HashSet::from([1]));
}

#[test]
fn tool_run_expansion_toggle_opens_and_closes_run() {
    let mut app = App::new(test_options(false), &Config::default());
    app.tool_collapse_mode = ToolCollapseMode::Compact;
    app.tool_collapse_threshold = 3;
    for name in ["read_file", "list_dir", "web_search"] {
        app.add_message(HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: name.to_string(),
            status: ToolStatus::Success,
            input_summary: None,
            output: Some("ok".to_string()),
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        })));
    }

    assert!(app.toggle_tool_run_expansion_at(0));
    assert!(app.expanded_tool_runs.contains(&0));
    assert!(app.toggle_tool_run_expansion_at(2));
    assert!(!app.expanded_tool_runs.contains(&0));
    assert!(!app.toggle_tool_run_expansion_at(99));
}

#[test]
fn test_scroll_operations() {
    let mut app = App::new(test_options(false), &Config::default());
    // Just verify scroll methods can be called without panic
    app.scroll_up(5);
    app.scroll_down(3);
}

#[test]
fn resize_preserves_scrolled_transcript_position() {
    let mut app = App::new(test_options(false), &Config::default());
    app.viewport.transcript_scroll = TranscriptScroll::at_line(42);
    app.viewport.last_transcript_top = 42;
    app.viewport.pending_scroll_delta = 5;

    app.handle_resize(120, 40);

    let meta = vec![TranscriptLineMeta::Spacer; 240];
    let (_, top) = app.viewport.transcript_scroll.resolve_top(&meta, 200);
    assert_eq!(top, 42);
    assert_eq!(app.viewport.pending_scroll_delta, 0);
}

#[test]
fn resize_keeps_tail_state_when_user_was_at_tail() {
    let mut app = App::new(test_options(false), &Config::default());
    app.viewport.transcript_scroll = TranscriptScroll::to_bottom();
    app.viewport.last_transcript_top = 42;

    app.handle_resize(120, 40);

    assert!(app.viewport.transcript_scroll.is_at_tail());
}

#[test]
fn resize_seeds_visible_height_for_paging_before_next_render() {
    let mut app = App::new(test_options(false), &Config::default());
    app.viewport.last_transcript_visible = 12;

    app.handle_resize(120, 40);
    assert_eq!(app.viewport.last_transcript_visible, 38);

    app.handle_resize(120, 1);
    assert_eq!(app.viewport.last_transcript_visible, 1);
}

#[test]
fn test_add_message() {
    let mut app = App::new(test_options(false), &Config::default());
    let initial_len = app.history.len();
    app.add_message(HistoryCell::User {
        content: "test".to_string(),
    });
    assert_eq!(app.history.len(), initial_len + 1);
}

#[test]
fn test_compaction_config() {
    let mut app = App::new(test_options(false), &Config::default());
    let config = app.compaction_config();
    // Config should be valid (just checking it returns something)
    let _ = config.enabled;

    app.auto_model = true;
    app.model = "auto".to_string();
    app.last_effective_model = None;
    let config = app.compaction_config();
    assert_eq!(config.model, DEFAULT_TEXT_MODEL);

    app.last_effective_model = Some("deepseek-v4-flash".to_string());
    let config = app.compaction_config();
    assert_eq!(config.model, "deepseek-v4-flash");
}

#[test]
fn compaction_config_respects_config_enabled_override() {
    let config = Config {
        compaction: CompactionRuntimeConfig {
            enabled: Some(false),
        },
        ..Default::default()
    };
    let mut app = App::new(test_options(false), &config);
    app.auto_compact = true;
    assert!(!app.compaction_config().enabled);

    let config = Config {
        compaction: CompactionRuntimeConfig {
            enabled: Some(true),
        },
        ..Default::default()
    };
    let mut app = App::new(test_options(false), &config);
    app.auto_compact = false;
    assert!(app.compaction_config().enabled);
}

#[test]
fn test_update_model_compaction_budget() {
    let mut app = App::new(test_options(false), &Config::default());
    // Pin the inputs so the budget math is deterministic and does not
    // depend on the developer's local `auto_compact_threshold_percent`
    // setting (App::new loads real settings) or on auto-model resolution.
    app.auto_model = false;
    app.active_route_limits = None;
    app.active_context_window_override = None;
    app.auto_compact_threshold_percent = 80.0;

    // A large-context model earns a proportionally larger compaction
    // budget; an unknown model falls back to the fixed default threshold.
    app.model = "deepseek-v4-pro".to_string();
    app.update_model_compaction_budget();
    let large_window_threshold = app.compact_threshold;

    app.model = "unknown-test-model".to_string();
    app.update_model_compaction_budget();
    let unknown_threshold = app.compact_threshold;

    assert!(
        unknown_threshold > 0,
        "unknown model must still get a positive budget"
    );
    assert!(
        large_window_threshold > unknown_threshold,
        "a large-context model ({large_window_threshold}) should budget more \
         than an unknown model ({unknown_threshold})"
    );
}

#[test]
fn test_input_history_navigation() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input_history.push("first".to_string());
    app.input_history.push("second".to_string());

    // Navigate up
    app.history_up();
    assert!(app.history_index.is_some());

    // Navigate down
    app.history_down();
}

#[test]
fn input_history_down_restores_live_draft_after_accidental_up() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input_history.push("previous prompt".to_string());
    app.input = "careful current draft".to_string();
    app.cursor_position = "careful".chars().count();

    app.history_up();
    assert_eq!(app.input, "previous prompt");

    app.history_down();
    assert_eq!(app.input, "careful current draft");
    assert_eq!(app.cursor_position, "careful".chars().count());
    assert!(app.history_index.is_none());
}

#[test]
fn input_history_navigation_clears_stale_selection() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input_history.push("previous input".to_string());
    app.input = "hello world".to_string();
    app.cursor_position = "hello ".chars().count();
    app.selection_anchor = Some(app.input.chars().count());

    app.history_up();
    assert_eq!(app.input, "previous input");
    assert!(app.selection_anchor.is_none());

    app.insert_char('x');
    assert_eq!(app.input, "previous inputx");
}

#[test]
fn input_history_restores_empty_draft_at_end_of_navigation() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input_history.push("previous prompt".to_string());

    app.history_up();
    assert_eq!(app.input, "previous prompt");

    app.history_down();
    assert!(app.input.is_empty());
    assert_eq!(app.cursor_position, 0);
    assert!(app.history_index.is_none());
}

#[test]
fn word_cursor_helpers_move_by_whitespace_delimited_words() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "alpha beta  gamma".to_string();
    app.cursor_position = 0;

    app.move_cursor_word_forward();
    assert_eq!(app.cursor_position, "alpha ".chars().count());

    app.move_cursor_word_forward();
    assert_eq!(app.cursor_position, "alpha beta  ".chars().count());

    app.move_cursor_word_backward();
    assert_eq!(app.cursor_position, "alpha ".chars().count());
}

#[test]
fn editing_history_entry_leaves_navigation_mode() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input_history.push("previous prompt".to_string());
    app.input = "current draft".to_string();
    app.cursor_position = app.input.chars().count();

    app.history_up();
    app.insert_char('!');
    app.history_down();

    assert_eq!(app.input, "previous prompt!");
    assert!(app.history_index.is_none());
}

#[test]
fn history_search_filters_matches_and_skips_duplicates() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input_history.clear();
    app.input_history.push("alpha one".to_string());
    app.input_history.push("beta two".to_string());
    app.input_history.push("alpha one".to_string());
    app.draft_history.push_back("draft alpha".to_string());

    app.start_history_search();
    app.history_search_insert_str("alpha");

    assert_eq!(
        app.history_search_matches(),
        vec!["draft alpha".to_string(), "alpha one".to_string()]
    );
}

#[test]
fn history_search_matches_unicode_case_insensitively() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input_history.clear();
    app.input_history.push("CAFÉ prompt".to_string());

    app.start_history_search();
    app.history_search_insert_str("café");

    assert_eq!(
        app.history_search_matches(),
        vec!["CAFÉ prompt".to_string()]
    );
}

#[test]
fn history_search_accepts_match_without_submitting() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input_history.clear();
    app.input_history.push("older prompt".to_string());

    app.start_history_search();
    app.history_search_insert_str("older");

    assert!(app.accept_history_search());
    assert_eq!(app.input, "older prompt");
    assert_eq!(app.cursor_position, "older prompt".chars().count());
    assert!(app.composer_history_search.is_none());
}

#[test]
fn history_search_cancel_restores_pre_search_draft() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input_history.clear();
    app.input = "current draft".to_string();
    app.cursor_position = 7;
    app.input_history.push("older prompt".to_string());

    app.start_history_search();
    app.history_search_insert_str("older");
    app.cancel_history_search();

    assert_eq!(app.input, "current draft");
    assert_eq!(app.cursor_position, 7);
    assert!(app.composer_history_search.is_none());
}

#[test]
fn recoverable_clear_stashes_nonempty_draft() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input_history.clear();
    app.input = "recover this".to_string();
    app.cursor_position = app.input.chars().count();

    app.clear_input_recoverable();
    app.start_history_search();
    app.history_search_insert_str("recover");

    assert_eq!(
        app.history_search_matches(),
        vec!["recover this".to_string()]
    );
}

#[test]
fn clear_undo_buffer_is_set_on_clear_input_recoverable() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "hello".to_string();
    app.cursor_position = 5;

    app.clear_input_recoverable();

    assert!(app.input.is_empty());
    assert_eq!(app.clear_undo_buffer.as_deref(), Some("hello"));
}

#[test]
fn clear_undo_buffer_is_none_when_clearing_empty_input() {
    let mut app = App::new(test_options(false), &Config::default());
    assert!(app.input.is_empty());

    app.clear_input_recoverable();

    assert!(app.clear_undo_buffer.is_none());
}

#[test]
fn restore_last_cleared_input_restores_saved_draft() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "previous".to_string();
    app.cursor_position = 8;
    app.clear_input_recoverable();
    assert!(app.input.is_empty());

    let restored = app.restore_last_cleared_input_if_empty();
    assert!(restored);
    assert_eq!(app.input, "previous");
    assert!(app.clear_undo_buffer.is_none());
}

#[test]
fn restore_last_cleared_input_does_nothing_when_composer_not_empty() {
    let mut app = App::new(test_options(false), &Config::default());
    app.clear_undo_buffer = Some("old".to_string());
    app.input = "current".to_string();
    assert!(!app.restore_last_cleared_input_if_empty());
}

#[test]
fn composer_paste_flushes_pending_burst_and_normalizes_crlf() {
    let mut app = App::new(test_options(false), &Config::default());
    app.use_paste_burst_detection = true;
    let now = Instant::now();
    let key = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('x'),
        crossterm::event::KeyModifiers::NONE,
    );

    assert!(crate::tui::paste::handle_paste_burst_key(
        &mut app, &key, now
    ));
    assert!(
        app.input.is_empty(),
        "first burst char should stay buffered"
    );

    app.insert_paste_text("a\r\nb\rc");

    assert_eq!(app.input, "xa\nb\nc");
    assert_eq!(app.cursor_position, "xa\nb\nc".chars().count());
    assert!(!app.paste_burst.is_active());
}

#[test]
fn bracketed_paste_preserves_bare_carriage_return_line_breaks() {
    let mut app = App::new(test_options(false), &Config::default());

    app.insert_paste_text("alpha\r  indented\r# literal heading\r- literal list");

    assert_eq!(
        app.input,
        "alpha\n  indented\n# literal heading\n- literal list"
    );
    assert_eq!(app.cursor_position, app.input.chars().count());
}

#[test]
fn enter_during_active_paste_burst_appends_newline_to_buffer_not_submit() {
    // #1073: when chars are still being assembled into a paste burst and
    // an Enter arrives (the trailing newline of the paste), the Enter
    // must be absorbed into the burst buffer — not fired as a submit.
    let mut app = App::new(test_options(false), &Config::default());
    app.use_paste_burst_detection = true;
    let now = Instant::now();
    app.paste_burst.append_char_to_buffer('h', now);
    app.paste_burst.append_char_to_buffer('i', now);
    assert!(app.paste_burst.is_active());
    assert!(app.input.is_empty());

    let result = app.handle_composer_enter();

    assert!(
        result.is_none(),
        "Enter during active paste burst must not submit"
    );
    let flushed = app.paste_burst.flush_before_modified_input();
    assert_eq!(
        flushed.as_deref(),
        Some("hi\n"),
        "newline must land in the burst buffer so the next flush carries it"
    );
}

#[test]
fn enter_inside_paste_burst_window_after_flush_inserts_newline_not_submit() {
    // #1073: after a burst has flushed (text now in `input`), the
    // suppression window stays open for ~120ms. An Enter arriving in
    // that window is the trailing newline of the paste, not a user
    // submit — insert it as a literal newline into the composer.
    let mut app = App::new(test_options(false), &Config::default());
    app.use_paste_burst_detection = true;
    app.input = "hello".to_string();
    app.cursor_position = "hello".chars().count();
    let now = Instant::now();
    app.paste_burst.extend_window(now);
    assert!(!app.paste_burst.is_active());
    assert!(
        app.paste_burst.newline_should_insert_instead_of_submit(now),
        "suppression window should be open"
    );

    let result = app.handle_composer_enter();

    assert!(
        result.is_none(),
        "Enter inside post-flush suppression window must not submit"
    );
    assert_eq!(
        app.input, "hello\n",
        "newline must be inserted into the composer instead of firing a submit"
    );
}

#[test]
fn enter_outside_any_paste_burst_window_submits_normally() {
    // Regression guard: the suppression must not trip when the user
    // actually wants to submit.
    let mut app = App::new(test_options(false), &Config::default());
    app.use_paste_burst_detection = true;
    app.input = "hello world".to_string();
    app.cursor_position = "hello world".chars().count();

    let result = app.handle_composer_enter();

    assert_eq!(
        result.as_deref(),
        Some("hello world"),
        "Enter outside any paste burst window must submit normally"
    );
    assert!(
        app.input.is_empty(),
        "submit_input should clear the composer"
    );
}

#[test]
fn enter_with_paste_burst_detection_disabled_submits_normally() {
    // When the user has explicitly turned off paste-burst detection
    // (`bracketed_paste = false` is independent, this is the
    // `paste_burst_detection` setting), the suppression must be
    // skipped — otherwise turning it off would not actually turn it
    // off.
    let mut app = App::new(test_options(false), &Config::default());
    app.use_paste_burst_detection = false;
    app.input = "ship it".to_string();
    app.cursor_position = "ship it".chars().count();
    let now = Instant::now();
    app.paste_burst.extend_window(now);

    let result = app.handle_composer_enter();

    assert_eq!(result.as_deref(), Some("ship it"));
}

#[test]
fn clipboard_text_paste_matches_bracketed_paste_state() {
    let text = "alpha\r\nbeta";
    let mut bracketed = App::new(test_options(false), &Config::default());
    let mut clipboard = App::new(test_options(false), &Config::default());

    bracketed.insert_paste_text(text);
    clipboard.apply_clipboard_content(ClipboardContent::Text(text.to_string()));

    assert_eq!(clipboard.input, bracketed.input);
    assert_eq!(clipboard.cursor_position, bracketed.cursor_position);
    assert_eq!(clipboard.slash_menu_hidden, bracketed.slash_menu_hidden);
    assert_eq!(clipboard.mention_menu_hidden, bracketed.mention_menu_hidden);
}

#[test]
fn clipboard_image_paste_keeps_adjacent_text_and_concise_status() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "before after".to_string();
    app.cursor_position = "before".chars().count();

    app.apply_clipboard_content(ClipboardContent::Image(PastedImage {
        path: PathBuf::from("/tmp/pasted.png"),
        width: 8,
        height: 4,
        byte_len: 2048,
    }));

    assert!(
        app.input
            .contains("before\n[Attached image: 8x4 PNG (2KB) at /tmp/pasted.png]")
    );
    assert!(app.input.contains("] after"));
    let status = app.status_message.as_deref().expect("status message");
    assert_eq!(status, "Attached image: 8x4 PNG (2KB)");
}

#[test]
fn pasted_text_and_image_placeholders_survive_history_and_queue_paths() {
    let mut app = App::new(test_options(false), &Config::default());
    app.insert_paste_text("line 1\r\nline 2");
    app.insert_media_attachment("image", Path::new("/tmp/pasted.png"), Some("8x4 PNG (2KB)"));

    let submitted = app.submit_input().expect("submitted input");
    assert!(submitted.contains("line 1\nline 2"));
    assert!(submitted.contains("[Attached image: 8x4 PNG (2KB) at /tmp/pasted.png]"));

    app.history_up();
    assert_eq!(app.input, submitted);
    assert_eq!(app.composer_attachment_count(), 1);

    app.clear_input();
    app.queue_message(QueuedMessage::new(
        submitted.clone(),
        Some("Use this skill".to_string()),
    ));
    assert!(app.pop_last_queued_into_draft());
    assert_eq!(app.input, submitted);
    assert_eq!(app.composer_attachment_count(), 1);
    assert_eq!(
        app.queued_draft
            .as_ref()
            .and_then(|draft| draft.skill_instruction.as_deref()),
        Some("Use this skill")
    );

    app.push_pending_steer(QueuedMessage::new(submitted.clone(), None));
    let steers = app.drain_pending_steers();
    assert_eq!(steers[0].display, submitted);
}

#[test]
fn selected_attachment_row_removes_placeholder_without_manual_editing() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "before".to_string();
    app.cursor_position = "before".chars().count();
    app.insert_media_attachment("image", Path::new("/tmp/pasted.png"), Some("8x4 PNG"));
    app.insert_str("after");

    app.move_cursor_start();
    assert!(app.select_previous_composer_attachment());
    assert_eq!(app.selected_composer_attachment_index(), Some(0));
    assert!(app.remove_selected_composer_attachment());

    assert!(!app.input.contains("[Attached image:"));
    assert!(app.input.contains("before"));
    assert!(app.input.contains("after"));
    assert_eq!(app.composer_attachment_count(), 0);
    assert!(app.selected_composer_attachment_index().is_none());
}

#[test]
fn kill_to_end_of_line_cuts_from_middle_of_word() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "hello world".to_string();
    app.cursor_position = 6; // before 'w'
    assert!(app.kill_to_end_of_line());
    assert_eq!(app.input, "hello ");
    assert_eq!(app.cursor_position, 6);
    assert_eq!(app.kill_buffer, "world");
}

#[test]
fn kill_at_eol_consumes_following_newline() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "line one\nline two".to_string();
    app.cursor_position = 8; // sitting on the '\n'
    assert!(app.kill_to_end_of_line());
    assert_eq!(app.input, "line oneline two");
    assert_eq!(app.cursor_position, 8);
    assert_eq!(app.kill_buffer, "\n");

    // Empty input: kill is a no-op and the buffer is untouched.
    let mut empty = App::new(test_options(false), &Config::default());
    assert!(!empty.kill_to_end_of_line());
    assert!(empty.input.is_empty());
    assert!(empty.kill_buffer.is_empty());
}

#[test]
fn yank_inserts_kill_buffer_and_preserves_it() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "abc def".to_string();
    app.cursor_position = 4; // before 'd'
    assert!(app.kill_to_end_of_line());
    assert_eq!(app.input, "abc ");
    assert_eq!(app.kill_buffer, "def");

    // Move cursor to the start and yank twice — kill_buffer must persist.
    app.cursor_position = 0;
    assert!(app.yank());
    assert!(app.yank());
    assert_eq!(app.input, "defdefabc ");
    assert_eq!(app.cursor_position, 6);
    assert_eq!(app.kill_buffer, "def");

    // Yank with empty buffer is a no-op.
    let mut empty = App::new(test_options(false), &Config::default());
    assert!(!empty.yank());
    assert!(empty.input.is_empty());
}

// ---- Issue #90: quit confirmation timeout ----

#[test]
fn quit_is_not_armed_by_default() {
    let app = App::new(test_options(false), &Config::default());
    assert!(!app.quit_is_armed());
    assert!(app.quit_armed_until.is_none());
}

#[test]
fn arm_quit_sets_two_second_window() {
    let mut app = App::new(test_options(false), &Config::default());
    app.arm_quit();
    assert!(app.quit_is_armed());
    let deadline = app.quit_armed_until.expect("deadline set");
    let remaining = deadline.saturating_duration_since(Instant::now());
    // Allow a generous margin for slow CI machines: 1.5s..=2.0s.
    assert!(
        remaining >= Duration::from_millis(1500) && remaining <= Duration::from_secs(2),
        "expected ~2s window, got {remaining:?}",
    );
    assert!(app.needs_redraw, "armed prompt should request a redraw");
}

#[test]
fn disarm_quit_clears_the_timer() {
    let mut app = App::new(test_options(false), &Config::default());
    app.arm_quit();
    app.needs_redraw = false;
    app.disarm_quit();
    assert!(!app.quit_is_armed());
    assert!(app.quit_armed_until.is_none());
    assert!(app.needs_redraw, "disarming should request a redraw");
}

#[test]
fn disarm_quit_when_not_armed_is_a_noop() {
    let mut app = App::new(test_options(false), &Config::default());
    app.needs_redraw = false;
    app.disarm_quit();
    assert!(!app.needs_redraw, "no redraw when nothing changed");
}

#[test]
fn quit_armed_expires_after_window() {
    let mut app = App::new(test_options(false), &Config::default());
    // Pin the deadline in the past to simulate a stale timer.
    app.quit_armed_until = Some(Instant::now() - Duration::from_millis(10));
    assert!(
        !app.quit_is_armed(),
        "expired timer must not count as armed"
    );

    app.needs_redraw = false;
    app.tick_quit_armed();
    assert!(app.quit_armed_until.is_none(), "tick clears expired timer");
    assert!(
        app.needs_redraw,
        "expiry triggers a redraw to repaint footer"
    );
}

#[test]
fn receipt_expires_and_requests_redraw() {
    let mut app = App::new(test_options(false), &Config::default());
    app.set_receipt_text("✓ turn completed");
    app.receipt_started_at =
        Some(Instant::now() - App::RECEIPT_VISIBLE_DURATION - Duration::from_millis(10));
    assert_eq!(app.active_receipt_text(), None);

    app.needs_redraw = false;
    app.tick_receipt();
    assert!(app.receipt_text.is_none());
    assert!(app.receipt_started_at.is_none());
    assert!(
        app.needs_redraw,
        "receipt expiry should repaint composer chrome"
    );
}

#[test]
fn quit_armed_tick_is_noop_within_window() {
    let mut app = App::new(test_options(false), &Config::default());
    app.arm_quit();
    app.needs_redraw = false;
    app.tick_quit_armed();
    assert!(
        app.quit_is_armed(),
        "tick within window keeps the timer armed"
    );
    assert!(!app.needs_redraw, "no redraw when nothing changed");
}

#[test]
fn re_arming_after_expiry_starts_a_fresh_window() {
    let mut app = App::new(test_options(false), &Config::default());
    app.quit_armed_until = Some(Instant::now() - Duration::from_secs(5));
    app.tick_quit_armed();
    assert!(app.quit_armed_until.is_none());
    app.arm_quit();
    let deadline = app.quit_armed_until.expect("re-armed");
    assert!(deadline > Instant::now(), "fresh deadline in the future");
}

// ---- Issue #208: in-flight input routing ----

#[test]
fn submit_disposition_immediate_when_idle_and_online() {
    let app = App::new(test_options(false), &Config::default());
    assert!(!app.is_loading);
    assert!(!app.offline_mode);
    assert_eq!(
        app.decide_submit_disposition(),
        SubmitDisposition::Immediate
    );
}

#[test]
fn submit_disposition_steer_when_busy_and_online_not_streaming() {
    // v0.8.44: Busy + not streaming → Steer (Enter reaches engine during
    // sub-agent/shell waits instead of silently queueing).
    let mut app = App::new(test_options(false), &Config::default());
    app.is_loading = true;
    app.offline_mode = false;
    // streaming_message_index is None (default) → tool execution phase
    assert_eq!(app.decide_submit_disposition(), SubmitDisposition::Steer);
}

#[test]
fn submit_disposition_queue_when_busy_and_streaming() {
    // #382: Busy + streaming → Queue (was QueueFollowUp; now unified)
    let mut app = App::new(test_options(false), &Config::default());
    app.is_loading = true;
    app.offline_mode = false;
    app.streaming_message_index = Some(0);
    assert_eq!(app.decide_submit_disposition(), SubmitDisposition::Queue);
}

#[test]
fn submit_disposition_queue_when_offline_and_idle() {
    let mut app = App::new(test_options(false), &Config::default());
    app.is_loading = false;
    app.offline_mode = true;
    assert_eq!(app.decide_submit_disposition(), SubmitDisposition::Queue);
}

#[test]
fn submit_disposition_offline_busy_queues() {
    let mut app = App::new(test_options(false), &Config::default());
    app.is_loading = true;
    app.offline_mode = true;
    // Offline mode always queues, even when streaming
    app.streaming_message_index = Some(0);
    assert_eq!(app.decide_submit_disposition(), SubmitDisposition::Queue);
}

#[test]
fn push_pending_steer_arms_resend_flag() {
    let mut app = App::new(test_options(false), &Config::default());
    assert!(!app.submit_pending_steers_after_interrupt);
    app.push_pending_steer(QueuedMessage::new("steer me".to_string(), None));
    assert_eq!(app.pending_steers.len(), 1);
    assert!(app.submit_pending_steers_after_interrupt);
}

#[test]
fn drain_pending_steers_clears_flag_and_returns_in_order() {
    let mut app = App::new(test_options(false), &Config::default());
    app.push_pending_steer(QueuedMessage::new("first".to_string(), None));
    app.push_pending_steer(QueuedMessage::new("second".to_string(), None));
    app.push_pending_steer(QueuedMessage::new("third".to_string(), None));

    let drained = app.drain_pending_steers();
    assert_eq!(drained.len(), 3);
    assert_eq!(drained[0].display, "first");
    assert_eq!(drained[2].display, "third");
    assert!(app.pending_steers.is_empty());
    assert!(!app.submit_pending_steers_after_interrupt);
}

#[test]
fn drain_pending_steers_when_empty_is_safe() {
    let mut app = App::new(test_options(false), &Config::default());
    // Flag-only set (someone armed it manually): drain still clears it.
    app.submit_pending_steers_after_interrupt = true;
    let drained = app.drain_pending_steers();
    assert!(drained.is_empty());
    assert!(!app.submit_pending_steers_after_interrupt);
}

#[test]
fn double_push_pending_steer_is_idempotent_on_flag() {
    let mut app = App::new(test_options(false), &Config::default());
    app.push_pending_steer(QueuedMessage::new("a".to_string(), None));
    app.push_pending_steer(QueuedMessage::new("b".to_string(), None));
    assert!(app.submit_pending_steers_after_interrupt);
    assert_eq!(app.pending_steers.len(), 2);
}

#[test]
fn pop_last_queued_into_draft_pops_back_and_arms_draft() {
    let mut app = App::new(test_options(false), &Config::default());
    app.queue_message(QueuedMessage::new(
        "first".to_string(),
        Some("skill-A".to_string()),
    ));
    app.queue_message(QueuedMessage::new(
        "last".to_string(),
        Some("skill-B".to_string()),
    ));

    assert!(app.pop_last_queued_into_draft());
    assert_eq!(app.input, "last");
    assert_eq!(app.cursor_position, "last".chars().count());
    assert_eq!(app.queued_messages.len(), 1);
    let draft = app.queued_draft.clone().expect("draft is set");
    assert_eq!(draft.display, "last");
    assert_eq!(draft.skill_instruction.as_deref(), Some("skill-B"));
}

#[test]
fn pop_last_queued_into_draft_noop_when_composer_dirty() {
    let mut app = App::new(test_options(false), &Config::default());
    app.queue_message(QueuedMessage::new("queued".to_string(), None));
    app.input = "typing".to_string();
    app.cursor_position = char_count(&app.input);

    assert!(!app.pop_last_queued_into_draft());
    assert_eq!(app.input, "typing");
    assert_eq!(app.queued_messages.len(), 1);
    assert!(app.queued_draft.is_none());
}

#[test]
fn pop_last_queued_into_draft_noop_when_draft_already_armed() {
    let mut app = App::new(test_options(false), &Config::default());
    app.queue_message(QueuedMessage::new("queued".to_string(), None));
    app.queued_draft = Some(QueuedMessage::new("editing".to_string(), None));

    assert!(!app.pop_last_queued_into_draft());
    assert_eq!(app.queued_messages.len(), 1);
    assert_eq!(
        app.queued_draft.as_ref().map(|d| d.display.as_str()),
        Some("editing")
    );
}

#[test]
fn pop_last_queued_into_draft_noop_when_queue_empty() {
    let mut app = App::new(test_options(false), &Config::default());
    assert!(!app.pop_last_queued_into_draft());
    assert!(app.input.is_empty());
    assert!(app.queued_draft.is_none());
}

#[test]
fn cancel_queued_draft_edit_restores_original_message() {
    let mut app = App::new(test_options(false), &Config::default());
    app.queue_message(QueuedMessage::new("first".to_string(), None));
    app.queue_message(QueuedMessage::new(
        "original follow-up".to_string(),
        Some("skill".to_string()),
    ));
    assert!(app.pop_last_queued_into_draft());
    app.input = "edited but not submitted".to_string();
    app.cursor_position = char_count(&app.input);

    assert!(app.cancel_queued_draft_edit());

    assert!(app.input.is_empty());
    assert!(app.queued_draft.is_none());
    assert_eq!(app.queued_messages.len(), 2);
    let restored = app.queued_messages.back().expect("restored message");
    assert_eq!(restored.display, "original follow-up");
    assert_eq!(restored.skill_instruction.as_deref(), Some("skill"));
    assert_eq!(
        app.clear_undo_buffer.as_deref(),
        Some("edited but not submitted"),
        "the interrupted edit remains recoverable via normal draft recovery"
    );
}

#[test]
fn finalize_streaming_assistant_marks_existing_cell_interrupted() {
    let mut app = App::new(test_options(false), &Config::default());
    app.add_message(HistoryCell::Assistant {
        content: "partial reply so far".to_string(),
        streaming: true,
    });
    let idx = app.history.len() - 1;
    app.streaming_message_index = Some(idx);

    app.finalize_streaming_assistant_as_interrupted();

    assert!(app.streaming_message_index.is_none());
    match &app.history[idx] {
        HistoryCell::Assistant { content, streaming } => {
            assert!(content.starts_with("[interrupted]"), "got: {content}");
            assert!(content.contains("partial reply so far"));
            assert!(!*streaming);
        }
        other => panic!("expected Assistant cell, got {other:?}"),
    }
}

#[test]
fn finalize_streaming_assistant_handles_empty_content() {
    let mut app = App::new(test_options(false), &Config::default());
    app.add_message(HistoryCell::Assistant {
        content: String::new(),
        streaming: true,
    });
    let idx = app.history.len() - 1;
    app.streaming_message_index = Some(idx);

    app.finalize_streaming_assistant_as_interrupted();

    match &app.history[idx] {
        HistoryCell::Assistant { content, streaming } => {
            assert_eq!(content, "[interrupted]");
            assert!(!*streaming);
        }
        other => panic!("expected Assistant cell, got {other:?}"),
    }
}

#[test]
fn finalize_streaming_assistant_no_op_without_index() {
    let mut app = App::new(test_options(false), &Config::default());
    // No streaming index set; should not panic and should leave history unchanged.
    let prev_len = app.history.len();
    app.finalize_streaming_assistant_as_interrupted();
    assert_eq!(app.history.len(), prev_len);
    assert!(app.streaming_message_index.is_none());
}

#[test]
fn finalize_streaming_assistant_is_idempotent_on_double_call() {
    let mut app = App::new(test_options(false), &Config::default());
    app.add_message(HistoryCell::Assistant {
        content: "something".to_string(),
        streaming: true,
    });
    let idx = app.history.len() - 1;
    app.streaming_message_index = Some(idx);

    app.finalize_streaming_assistant_as_interrupted();
    // Second call without resetting state must be safe.
    app.finalize_streaming_assistant_as_interrupted();

    match &app.history[idx] {
        HistoryCell::Assistant { content, .. } => {
            // Second call still finds index None — content unchanged from first.
            assert!(content.starts_with("[interrupted] "));
            assert_eq!(content.matches("[interrupted]").count(), 1);
        }
        other => panic!("expected Assistant cell, got {other:?}"),
    }
}

#[test]
fn delete_word_backward_removes_previous_word_only() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "hello world".to_string();
    app.cursor_position = char_count(&app.input);

    app.delete_word_backward();

    assert_eq!(app.input, "hello ");
    assert_eq!(app.cursor_position, char_count("hello "));
}

#[test]
fn delete_word_backward_handles_trailing_space_and_utf8() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "cafe 你好   ".to_string();
    app.cursor_position = char_count(&app.input);

    app.delete_word_backward();

    assert_eq!(app.input, "cafe ");
    assert_eq!(app.cursor_position, char_count("cafe "));
}

#[test]
fn delete_word_forward_handles_leading_space_and_utf8() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "hello 你好 world".to_string();
    app.cursor_position = char_count("hello");

    app.delete_word_forward();

    assert_eq!(app.input, "hello world");
    assert_eq!(app.cursor_position, char_count("hello"));
}

#[test]
fn delete_to_start_of_line_respects_multiline_cursor() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "first\nsecond line".to_string();
    app.cursor_position = char_count("first\nsecond");

    app.delete_to_start_of_line();

    assert_eq!(app.input, "first\n line");
    assert_eq!(app.cursor_position, char_count("first\n"));
}

#[test]
fn kill_and_yank_handle_multibyte_utf8() {
    let mut app = App::new(test_options(false), &Config::default());
    // "café 你好" — char_count = 7 (c,a,f,é, ,你,好); UTF-8 bytes differ.
    app.input = "café 你好".to_string();
    app.cursor_position = 5; // before '你'
    assert!(app.kill_to_end_of_line());
    assert_eq!(app.input, "café ");
    assert_eq!(app.cursor_position, 5);
    assert_eq!(app.kill_buffer, "你好");

    // Yank back at the same spot — must not panic on char boundaries.
    assert!(app.yank());
    assert_eq!(app.input, "café 你好");
    assert_eq!(app.cursor_position, 7);
}

#[test]
fn selection_range_returns_none_when_no_anchor() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "hello world".to_string();
    app.cursor_position = 5;
    app.selection_anchor = None;
    assert!(app.selection_range().is_none());
}

#[test]
fn selection_range_returns_ordered_range() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "hello world".to_string();
    app.cursor_position = 5;
    app.selection_anchor = Some(2);
    assert_eq!(app.selection_range(), Some((2, 5)));
}

#[test]
fn selection_range_normalizes_order() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "hello world".to_string();
    app.cursor_position = 2;
    app.selection_anchor = Some(5);
    assert_eq!(app.selection_range(), Some((2, 5)));
}

#[test]
fn selection_range_returns_none_when_anchor_equals_cursor() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "hello".to_string();
    app.cursor_position = 3;
    app.selection_anchor = Some(3);
    assert!(app.selection_range().is_none());
}

#[test]
fn delete_selection_removes_selected_text() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "hello world".to_string();
    app.cursor_position = 5;
    app.selection_anchor = Some(2);
    assert!(app.delete_selection());
    assert_eq!(app.input, "he world");
    assert_eq!(app.cursor_position, 2);
    assert!(app.selection_anchor.is_none());
}

#[test]
fn insert_char_replaces_selection() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "hello world".to_string();
    app.cursor_position = 5;
    app.selection_anchor = Some(2);
    app.insert_char('X');
    assert_eq!(app.input, "heX world");
    assert_eq!(app.cursor_position, 3);
    assert!(app.selection_anchor.is_none());
}

#[test]
fn delete_char_removes_selection_instead_of_single_char() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "hello world".to_string();
    app.cursor_position = 5;
    app.selection_anchor = Some(2);
    app.delete_char();
    assert_eq!(app.input, "he world");
    assert_eq!(app.cursor_position, 2);
}

#[test]
fn selected_text_returns_correct_substring() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "hello world".to_string();
    app.cursor_position = 5;
    app.selection_anchor = Some(2);
    assert_eq!(app.selected_text(), "llo");
}

#[test]
fn insert_str_replaces_selection() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "hello world".to_string();
    app.cursor_position = 5;
    app.selection_anchor = Some(2);
    app.insert_str("yo");
    assert_eq!(app.input, "heyo world");
    assert_eq!(app.cursor_position, 4);
    assert!(app.selection_anchor.is_none());
}

#[test]
fn delete_selection_noop_when_no_selection() {
    let mut app = App::new(test_options(false), &Config::default());
    app.input = "hello".to_string();
    app.cursor_position = 3;
    app.selection_anchor = None;
    assert!(!app.delete_selection());
    assert_eq!(app.input, "hello");
    assert_eq!(app.cursor_position, 3);
}

// === #2574: capability-aware fallback eligibility ===============================

/// Build an `App` whose fallback chain is `[active, fallbacks...]` with each
/// provider's auth controlled via `config.providers` keys. Env-var keys for the
/// providers under test are cleared so readiness is driven solely by config.
fn app_with_fallback_chain(
    active: ApiProvider,
    fallbacks: &[codewhale_config::ProviderKind],
    keyed: &[ApiProvider],
) -> App {
    let mut providers = ProvidersConfig::default();
    for provider in keyed {
        let entry = ProviderConfig {
            api_key: Some(format!("test-key-{}", provider.as_str())),
            ..Default::default()
        };
        match provider {
            ApiProvider::Deepseek => providers.deepseek = entry,
            ApiProvider::Openai => providers.openai = entry,
            ApiProvider::Openrouter => providers.openrouter = entry,
            ApiProvider::Together => providers.together = entry,
            ApiProvider::Fireworks => providers.fireworks = entry,
            other => panic!("unhandled keyed provider in test helper: {other:?}"),
        }
    }

    let config = Config {
        provider: Some(active.as_str().to_string()),
        fallback_providers: fallbacks.to_vec(),
        providers: Some(providers),
        ..Default::default()
    };

    let mut options = test_options(false);
    options.start_in_agent_mode = true;
    options.skip_onboarding = true;
    App::new(options, &config)
}

#[test]
fn advance_fallback_skips_unauthed_middle_provider_and_lands_on_next_ready() {
    let _lock = lock_test_env();
    let _openai = EnvVarGuard::remove("OPENAI_API_KEY");
    let _openrouter = EnvVarGuard::remove("OPENROUTER_API_KEY");
    let _together = EnvVarGuard::remove("TOGETHER_API_KEY");

    // Chain: Openai (active, keyed) -> Openrouter (no key) -> Together (keyed).
    let mut app = app_with_fallback_chain(
        ApiProvider::Openai,
        &[
            codewhale_config::ProviderKind::Openrouter,
            codewhale_config::ProviderKind::Together,
        ],
        &[ApiProvider::Openai, ApiProvider::Together],
    );
    assert_eq!(app.fallback_chain_position(), Some(0));

    // Openrouter is skipped (needs auth); we land on Together.
    let next = app.advance_fallback("network error");
    assert_eq!(next, Some(ApiProvider::Together));
    assert_eq!(app.api_provider, ApiProvider::Together);
    assert_eq!(app.fallback_chain_position(), Some(2));

    let reason = app.last_fallback_reason.as_deref().unwrap_or_default();
    assert!(
        reason.contains("Fell back to together"),
        "reason should name the landed provider: {reason}"
    );
    assert!(
        reason.contains("skipped openrouter: needs auth"),
        "reason should note the skipped provider: {reason}"
    );
}

#[test]
fn advance_fallback_local_provider_is_eligible_without_a_key() {
    let _lock = lock_test_env();
    let _openai = EnvVarGuard::remove("OPENAI_API_KEY");

    // Chain: Openai (active, keyed) -> Ollama (local, no key needed).
    let mut app = app_with_fallback_chain(
        ApiProvider::Openai,
        &[codewhale_config::ProviderKind::Ollama],
        &[ApiProvider::Openai],
    );

    let next = app.advance_fallback("timeout");
    assert_eq!(
        next,
        Some(ApiProvider::Ollama),
        "self-hosted providers are ready without a key"
    );
    assert_eq!(app.api_provider, ApiProvider::Ollama);
    let reason = app.last_fallback_reason.as_deref().unwrap_or_default();
    assert!(reason.contains("Fell back to ollama"), "{reason}");
    assert!(
        !reason.contains("skipped"),
        "no providers should be skipped: {reason}"
    );
}

#[test]
fn advance_fallback_all_unready_exhausts_with_clear_reason() {
    let _lock = lock_test_env();
    let _openai = EnvVarGuard::remove("OPENAI_API_KEY");
    let _openrouter = EnvVarGuard::remove("OPENROUTER_API_KEY");
    let _together = EnvVarGuard::remove("TOGETHER_API_KEY");

    // Chain: Openai (active, keyed) -> Openrouter (no key) -> Together (no key).
    // Every fallback entry is unready, so the chain exhausts.
    let mut app = app_with_fallback_chain(
        ApiProvider::Openai,
        &[
            codewhale_config::ProviderKind::Openrouter,
            codewhale_config::ProviderKind::Together,
        ],
        &[ApiProvider::Openai],
    );

    let next = app.advance_fallback("rate limited");
    assert_eq!(next, None, "no ready fallback remains");
    // Active provider is unchanged on exhaustion.
    assert_eq!(app.api_provider, ApiProvider::Openai);

    let reason = app.last_fallback_reason.as_deref().unwrap_or_default();
    assert!(
        reason.contains("Fallback chain exhausted"),
        "reason should state exhaustion: {reason}"
    );
    assert!(
        reason.contains("skipped openrouter: needs auth")
            && reason.contains("skipped together: needs auth"),
        "reason should note every skipped provider: {reason}"
    );
}

#[test]
fn advance_fallback_local_primary_does_not_fall_back_to_cloud() {
    let _lock = lock_test_env();
    let _openai = EnvVarGuard::remove("OPENAI_API_KEY");
    let _deepseek = EnvVarGuard::remove("DEEPSEEK_API_KEY");

    // Local primary (Ollama) -> cloud fallback (DeepSeek, fully keyed). The
    // cloud entry is policy-blocked even though it is otherwise ready, so the
    // chain exhausts rather than leaking a local/private route out to cloud.
    let mut app = app_with_fallback_chain(
        ApiProvider::Ollama,
        &[codewhale_config::ProviderKind::Deepseek],
        &[ApiProvider::Deepseek],
    );

    let next = app.advance_fallback("local runtime unavailable");
    assert_eq!(next, None, "local->cloud fallback must be blocked");
    assert_eq!(app.api_provider, ApiProvider::Ollama);

    let reason = app.last_fallback_reason.as_deref().unwrap_or_default();
    assert!(
        reason.contains("local/private policy"),
        "block reason must be visible and specific: {reason}"
    );
    assert!(
        !reason.contains("needs auth"),
        "the block is policy, not missing auth: {reason}"
    );
}

#[test]
fn advance_fallback_local_primary_may_fall_back_to_local_sibling() {
    let _lock = lock_test_env();

    // Local primary (Ollama) -> local sibling (vLLM). Both are self-hosted, so
    // the local/private posture is preserved and the fallback is allowed.
    let mut app = app_with_fallback_chain(
        ApiProvider::Ollama,
        &[codewhale_config::ProviderKind::Vllm],
        &[],
    );

    let next = app.advance_fallback("local runtime unavailable");
    assert_eq!(
        next,
        Some(ApiProvider::Vllm),
        "local->local fallback stays within the private posture"
    );
    assert_eq!(app.api_provider, ApiProvider::Vllm);
    let reason = app.last_fallback_reason.as_deref().unwrap_or_default();
    assert!(reason.contains("Fell back to vllm"), "{reason}");
}

#[test]
fn advance_fallback_cloud_primary_can_hop_cloud_to_local_to_cloud() {
    let _lock = lock_test_env();
    let _openai = EnvVarGuard::remove("OPENAI_API_KEY");
    let _deepseek = EnvVarGuard::remove("DEEPSEEK_API_KEY");

    // The local/private guard is origin-based. A cloud primary may route to a
    // local fallback and then to another cloud fallback if the cloud candidate
    // is otherwise ready; only local/private primaries are blocked from leaking
    // out to cloud.
    let mut app = app_with_fallback_chain(
        ApiProvider::Openai,
        &[
            codewhale_config::ProviderKind::Ollama,
            codewhale_config::ProviderKind::Deepseek,
        ],
        &[ApiProvider::Openai, ApiProvider::Deepseek],
    );

    let local = app.advance_fallback("cloud provider timed out");
    assert_eq!(local, Some(ApiProvider::Ollama));
    assert_eq!(app.api_provider, ApiProvider::Ollama);

    let cloud = app.advance_fallback("local runtime unavailable");
    assert_eq!(cloud, Some(ApiProvider::Deepseek));
    assert_eq!(app.api_provider, ApiProvider::Deepseek);

    let reason = app.last_fallback_reason.as_deref().unwrap_or_default();
    assert!(reason.contains("Fell back to deepseek"), "{reason}");
    assert!(
        !reason.contains("local/private policy"),
        "cloud-primary chains should not trigger local/private blocking: {reason}"
    );
}
