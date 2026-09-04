use super::*;
use crate::test_support::{EnvVarGuard, env_scope_ticket, join_env_scope, lock_test_env};
use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::mpsc;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct HeaderItemsTestConfig {
    #[serde(default, deserialize_with = "deser_header_items")]
    header_items: Option<Vec<HeaderItem>>,
}

#[test]
fn parses_header_tokens_item() {
    let config: HeaderItemsTestConfig = toml::from_str(
        r#"
header_items = ["tokens"]
"#,
    )
    .expect("header_items should parse");

    assert_eq!(config.header_items, Some(vec![HeaderItem::Tokens]));
}

#[test]
fn ignores_unknown_header_items() {
    let config: HeaderItemsTestConfig = toml::from_str(
        r#"
header_items = ["tokens", "future_item"]
"#,
    )
    .expect("unknown header items should not reject the config");

    assert_eq!(config.header_items, Some(vec![HeaderItem::Tokens]));
}

#[test]
fn header_items_scenario() {
    // Scenario consolidation of: header_items_round_trip, header_items_are_opt_in_by_default
    // from header_items_round_trip
    {
        let original = HeaderItemsTestConfig {
            header_items: Some(vec![HeaderItem::Tokens]),
        };

        let serialized = toml::to_string(&original).expect("config should serialize");
        let decoded: HeaderItemsTestConfig =
            toml::from_str(&serialized).expect("serialized config should parse");

        assert_eq!(decoded, original);
    }
    // from header_items_are_opt_in_by_default
    {
        assert!(HeaderItem::default_header().is_empty());
    }
}

#[test]
fn malformed_config_error_omits_secret_contents_and_keys() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let secret = "cw-secret-tui-config-4507";
    fs::write(
        &path,
        format!("[providers.xai]\napi_key = \"{secret}\" trailing-junk\n"),
    )
    .expect("write malformed config");

    let error = Config::load(Some(path), None).expect_err("malformed config must fail");
    let diagnostic = format!("{error:#}");
    assert!(!diagnostic.contains(secret), "{diagnostic}");
    assert!(!diagnostic.contains("api_key"), "{diagnostic}");
    assert!(
        diagnostic.contains("file contents were omitted"),
        "{diagnostic}"
    );
}

#[test]
fn api_provider_metadata_helpers_follow_config_provider_metadata() {
    let sorted = ApiProvider::sorted_for_display();
    let expected_sorted: Vec<ApiProvider> =
        codewhale_config::provider::providers_sorted_for_display()
            .iter()
            .map(|provider| ApiProvider::from_kind(provider.kind()))
            .collect();
    assert_eq!(sorted, expected_sorted);

    for kind in codewhale_config::ProviderKind::ALL {
        let provider = ApiProvider::from_kind(kind);
        let metadata = provider.metadata().expect("metadata-backed provider");
        assert_eq!(metadata.kind(), kind);
        assert_eq!(provider.env_vars(), kind.provider().env_vars());
        assert_eq!(
            provider.default_base_url(),
            kind.provider().default_base_url()
        );
    }

    assert_eq!(ApiProvider::DeepseekCN.metadata().map(|p| p.kind()), None);
    assert_eq!(
        ApiProvider::DeepseekCN.env_vars(),
        codewhale_config::ProviderKind::Deepseek
            .provider()
            .env_vars()
    );
    assert_eq!(
        ApiProvider::DeepseekCN.default_base_url(),
        DEFAULT_DEEPSEEKCN_BASE_URL
    );
}

#[test]
fn every_api_provider_variant_resolves_base_url_without_panicking() {
    // Guard against the historical `.expect("ApiProvider variant missing
    // ProviderKind metadata")` in `default_base_url()`: a provider variant
    // added without KIND_LOOKUP metadata used to hard-panic at startup or
    // render. Every variant must resolve a non-empty base URL through the
    // DeepSeek fallback when it has no registered metadata.
    let mut constructed = 0usize;
    for provider in ApiProvider::all() {
        let url = provider.default_base_url();
        assert!(!url.is_empty(), "{provider:?} default_base_url is empty");
        constructed += 1;
    }
    // DeepseekCN is intentionally absent from `all()` (TUI-only legacy alias
    // with its own config table) — cover it explicitly.
    let url = ApiProvider::DeepseekCN.default_base_url();
    assert!(!url.is_empty(), "DeepseekCN default_base_url is empty");
    constructed += 1;

    // Every variant of the enum must have been constructed above. If this
    // assertion fails, a new variant was added without extending the lookup
    // tables — extend `all()`/KIND_LOOKUP and re-run.
    assert_eq!(
        constructed,
        ApiProvider::all().len() + 1,
        "unconstructed ApiProvider variant"
    );
}

#[test]
fn provider_config_key_follows_config_provider_metadata() {
    for kind in codewhale_config::ProviderKind::ALL
        .into_iter()
        .filter(|kind| *kind != codewhale_config::ProviderKind::Deepseek)
    {
        let provider = ApiProvider::from_kind(kind);
        assert_eq!(
            provider_config_key(provider).expect("metadata-backed config key"),
            kind.provider().provider_config_key()
        );
    }

    assert!(provider_config_key(ApiProvider::Deepseek).is_err());
    assert!(provider_config_key(ApiProvider::DeepseekCN).is_err());
}

#[test]
fn deepseek_api_key_reads_metadata_env_vars_for_newer_providers() -> Result<()> {
    let _lock = lock_test_env();
    let _source = EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
    let cases = [
        (ApiProvider::Zai, "ZAI_API_KEY", "zai-env-key"),
        (ApiProvider::Stepfun, "STEPFUN_API_KEY", "stepfun-env-key"),
        (ApiProvider::Minimax, "MINIMAX_API_KEY", "minimax-env-key"),
        (
            ApiProvider::MinimaxAnthropic,
            "MINIMAX_API_KEY",
            "minimax-env-key",
        ),
        (
            ApiProvider::Deepinfra,
            "DEEPINFRA_API_KEY",
            "deepinfra-env-key",
        ),
        (ApiProvider::Sakana, "FUGU_API_KEY", "fugu-env-key"),
        (
            ApiProvider::Together,
            "TOGETHER_API_KEY",
            "together-env-key",
        ),
        (ApiProvider::Qianfan, "QIANFAN_API_KEY", "qianfan-env-key"),
        (
            ApiProvider::OpencodeGo,
            "OPENCODE_GO_API_KEY",
            "opencode-go-env-key",
        ),
    ];
    let _env_guards: Vec<_> = cases
        .iter()
        .map(|(_, var, value)| EnvVarGuard::set(var, value))
        .collect();

    for (provider, _, expected_key) in cases {
        let config = Config {
            provider: Some(provider.as_str().to_string()),
            ..Config::default()
        };

        assert_eq!(config.deepseek_api_key()?, expected_key);
    }

    Ok(())
}

#[test]
fn goal_max_continuations_loads_from_goal_table() -> Result<()> {
    // Absent table → unlimited by default (#5052).
    let config: Config = toml::from_str("")?;
    assert_eq!(
        config.goal_max_continuations(),
        crate::goal_loop::DEFAULT_MAX_GOAL_CONTINUATIONS
    );
    assert_eq!(config.goal_max_continuations(), 0);
    assert_eq!(config.goal_continuation_delay_seconds(), 0);

    // Explicit backstop override.
    let config: Config = toml::from_str(
        r#"
[goal]
max_continuations = 25
continuation_delay_seconds = 300
"#,
    )?;
    assert_eq!(config.goal_max_continuations(), 25);
    assert_eq!(config.goal_continuation_delay_seconds(), 300);

    // 0 = unlimited; token/time budgets are telemetry only.
    let config: Config = toml::from_str(
        r#"
[goal]
max_continuations = 0
"#,
    )?;
    assert_eq!(config.goal_max_continuations(), 0);
    assert_eq!(config.goal_continuation_delay_seconds(), 0);

    // Bound accidental giant cadences; this remains a turn loop, not a
    // replacement for durable low-frequency automations.
    let config: Config = toml::from_str(
        r#"
[goal]
continuation_delay_seconds = 999999999
"#,
    )?;
    assert_eq!(
        config.goal_continuation_delay_seconds(),
        crate::goal_loop::MAX_GOAL_CONTINUATION_DELAY_SECONDS
    );

    Ok(())
}

#[test]
fn reasoning_only_config_loads_from_table() -> Result<()> {
    // Absent table → built-in defaults.
    let config: Config = toml::from_str("")?;
    assert_eq!(
        config.reasoning_only_max_reprompts(),
        crate::config::DEFAULT_REASONING_ONLY_REPROMPTS
    );
    assert_eq!(
        config.reasoning_only_reprompt_message(),
        crate::config::DEFAULT_REASONING_ONLY_REPROMPT_MESSAGE
    );

    // Explicit max_reprompts override.
    let config: Config = toml::from_str(
        r#"
[reasoning_only]
max_reprompts = 10
"#,
    )?;
    assert_eq!(config.reasoning_only_max_reprompts(), 10);
    assert_eq!(
        config.reasoning_only_reprompt_message(),
        crate::config::DEFAULT_REASONING_ONLY_REPROMPT_MESSAGE
    );

    // Explicit reprompt_message override.
    let config: Config = toml::from_str(
        r#"
[reasoning_only]
max_reprompts = 5
reprompt_message = "tu n'as rien a dire"
"#,
    )?;
    assert_eq!(config.reasoning_only_max_reprompts(), 5);
    assert_eq!(
        config.reasoning_only_reprompt_message(),
        "tu n'as rien a dire"
    );

    // 0 = disable automatic recovery.
    let config: Config = toml::from_str(
        r#"
[reasoning_only]
max_reprompts = 0
"#,
    )?;
    assert_eq!(config.reasoning_only_max_reprompts(), 0);
    assert_eq!(
        config.reasoning_only_reprompt_message(),
        crate::config::DEFAULT_REASONING_ONLY_REPROMPT_MESSAGE
    );

    Ok(())
}

#[test]
fn modelstudio_coding_plan_mode_resolves_the_official_chat_base_url() {
    // The picker represents Coding Plan as the primary Model Studio provider
    // plus a mode, rather than switching to the legacy Coding Plan identity.
    // Keep this config-resolution seam covered: chat-route reasoning support
    // relies on receiving this exact official URL downstream.
    let config: Config = toml::from_str(
        r#"
provider = "modelstudio-token-plan"

[providers.modelstudio_token_plan]
mode = "coding-plan"
"#,
    )
    .expect("Coding Plan mode should parse");

    assert_eq!(config.api_provider(), ApiProvider::ModelstudioTokenPlan);
    assert_eq!(
        config.deepseek_base_url(),
        DEFAULT_MODELSTUDIO_CODING_PLAN_BASE_URL
    );
}

#[test]
fn provider_context_scenario() -> Result<()> {
    // Scenario consolidation of: provider_context_window_loads_from_provider_table, provider_context_window_zero_is_invalid
    // from provider_context_window_loads_from_provider_table
    {
        let config: Config = toml::from_str(
            r#"
    provider = "openai"

    [providers.openai]
    model = "qwen3.7"
    context_window = 1000000
    "#,
        )?;

        config.validate()?;
        assert_eq!(
            config.context_window_for_provider_config(ApiProvider::Openai),
            Some(1_000_000)
        );
    }
    // from provider_context_window_zero_is_invalid
    {
        let config: Config = toml::from_str(
            r#"
    [providers.openai]
    context_window = 0
    "#,
        )
        .expect("zero is syntactically valid TOML");

        let err = config
            .validate()
            .expect_err("zero context_window should be rejected");
        assert!(err.to_string().contains("providers.openai.context_window"));
    }
    Ok(())
}

#[test]
fn opencode_go_context_window_zero_is_invalid() {
    let config: Config = toml::from_str(
        r#"
[providers.opencode_go]
context_window = 0
"#,
    )
    .expect("zero is syntactically valid TOML");

    let err = config
        .validate()
        .expect_err("zero OpenCode Go context_window should be rejected");
    assert!(
        err.to_string()
            .contains("providers.opencode_go.context_window")
    );
}

#[test]
fn missing_provider_api_key_message_uses_provider_metadata() -> Result<()> {
    let message = missing_provider_api_key_message(ApiProvider::Zai)?;

    assert!(message.contains("Zhipu AI / Z.ai API key not found"));
    assert!(message.contains("https://z.ai/model-api"));
    assert!(message.contains("ZAI_API_KEY / Z_AI_API_KEY"));
    assert!(message.contains("[providers.zai] api_key"));

    Ok(())
}

#[test]
fn opencode_zen_missing_credentials_never_mentions_codex_oauth() -> Result<()> {
    let message = missing_provider_api_key_message(ApiProvider::OpencodeZen)?;
    assert!(message.contains("OpenCode Zen API key not found"));
    assert!(message.contains("OPENCODE_ZEN_API_KEY"));
    assert!(message.contains("OPENCODE_API_KEY"));
    assert!(message.contains("[providers.opencode_zen]"));
    assert!(!message.contains("codex login"));
    assert!(!message.contains("ChatGPT"));
    assert!(!message.contains("auth.json"));
    Ok(())
}

// GHSA-72w5-pf8h-xfp4 — regression: `allow_shell` must be opt-in.
#[test]
fn allow_shell_defaults_to_false_when_unset() {
    let config = Config::default();
    assert_eq!(config.allow_shell, None, "default Config has no opt-in set");
    assert!(
        !config.allow_shell(),
        "Config::allow_shell() must default to false when no opt-in is recorded"
    );
}

// The interactive default is shell-on (approval-gated). Both interactive
// startup and the durable Agent permission baseline (app.rs) read this single
// method so the default cannot drift between launch modes; an explicit opt-out
// is still honored.
#[test]
fn interactive_allow_shell_defaults_to_true_but_honors_explicit_opt_out() {
    let default_config = Config::default();
    assert!(
        default_config.interactive_allow_shell(),
        "interactive Agent sessions expose shell by default so approvals can gate commands"
    );

    let opted_out = Config {
        allow_shell: Some(false),
        ..Config::default()
    };
    assert!(
        !opted_out.interactive_allow_shell(),
        "explicit allow_shell = false still hides shell in interactive sessions"
    );

    let opted_in = Config {
        allow_shell: Some(true),
        ..Config::default()
    };
    assert!(opted_in.interactive_allow_shell());
}

#[test]
fn prompt_suggestion_scenario() {
    // Scenario consolidation of: prompt_suggestion_defaults_to_false, prompt_suggestion_enabled_when_set_true
    // from prompt_suggestion_defaults_to_false
    {
        let config = Config::default();
        assert_eq!(
            config.prompt_suggestion, None,
            "default Config must not opt in"
        );
        assert!(
            !config.prompt_suggestion_enabled(),
            "prompt_suggestion must be opt-in (default off)"
        );
    }
    // from prompt_suggestion_enabled_when_set_true
    {
        let config = Config {
            prompt_suggestion: Some(true),
            ..Default::default()
        };
        assert!(config.prompt_suggestion_enabled());
    }
}

#[test]
fn auto_review_config_builds_runtime_policy() -> Result<()> {
    let config: Config = toml::from_str(
        r#"
[auto_review]
natural_language_guidance = "retired compatibility key"

[[auto_review.block]]
id = "block-shell"
action_kind = "shell"
reason = "shell requires maintainer review"

[[auto_review.allow]]
id = "allow-read-file"
tool = "read_file"
reason = "read_file is allowed"
"#,
    )?;
    config.validate()?;

    let policy = config.auto_review_policy();
    let shell_context = crate::tui::auto_review::AutoReviewContext::from_tool_call(
        "exec_shell",
        &serde_json::json!({"command": "cargo test"}),
        crate::tui::auto_review::RunOrigin::Interactive,
        crate::tui::approval::ApprovalMode::Auto,
        true,
        None,
    );
    let shell_decision = policy.evaluate(&shell_context);
    assert_eq!(
        shell_decision.action,
        crate::tui::auto_review::AutoReviewAction::Block
    );
    assert_eq!(shell_decision.rule_id.as_deref(), Some("block-shell"));

    let read_context = crate::tui::auto_review::AutoReviewContext::from_tool_call(
        "read_file",
        &serde_json::json!({"path": "README.md"}),
        crate::tui::auto_review::RunOrigin::Interactive,
        crate::tui::approval::ApprovalMode::Auto,
        true,
        None,
    );
    let read_decision = policy.evaluate(&read_context);
    assert_eq!(
        read_decision.action,
        crate::tui::auto_review::AutoReviewAction::Allow
    );
    assert_eq!(read_decision.rule_id.as_deref(), Some("allow-read-file"));

    Ok(())
}

#[test]
fn auto_review_scenario() -> Result<()> {
    // Scenario consolidation of: auto_review_profile_overrides_base_policy, auto_review_text_contains_fails_closed_instead_of_broadening_a_rule, auto_review_legacy_allow_kind_fails_closed_instead_of_widening, auto_review_config_rejects_invalid_rule_shapes
    // from auto_review_profile_overrides_base_policy
    {
        let parsed: ConfigFile = toml::from_str(
            r#"
    [[auto_review.block]]
    action_kind = "shell"

    [[profiles.strict.auto_review.block]]
    action_kind = "network"
    "#,
        )?;

        let merged = apply_profile(parsed, Some("strict"))?;
        let policy = merged.auto_review_policy();

        assert_eq!(policy.block_rules.len(), 1);
        assert_eq!(
            policy.block_rules[0].action_kind,
            Some(crate::tui::auto_review::ToolActionKind::External)
        );
    }
    // from auto_review_text_contains_fails_closed_instead_of_broadening_a_rule
    {
        let error = toml::from_str::<Config>(
            r#"
    [[auto_review.allow]]
    tool = "exec_shell"
    text_contains = "run tests"
    "#,
        )
        .expect("shape parses")
        .validate()
        .expect_err("retired user-intent matcher must not disappear");

        assert!(
            error
                .to_string()
                .contains("user-intent matching was retired")
        );
    }
    // from auto_review_legacy_allow_kind_fails_closed_instead_of_widening
    {
        let error = toml::from_str::<Config>(
            r#"
    [[auto_review.allow]]
    action_kind = "git"
    "#,
        )
        .expect("shape parses")
        .validate()
        .expect_err("narrow legacy allow kind must not widen to external");

        assert!(error.to_string().contains("cannot safely widen"));
    }
    // from auto_review_config_rejects_invalid_rule_shapes
    {
        let invalid_kind: Config = toml::from_str(
            r#"
    [[auto_review.block]]
    action_kind = "teleport"
    "#,
        )
        .expect("parse config");
        let err = invalid_kind.validate().expect_err("invalid kind");
        assert!(
            err.to_string()
                .contains("Invalid auto_review.block[0].action_kind")
        );

        let global_allow: Config = toml::from_str(
            r#"
    [[auto_review.allow]]
    reason = "too broad"
    "#,
        )
        .expect("parse config");
        let err = global_allow.validate().expect_err("missing matcher");
        assert!(err.to_string().contains("set at least one of tool"));
    }
    Ok(())
}

#[test]
fn config_loads_sibling_permissions_into_exec_policy_engine() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    fs::write(&config_path, "model = \"deepseek-v4-pro\"\n").expect("write config");
    fs::write(
        dir.path().join(codewhale_config::PERMISSIONS_FILE_NAME),
        r#"
[[rules]]
tool = "exec_shell"
command = "cargo test"
"#,
    )
    .expect("write permissions");

    let config = Config::load(Some(config_path), None).expect("load config");
    let decision = config
        .exec_policy_engine
        .check(codewhale_execpolicy::ExecPolicyContext {
            command: "cargo test --workspace",
            cwd: dir.path().to_string_lossy().as_ref(),
            tool: Some("exec_shell"),
            path: None,
            ask_for_approval: codewhale_execpolicy::AskForApproval::OnFailure,
            sandbox_mode: None,
        })
        .expect("check permission");

    assert!(decision.allow);
    assert!(decision.requires_approval);
    assert_eq!(
        decision.matched_rule.as_deref(),
        Some("tool=exec_shell command=cargo test")
    );
}

#[test]
fn config_loads_sibling_permissions_when_config_file_is_absent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    fs::write(
        dir.path().join(codewhale_config::PERMISSIONS_FILE_NAME),
        r#"
[[rules]]
tool = "exec_shell"
command = "npm test"
"#,
    )
    .expect("write permissions");

    let config = Config::load(Some(config_path), None).expect("load config");
    let decision = config
        .exec_policy_engine
        .check(codewhale_execpolicy::ExecPolicyContext {
            command: "npm test -- --runInBand",
            cwd: dir.path().to_string_lossy().as_ref(),
            tool: Some("exec_shell"),
            path: None,
            ask_for_approval: codewhale_execpolicy::AskForApproval::OnFailure,
            sandbox_mode: None,
        })
        .expect("check permission");

    assert!(decision.requires_approval);
    assert_eq!(
        decision.matched_rule.as_deref(),
        Some("tool=exec_shell command=npm test")
    );
}

#[test]
fn warns_when_allow_shell_nested_under_general_section() {
    // #2589: the reporter's config nested top-level keys under sections that
    // do not exist, so they were silently dropped and shell tools vanished.
    let raw = "[general]\nallow_shell = true\n\n[sandbox]\nsandbox_mode = \"danger-full-access\"\n";
    let warning =
        warn_on_misplaced_top_level_keys(raw).expect("misplaced keys should produce a warning");
    assert!(warning.contains("general.allow_shell"));
    assert!(warning.contains("sandbox.sandbox_mode"));
    assert!(warning.contains("#2589"));

    // Correctly placed top-level keys produce no warning.
    let ok = "allow_shell = true\nsandbox_mode = \"danger-full-access\"\n";
    assert!(warn_on_misplaced_top_level_keys(ok).is_none());

    // A parsed config from the correct placement actually enables shell.
    let parsed: ConfigFile = toml::from_str(ok).expect("parse top-level config");
    assert!(parsed.base.allow_shell());
}

#[test]
fn sandbox_network_access_parses_and_defaults_to_restricted() {
    // Absent key: restricted. Editing the workspace does not imply egress.
    let parsed: ConfigFile =
        toml::from_str("sandbox_mode = \"workspace-write\"\n").expect("parse without the key");
    assert_eq!(parsed.base.sandbox_network_access, None);

    let parsed: ConfigFile =
        toml::from_str("sandbox_network_access = true\n").expect("parse snake_case");
    assert_eq!(parsed.base.sandbox_network_access, Some(true));

    let parsed: ConfigFile =
        toml::from_str("sandboxNetworkAccess = true\n").expect("parse camelCase alias");
    assert_eq!(parsed.base.sandbox_network_access, Some(true));

    let parsed: ConfigFile =
        toml::from_str("sandbox_network_access = false\n").expect("parse explicit false");
    assert_eq!(parsed.base.sandbox_network_access, Some(false));
}

#[test]
fn load_honors_codewhale_home_for_primary_config_path() -> Result<()> {
    let _lock = lock_test_env();
    let dir = tempfile::tempdir()?;
    let codewhale_home = dir.path().join("isolated-codewhale");
    fs::create_dir_all(&codewhale_home)?;
    fs::write(codewhale_home.join("config.toml"), "provider = \"zai\"\n")?;
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());
    let _codewhale_config = EnvVarGuard::remove("CODEWHALE_CONFIG_PATH");
    let _deepseek_config = EnvVarGuard::remove("DEEPSEEK_CONFIG_PATH");

    let expected = codewhale_home.join("config.toml");
    assert_eq!(default_config_path()?, expected);
    let config = Config::load(None, None)?;

    assert_eq!(config.provider.as_deref(), Some("zai"));
    Ok(())
}

#[test]
fn load_accepts_dispatcher_written_camel_case_config_shape() -> Result<()> {
    let _lock = lock_test_env();
    let dir = tempfile::tempdir()?;
    let codewhale_home = dir.path().join("isolated-codewhale");
    fs::create_dir_all(&codewhale_home)?;
    fs::write(
        codewhale_home.join("config.toml"),
        r#"
provider = "zai"
fallbackProviders = []
apiKey = "deepseek-test-key"
defaultTextModel = "deepseek-v4-pro"
authMode = "api_key"

[providers.zai]
apiKey = "zai-test-key"
authMode = "api_key"

[providers.zai.httpHeaders]

[providers.xiaomiMimo]
baseUrl = "https://token-plan-sgp.xiaomimimo.com/v1"

[features.enabled]
shell_tool = true
subagents = true
web_search = true
"#,
    )?;
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());
    let _codewhale_config = EnvVarGuard::remove("CODEWHALE_CONFIG_PATH");
    let _deepseek_config = EnvVarGuard::remove("DEEPSEEK_CONFIG_PATH");

    let config = Config::load(None, None)?;

    assert_eq!(config.provider.as_deref(), Some("zai"));
    assert_eq!(config.api_key.as_deref(), Some("deepseek-test-key"));
    assert_eq!(
        config.default_text_model.as_deref(),
        Some("deepseek-v4-pro")
    );
    assert_eq!(config.auth_mode.as_deref(), Some("api_key"));
    let providers = config.providers.as_ref().expect("provider table");
    assert_eq!(providers.zai.api_key.as_deref(), Some("zai-test-key"));
    assert_eq!(providers.zai.auth_mode.as_deref(), Some("api_key"));
    assert_eq!(
        providers.xiaomi_mimo.base_url.as_deref(),
        Some("https://token-plan-sgp.xiaomimimo.com/v1")
    );
    let features = config.features();
    assert!(features.enabled(crate::features::Feature::ShellTool));
    assert!(features.enabled(crate::features::Feature::Subagents));
    assert!(features.enabled(crate::features::Feature::WebSearch));
    Ok(())
}

#[test]
fn tui_config_parses_lifecycle_outbox_table() {
    let raw = r#"
[lifecycle_outbox]
path = "~/.codewhale/notifications/outbox.jsonl"
webhook_url = "https://example.com/hooks/codewhale"
webhook_token = "secret-token"
"#;
    let parsed: ConfigFile = toml::from_str(raw).expect("parse lifecycle_outbox config");

    let outbox = parsed
        .base
        .lifecycle_outbox
        .expect("lifecycle_outbox table should parse");
    assert_eq!(
        outbox.path,
        Some(PathBuf::from("~/.codewhale/notifications/outbox.jsonl"))
    );
    assert_eq!(
        outbox.webhook_url.as_deref(),
        Some("https://example.com/hooks/codewhale")
    );
    assert_eq!(outbox.webhook_token.as_deref(), Some("secret-token"));

    // Off by default: a config without the table leaves the feature off.
    let absent: ConfigFile =
        toml::from_str("model = \"demo\"").expect("parse config without outbox table");
    assert!(absent.base.lifecycle_outbox.is_none());
}

#[test]
fn tui_config_parses_control_socket_table() {
    let raw = r#"
[control_socket]
enabled = true
"#;
    let parsed: ConfigFile = toml::from_str(raw).expect("parse control_socket config");

    let socket = parsed
        .base
        .control_socket
        .expect("control_socket table should parse");
    assert!(socket.enabled);

    // Off by default: a config without the table leaves the feature off.
    let absent: ConfigFile =
        toml::from_str("model = \"demo\"").expect("parse config without control_socket table");
    assert!(absent.base.control_socket.is_none());

    // An empty table stays off.
    let empty: ConfigFile =
        toml::from_str("[control_socket]").expect("parse empty control_socket table");
    assert!(
        !empty
            .base
            .control_socket
            .expect("table should parse")
            .enabled,
        "empty table must leave the socket off"
    );
}

#[test]
fn tui_config_parses_hotbar_bindings() {
    let raw = r#"
[[hotbar]]
slot = 1
label = "Plan"
action = "mode.plan"

[[hotbar]]
slot = 2
action = "session.compact"
"#;
    let parsed: ConfigFile = toml::from_str(raw).expect("parse hotbar config");

    let resolved = parsed
        .base
        .resolve_hotbar_bindings(&["mode.plan", "session.compact"]);

    assert_eq!(resolved.warnings, Vec::new());
    assert_eq!(
        resolved
            .bindings
            .iter()
            .map(|binding| (
                binding.slot,
                binding.action.as_str(),
                binding.label.as_deref()
            ))
            .collect::<Vec<_>>(),
        vec![(1, "mode.plan", Some("Plan")), (2, "session.compact", None),]
    );
}

#[test]
fn tui_config_empty_hotbar_array_disables_defaults() {
    let parsed: ConfigFile = toml::from_str("hotbar = []\n").expect("parse empty hotbar");

    let resolved = parsed
        .base
        .resolve_hotbar_bindings(&["mode.plan", "session.compact"]);

    assert_eq!(resolved.warnings, Vec::new());
    assert_eq!(resolved.bindings, Vec::new());
}

#[test]
fn profile_hotbar_override_replaces_entire_user_list() {
    let mut profiles = HashMap::new();
    profiles.insert(
        "compact".to_string(),
        Config {
            hotbar: Some(vec![codewhale_config::HotbarBindingToml {
                slot: 2,
                action: "session.compact".to_string(),
                label: Some("Compact".to_string()),
            }]),
            ..Config::default()
        },
    );
    let config = ConfigFile {
        base: Config {
            hotbar: Some(vec![codewhale_config::HotbarBindingToml {
                slot: 1,
                action: "mode.plan".to_string(),
                label: Some("Plan".to_string()),
            }]),
            ..Config::default()
        },
        profiles: Some(profiles),
    };

    let merged = apply_profile(config, Some("compact")).expect("profile");

    assert_eq!(
        merged.hotbar,
        Some(vec![codewhale_config::HotbarBindingToml {
            slot: 2,
            action: "session.compact".to_string(),
            label: Some("Compact".to_string()),
        }])
    );
}

#[test]
fn profile_without_scenario() {
    // Scenario consolidation of: profile_without_hotbar_keeps_base_hotbar, profile_without_context_does_not_disable_base_context
    // from profile_without_hotbar_keeps_base_hotbar
    {
        let mut profiles = HashMap::new();
        profiles.insert("work".to_string(), Config::default());
        let config = ConfigFile {
            base: Config {
                hotbar: Some(vec![codewhale_config::HotbarBindingToml {
                    slot: 1,
                    action: "mode.plan".to_string(),
                    label: None,
                }]),
                ..Config::default()
            },
            profiles: Some(profiles),
        };

        let merged = apply_profile(config, Some("work")).expect("profile");

        assert_eq!(
            merged.hotbar,
            Some(vec![codewhale_config::HotbarBindingToml {
                slot: 1,
                action: "mode.plan".to_string(),
                label: None,
            }])
        );
    }
    // from profile_without_context_does_not_disable_base_context
    {
        let mut profiles = HashMap::new();
        profiles.insert("work".to_string(), Config::default());
        let config = ConfigFile {
            base: Config {
                context: ContextConfig {
                    enabled: Some(true),
                    ..Default::default()
                },
                ..Default::default()
            },
            profiles: Some(profiles),
        };

        let merged = apply_profile(config, Some("work")).expect("profile");
        assert_eq!(merged.context.enabled, Some(true));
    }
}

#[test]
fn update_config_scenario() {
    // Scenario consolidation of: update_config_defaults_to_enabled_without_uri, update_config_deserializes_disable_and_custom_uri
    // from update_config_defaults_to_enabled_without_uri
    {
        let config = Config::default();
        assert_eq!(config.update, None);
        assert_eq!(config.update_config(), UpdateConfig::default());
        assert!(config.update_config().check_for_updates);
        assert_eq!(config.update_config().update_uri(), None);
    }
    // from update_config_deserializes_disable_and_custom_uri
    {
        let config: Config = toml::from_str(
            r#"
            [update]
            check_for_updates = false
            update_uri = "https://mirror.example/releases/latest"
            "#,
        )
        .expect("update config");

        let update = config.update_config();
        assert!(!update.check_for_updates);
        assert_eq!(
            update.update_uri(),
            Some("https://mirror.example/releases/latest")
        );
    }
}

#[test]
fn network_policy_toml_maps_proxy_hosts_to_runtime_policy() {
    let policy: NetworkPolicyToml = toml::from_str(
        r#"
        default = "allow"
        proxy = ["github.com", ".githubusercontent.com"]
        proxy_fake_ip_cidrs = ["198.18.0.0/15"]
        "#,
    )
    .expect("network policy toml");

    let runtime = policy.into_runtime();

    assert_eq!(runtime.proxy, ["github.com", ".githubusercontent.com"]);
    assert_eq!(runtime.proxy_fake_ip_cidrs, ["198.18.0.0/15"]);
    assert!(runtime.trusts_proxy_fakeip_host("github.com"));
    assert!(runtime.trusts_proxy_fakeip_host("raw.githubusercontent.com"));
}

#[test]
fn verifier_config_parses_and_merges_overrides() {
    let config: Config = toml::from_str(
        r#"
        [verifier]
        enabled = true
        "#,
    )
    .expect("parse verifier config");

    let verifier = config.verifier.expect("verifier table");
    assert!(verifier.enabled);

    let merged = merge_config(
        Config {
            verifier: Some(codewhale_config::VerifierConfigToml { enabled: false }),
            ..Config::default()
        },
        Config {
            verifier: Some(codewhale_config::VerifierConfigToml { enabled: true }),
            ..Config::default()
        },
    );

    assert!(merged.verifier.expect("merged verifier").enabled);
}

#[test]
fn workflow_config_defaults_when_omitted_and_overrides_round_trip() {
    // #4128: omitted `[workflow]` resolves through the accessor to product
    // defaults; explicit overrides load and survive serialize → parse.
    let omitted: Config = toml::from_str("").expect("empty config");
    assert!(omitted.workflow.is_none());
    assert_eq!(
        omitted.workflow_config(),
        codewhale_config::WorkflowConfigToml::default()
    );

    let config: Config = toml::from_str(
        r#"
        [workflow]
        automatic = false
        auto_start_read_only = false
        require_approval_for_writes = true
        auto_start_child_limit = 4
        max_children = 32
        max_depth = 1
        default_token_budget = 90000
        max_parallel_writes_without_worktree = 1
        persist_completed_activity = false
        persist_completed_across_restarts = false
        "#,
    )
    .expect("parse workflow config");

    let workflow = config.workflow.clone().expect("workflow table");
    assert!(!workflow.automatic);
    assert!(!workflow.auto_start_read_only);
    assert!(workflow.require_approval_for_writes);
    assert_eq!(workflow.auto_start_child_limit, 4);
    assert_eq!(workflow.max_children, 32);
    assert_eq!(workflow.max_depth, 1);
    assert_eq!(workflow.default_token_budget, 90_000);
    assert_eq!(workflow.max_parallel_writes_without_worktree, 1);
    assert!(!workflow.persist_completed_activity);
    assert!(!workflow.persist_completed_across_restarts);
    assert_eq!(config.workflow_config(), workflow);

    let serialized = toml::to_string_pretty(&workflow).expect("serialize workflow");
    let round_tripped: codewhale_config::WorkflowConfigToml =
        toml::from_str(&serialized).expect("round-trip parse");
    assert_eq!(round_tripped, workflow);

    // Profile/project overlays replace the whole table when present.
    let merged = merge_config(
        Config {
            workflow: Some(codewhale_config::WorkflowConfigToml::default()),
            ..Config::default()
        },
        Config {
            workflow: Some(workflow.clone()),
            ..Config::default()
        },
    );
    assert_eq!(merged.workflow_config(), workflow);
}

#[test]
fn window_title_config_parses_and_overlays() {
    // `title` is a plain optional root key: absent config → `None`.
    let omitted: Config = toml::from_str("").expect("empty config");
    assert_eq!(omitted.title, None);

    let config: Config = toml::from_str(r#"title = "workspace-x""#).expect("parse title config");
    assert_eq!(config.title.as_deref(), Some("workspace-x"));

    // Overlay wins over the base when both define a title.
    let merged = merge_config(
        Config {
            title: Some("base-title".to_string()),
            ..Config::default()
        },
        Config {
            title: Some("override-title".to_string()),
            ..Config::default()
        },
    );
    assert_eq!(merged.title.as_deref(), Some("override-title"));

    // Base title survives an overlay that does not mention it.
    let merged = merge_config(
        Config {
            title: Some("base-title".to_string()),
            ..Config::default()
        },
        Config::default(),
    );
    assert_eq!(merged.title.as_deref(), Some("base-title"));
}

#[test]
fn search_provider_scenario() {
    // Scenario consolidation of: search_provider_defaults_to_firecrawl, search_provider_resolution_reports_default_source, search_provider_resolution_reports_config_source, search_provider_resolution_reports_env_override_source, search_provider_env_override_accepts_baidu, search_provider_resolution_ignores_invalid_env_override
    // from search_provider_defaults_to_firecrawl
    {
        assert_eq!(SearchProvider::default(), SearchProvider::Firecrawl);
        assert_eq!(
            SearchProvider::parse("fire-crawl"),
            Some(SearchProvider::Firecrawl)
        );
        assert_eq!(SearchProvider::Firecrawl.as_str(), "firecrawl");
    }
    // from search_provider_resolution_reports_default_source
    {
        let _guard = lock_test_env();
        let prev = env::var_os("DEEPSEEK_SEARCH_PROVIDER");
        unsafe { env::remove_var("DEEPSEEK_SEARCH_PROVIDER") };

        let resolution = Config::default().search_provider_resolution();

        unsafe { EnvGuard::restore_var("DEEPSEEK_SEARCH_PROVIDER", prev) };
        assert_eq!(resolution.provider, SearchProvider::Firecrawl);
        assert_eq!(resolution.source, SearchProviderSource::Default);
    }
    // from search_provider_resolution_reports_config_source
    {
        let _guard = lock_test_env();
        let prev = env::var_os("DEEPSEEK_SEARCH_PROVIDER");
        unsafe { env::remove_var("DEEPSEEK_SEARCH_PROVIDER") };
        let config: Config = toml::from_str(
            r#"
            [search]
            provider = "tavily"
            "#,
        )
        .expect("search config");

        let resolution = config.search_provider_resolution();

        unsafe { EnvGuard::restore_var("DEEPSEEK_SEARCH_PROVIDER", prev) };
        assert_eq!(resolution.provider, SearchProvider::Tavily);
        assert_eq!(resolution.source, SearchProviderSource::Config);
    }
    // from search_provider_resolution_reports_env_override_source
    {
        let _guard = lock_test_env();
        let prev = env::var_os("DEEPSEEK_SEARCH_PROVIDER");
        unsafe { env::set_var("DEEPSEEK_SEARCH_PROVIDER", "bocha") };
        let config: Config = toml::from_str(
            r#"
            [search]
            provider = "duckduckgo"
            "#,
        )
        .expect("search config");

        let resolution = config.search_provider_resolution();

        unsafe { EnvGuard::restore_var("DEEPSEEK_SEARCH_PROVIDER", prev) };
        assert_eq!(resolution.provider, SearchProvider::Bocha);
        assert_eq!(resolution.source, SearchProviderSource::EnvOverride);
    }
    // from search_provider_env_override_accepts_baidu
    {
        let _guard = lock_test_env();
        let prev = env::var_os("DEEPSEEK_SEARCH_PROVIDER");
        unsafe { env::set_var("DEEPSEEK_SEARCH_PROVIDER", "baidu") };
        let config: Config = toml::from_str(
            r#"
            [search]
            provider = "duckduckgo"
            "#,
        )
        .expect("search config");

        let resolution = config.search_provider_resolution();

        unsafe { EnvGuard::restore_var("DEEPSEEK_SEARCH_PROVIDER", prev) };
        assert_eq!(resolution.provider, SearchProvider::Baidu);
        assert_eq!(resolution.source, SearchProviderSource::EnvOverride);
    }
    // from search_provider_resolution_ignores_invalid_env_override
    {
        let _guard = lock_test_env();
        let prev = env::var_os("DEEPSEEK_SEARCH_PROVIDER");
        unsafe { env::set_var("DEEPSEEK_SEARCH_PROVIDER", "not-a-provider") };
        let config: Config = toml::from_str(
            r#"
            [search]
            provider = "tavily"
            "#,
        )
        .expect("search config");

        let resolution = config.search_provider_resolution();

        unsafe { EnvGuard::restore_var("DEEPSEEK_SEARCH_PROVIDER", prev) };
        assert_eq!(resolution.provider, SearchProvider::Tavily);
        assert_eq!(resolution.source, SearchProviderSource::Config);
    }
}

#[test]
fn tools_always_load_parses_and_trims_names() {
    let parsed: ConfigFile = toml::from_str(
        r#"
        [tools]
        always_load = ["git_show", " notify ", ""]
        "#,
    )
    .expect("tools config");

    let names = parsed.base.tools_always_load();

    assert!(names.contains("git_show"));
    assert!(names.contains("notify"));
    assert!(!names.contains(""));
}

#[test]
fn explicit_duckduckgo_search_provider_is_preserved() {
    let config: Config = toml::from_str(
        r#"
        [search]
        provider = "duckduckgo"
        "#,
    )
    .expect("search config");

    assert_eq!(
        config.search.and_then(|search| search.provider),
        Some(SearchProvider::DuckDuckGo)
    );
}

#[test]
fn search_config_preserves_custom_base_url() {
    let config: Config = toml::from_str(
        r#"
        [search]
        provider = "duckduckgo"
        base_url = "https://search.internal.example/html/"
        "#,
    )
    .expect("search config");

    let search = config.search.expect("search table");
    assert_eq!(search.provider, Some(SearchProvider::DuckDuckGo));
    assert_eq!(
        search.base_url.as_deref(),
        Some("https://search.internal.example/html/")
    );
}

#[test]
fn explicit_searxng_search_provider_is_preserved() {
    let config: Config = toml::from_str(
        r#"
        [search]
        provider = "searxng"
        base_url = "https://search.internal.example/"
        "#,
    )
    .expect("search config");

    let search = config.search.expect("search table");
    assert_eq!(search.provider, Some(SearchProvider::Searxng));
    assert_eq!(
        search.base_url.as_deref(),
        Some("https://search.internal.example/")
    );
}

#[test]
fn searxng_search_provider_aliases_parse_and_round_trip() {
    assert_eq!(
        SearchProvider::parse("searxng"),
        Some(SearchProvider::Searxng)
    );
    assert_eq!(
        SearchProvider::parse("searx-ng"),
        Some(SearchProvider::Searxng)
    );
    assert_eq!(
        SearchProvider::parse("searx_ng"),
        Some(SearchProvider::Searxng)
    );
    assert_eq!(
        SearchProvider::parse("searx"),
        Some(SearchProvider::Searxng)
    );
    assert_eq!(SearchProvider::Searxng.as_str(), "searxng");
}

#[test]
fn explicit_baidu_search_provider_is_preserved() {
    let config: Config = toml::from_str(
        r#"
        [search]
        provider = "baidu"
        "#,
    )
    .expect("search config");

    assert_eq!(
        config.search.and_then(|search| search.provider),
        Some(SearchProvider::Baidu)
    );
}

#[test]
fn baidu_search_provider_aliases_parse() {
    assert_eq!(SearchProvider::parse("baidu"), Some(SearchProvider::Baidu));
    assert_eq!(
        SearchProvider::parse("baidu-search"),
        Some(SearchProvider::Baidu)
    );
    assert_eq!(
        SearchProvider::parse("baidu_ai_search"),
        Some(SearchProvider::Baidu)
    );
}

#[test]
fn volcengine_search_provider_aliases_parse_and_deserialize() {
    assert_eq!(
        SearchProvider::parse("volcengine"),
        Some(SearchProvider::Volcengine)
    );
    assert_eq!(
        SearchProvider::parse("volcengine-ark"),
        Some(SearchProvider::Volcengine)
    );

    let config: Config = toml::from_str(
        r#"
        [search]
        provider = "volcengine-ark"
        "#,
    )
    .expect("volcengine search config");

    assert_eq!(
        config.search.and_then(|search| search.provider),
        Some(SearchProvider::Volcengine)
    );
}

#[test]
fn explicit_sofya_search_provider_is_preserved() {
    let config: Config = toml::from_str(
        r#"
        [search]
        provider = "sofya"
        "#,
    )
    .expect("sofya search config");

    assert_eq!(
        config.search.and_then(|search| search.provider),
        Some(SearchProvider::Sofya)
    );
}

#[test]
fn sofya_search_provider_parses_and_round_trips() {
    assert_eq!(SearchProvider::parse("sofya"), Some(SearchProvider::Sofya));
    assert_eq!(SearchProvider::parse("Sofya"), Some(SearchProvider::Sofya));
    assert_eq!(SearchProvider::Sofya.as_str(), "sofya");
}

#[test]
fn live_search_provider_update_preserves_environment_precedence() {
    let _guard = lock_test_env();
    let previous_codewhale = env::var_os("CODEWHALE_SEARCH_PROVIDER");
    let previous_deepseek = env::var_os("DEEPSEEK_SEARCH_PROVIDER");
    unsafe {
        env::set_var("CODEWHALE_SEARCH_PROVIDER", "bocha");
        env::remove_var("DEEPSEEK_SEARCH_PROVIDER");
    }
    let mut config = Config::default();

    let effective = config.set_search_provider(SearchProvider::DuckDuckGo);

    unsafe {
        EnvGuard::restore_var("CODEWHALE_SEARCH_PROVIDER", previous_codewhale);
        EnvGuard::restore_var("DEEPSEEK_SEARCH_PROVIDER", previous_deepseek);
    }
    assert_eq!(effective, SearchProvider::Bocha);
    assert_eq!(
        config.search.and_then(|search| search.provider),
        Some(SearchProvider::DuckDuckGo),
        "the requested config value should still be retained beneath the override"
    );
}

#[test]
fn notification_defaults_and_live_updates_share_one_consistent_model() {
    let mut notifications = NotificationsConfig::default();
    assert_eq!(notifications.threshold_secs, 30);
    assert_eq!(notifications.completion_sound, CompletionSound::Off);
    assert_eq!(
        Config::default().notifications_config().threshold_secs,
        notifications.threshold_secs
    );
    assert_eq!(
        Config::default().notifications_config().completion_sound,
        CompletionSound::Off,
        "a fresh install must never opt itself into an audible completion cue"
    );

    notifications.apply_update(NotificationConfigUpdate::Method(NotificationMethod::Osc9));
    notifications.apply_update(NotificationConfigUpdate::Quiet(true));

    assert_eq!(notifications.method, NotificationMethod::Osc9);
    assert!(notifications.quiet);
    assert_eq!(
        notifications.threshold_secs, 30,
        "field deltas must preserve both defaults and earlier live edits"
    );
}

#[test]
fn notification_condition_accepts_background_only_policy() {
    let config: Config = toml::from_str(
        r#"
        [tui]
        notification_condition = "unfocused"
        "#,
    )
    .expect("unfocused attention policy should parse");

    assert_eq!(
        config.tui.and_then(|tui| tui.notification_condition),
        Some(NotificationCondition::Unfocused)
    );
}

#[test]
fn apply_env_scenario() {
    // Scenario consolidation of: apply_env_overrides_sets_search_api_key, apply_env_overrides_sets_search_base_url
    // from apply_env_overrides_sets_search_api_key
    {
        let _guard = lock_test_env();
        let prev = env::var_os("DEEPSEEK_SEARCH_API_KEY");
        unsafe { env::set_var("DEEPSEEK_SEARCH_API_KEY", "search-env-key") };
        let mut config = Config::default();

        apply_env_overrides(&mut config, ConfigEnvironmentPolicy::Runtime);

        unsafe { EnvGuard::restore_var("DEEPSEEK_SEARCH_API_KEY", prev) };
        assert_eq!(
            config.search.and_then(|search| search.api_key),
            Some("search-env-key".to_string())
        );
    }
    // from apply_env_overrides_sets_search_base_url
    {
        let _guard = lock_test_env();
        let prev_codewhale = env::var_os("CODEWHALE_SEARCH_BASE_URL");
        let prev_deepseek = env::var_os("DEEPSEEK_SEARCH_BASE_URL");
        unsafe {
            env::remove_var("CODEWHALE_SEARCH_BASE_URL");
            env::set_var(
                "DEEPSEEK_SEARCH_BASE_URL",
                "https://search.internal.example/html/",
            )
        };
        let mut config = Config::default();

        apply_env_overrides(&mut config, ConfigEnvironmentPolicy::Runtime);

        unsafe {
            EnvGuard::restore_var("CODEWHALE_SEARCH_BASE_URL", prev_codewhale);
            EnvGuard::restore_var("DEEPSEEK_SEARCH_BASE_URL", prev_deepseek);
        }
        assert_eq!(
            config.search.and_then(|search| search.base_url),
            Some("https://search.internal.example/html/".to_string())
        );
    }
}

#[test]
fn apply_env_overrides_yolo_prefers_codewhale_and_keeps_deepseek_alias() {
    let _guard = lock_test_env();
    let codewhale_prev = env::var_os("CODEWHALE_YOLO");
    let deepseek_prev = env::var_os("DEEPSEEK_YOLO");
    let mut config = Config::default();

    // Only the canonical name is set.
    unsafe {
        env::set_var("CODEWHALE_YOLO", "true");
        env::remove_var("DEEPSEEK_YOLO");
    }
    apply_env_overrides(&mut config, ConfigEnvironmentPolicy::Runtime);
    assert_eq!(
        config.yolo,
        Some(true),
        "CODEWHALE_YOLO=true must enable the yolo posture"
    );

    // Only the deprecated alias is set: it must keep working through 0.9.x.
    unsafe {
        env::remove_var("CODEWHALE_YOLO");
        env::set_var("DEEPSEEK_YOLO", "true");
    }
    apply_env_overrides(&mut config, ConfigEnvironmentPolicy::Runtime);
    assert_eq!(
        config.yolo,
        Some(true),
        "DEEPSEEK_YOLO remains a read-only deprecated alias until 0.10 (#5443)"
    );

    // Both set: the canonical name wins.
    unsafe { env::set_var("CODEWHALE_YOLO", "false") };
    apply_env_overrides(&mut config, ConfigEnvironmentPolicy::Runtime);
    assert_eq!(
        config.yolo,
        Some(false),
        "CODEWHALE_YOLO must win over the deprecated DEEPSEEK_YOLO alias"
    );

    unsafe {
        EnvGuard::restore_var("CODEWHALE_YOLO", codewhale_prev);
        EnvGuard::restore_var("DEEPSEEK_YOLO", deepseek_prev);
    }
}

#[test]
fn structural_config_load_keeps_safe_environment_overrides_but_omits_secret_values() {
    let _guard = lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let config_path = temp.path().join("config.toml");
    fs::write(&config_path, "").expect("empty config");
    let _home = EnvVarGuard::set("CODEWHALE_HOME", temp.path().join("home"));
    let _profile = EnvVarGuard::remove("CODEWHALE_PROFILE");
    let _legacy_profile = EnvVarGuard::remove("DEEPSEEK_PROFILE");
    let _managed = EnvVarGuard::remove("CODEWHALE_MANAGED_CONFIG_PATH");
    let _legacy_managed = EnvVarGuard::remove("DEEPSEEK_MANAGED_CONFIG_PATH");
    let _requirements = EnvVarGuard::remove("CODEWHALE_REQUIREMENTS_PATH");
    let _legacy_requirements = EnvVarGuard::remove("DEEPSEEK_REQUIREMENTS_PATH");
    let _headers = EnvVarGuard::set(
        "CODEWHALE_HTTP_HEADERS",
        "Authorization=structural-header-secret",
    );
    let _legacy_headers = EnvVarGuard::set(
        "DEEPSEEK_HTTP_HEADERS",
        "Authorization=legacy-structural-header-secret",
    );
    let _sandbox_key = EnvVarGuard::set("CODEWHALE_SANDBOX_API_KEY", "structural-sandbox-secret");
    let _legacy_sandbox_key = EnvVarGuard::set(
        "DEEPSEEK_SANDBOX_API_KEY",
        "legacy-structural-sandbox-secret",
    );
    let _search_key = EnvVarGuard::set("CODEWHALE_SEARCH_API_KEY", "structural-search-secret");
    let _legacy_search_key =
        EnvVarGuard::set("DEEPSEEK_SEARCH_API_KEY", "legacy-structural-search-secret");
    let _base_url = EnvVarGuard::set("CODEWHALE_BASE_URL", "https://safe.example:8443/v1");
    let _allow_shell = EnvVarGuard::set("CODEWHALE_ALLOW_SHELL", "false");

    let runtime = Config::load(Some(config_path.clone()), None).expect("runtime config");
    assert!(runtime.http_headers.is_some());
    assert_eq!(
        runtime.sandbox_api_key.as_deref(),
        Some("structural-sandbox-secret")
    );
    assert_eq!(
        runtime
            .search
            .as_ref()
            .and_then(|search| search.api_key.as_deref()),
        Some("structural-search-secret")
    );

    let structural = Config::load_structural(Some(config_path), None).expect("structural config");
    assert!(structural.http_headers.is_none());
    assert!(structural.sandbox_api_key.is_none());
    assert!(
        structural
            .search
            .as_ref()
            .and_then(|search| search.api_key.as_deref())
            .is_none()
    );
    assert_eq!(
        structural.base_url.as_deref(),
        Some("https://safe.example:8443/v1")
    );
    assert_eq!(structural.allow_shell, Some(false));

    let rendered = format!("{structural:?}");
    for sentinel in [
        "structural-header-secret",
        "legacy-structural-header-secret",
        "structural-sandbox-secret",
        "legacy-structural-sandbox-secret",
        "structural-search-secret",
        "legacy-structural-search-secret",
    ] {
        assert!(
            !rendered.contains(sentinel),
            "structural config retained {sentinel}"
        );
    }
}

#[test]
fn codewhale_search_base_url_env_wins_over_legacy_alias() {
    let _guard = lock_test_env();
    let prev_codewhale = env::var_os("CODEWHALE_SEARCH_BASE_URL");
    let prev_deepseek = env::var_os("DEEPSEEK_SEARCH_BASE_URL");
    unsafe {
        env::set_var(
            "CODEWHALE_SEARCH_BASE_URL",
            "https://codewhale-search.example/html/",
        );
        env::set_var(
            "DEEPSEEK_SEARCH_BASE_URL",
            "https://legacy-search.example/html/",
        );
    }
    let mut config = Config::default();

    apply_env_overrides(&mut config, ConfigEnvironmentPolicy::Runtime);

    unsafe {
        EnvGuard::restore_var("CODEWHALE_SEARCH_BASE_URL", prev_codewhale);
        EnvGuard::restore_var("DEEPSEEK_SEARCH_BASE_URL", prev_deepseek);
    }
    assert_eq!(
        config.search.and_then(|search| search.base_url),
        Some("https://codewhale-search.example/html/".to_string())
    );
}

#[test]
fn codewhale_prefer_bwrap_env_wins_over_legacy_alias() {
    let _guard = lock_test_env();
    let _primary = EnvVarGuard::set("CODEWHALE_PREFER_BWRAP", "false");
    let _legacy = EnvVarGuard::set("DEEPSEEK_PREFER_BWRAP", "true");
    let mut config = Config {
        prefer_bwrap: Some(true),
        ..Config::default()
    };

    apply_env_overrides(&mut config, ConfigEnvironmentPolicy::Runtime);

    assert_eq!(config.prefer_bwrap, Some(false));
}

#[test]
fn legacy_prefer_bwrap_env_remains_a_compatible_alias() {
    let _guard = lock_test_env();
    let _primary = EnvVarGuard::remove("CODEWHALE_PREFER_BWRAP");
    let _legacy = EnvVarGuard::set("DEEPSEEK_PREFER_BWRAP", "true");
    let mut config = Config::default();

    apply_env_overrides(&mut config, ConfigEnvironmentPolicy::Runtime);

    assert_eq!(config.prefer_bwrap, Some(true));
}

struct EnvGuard {
    // Seal path overrides through EnvVarGuard so default_config_path honors
    // this fixture instead of the isolated test root (#5355, #5359).
    _sealed_home: EnvVarGuard,
    _sealed_userprofile: EnvVarGuard,
    _sealed_codewhale_home: EnvVarGuard,
    _sealed_codewhale_config_path: EnvVarGuard,
    _sealed_deepseek_config_path: EnvVarGuard,
    home: Option<OsString>,
    userprofile: Option<OsString>,
    codewhale_home: Option<OsString>,
    codewhale_config_path: Option<OsString>,
    deepseek_config_path: Option<OsString>,
    codewhale_secret_backend: Option<OsString>,
    deepseek_secret_backend: Option<OsString>,
    deepseek_provider: Option<OsString>,
    deepseek_api_key: Option<OsString>,
    deepseek_base_url: Option<OsString>,
    deepseek_http_headers: Option<OsString>,
    deepseek_model: Option<OsString>,
    deepseek_default_text_model: Option<OsString>,
    codewhale_provider: Option<OsString>,
    codewhale_model: Option<OsString>,
    codewhale_base_url: Option<OsString>,
    nvidia_api_key: Option<OsString>,
    nvidia_nim_api_key: Option<OsString>,
    nim_base_url: Option<OsString>,
    nvidia_base_url: Option<OsString>,
    nvidia_nim_base_url: Option<OsString>,
    nvidia_nim_model: Option<OsString>,
    openai_api_key: Option<OsString>,
    openai_base_url: Option<OsString>,
    openai_model: Option<OsString>,
    atlascloud_api_key: Option<OsString>,
    atlascloud_base_url: Option<OsString>,
    atlascloud_model: Option<OsString>,
    wanjie_ark_api_key: Option<OsString>,
    wanjie_api_key: Option<OsString>,
    wanjie_maas_api_key: Option<OsString>,
    wanjie_ark_base_url: Option<OsString>,
    wanjie_base_url: Option<OsString>,
    wanjie_maas_base_url: Option<OsString>,
    wanjie_ark_model: Option<OsString>,
    wanjie_model: Option<OsString>,
    wanjie_maas_model: Option<OsString>,
    openrouter_api_key: Option<OsString>,
    openrouter_base_url: Option<OsString>,
    openrouter_model: Option<OsString>,
    volcengine_api_key: Option<OsString>,
    volcengine_ark_api_key: Option<OsString>,
    ark_api_key: Option<OsString>,
    volcengine_base_url: Option<OsString>,
    volcengine_ark_base_url: Option<OsString>,
    ark_base_url: Option<OsString>,
    volcengine_model: Option<OsString>,
    volcengine_ark_model: Option<OsString>,
    xiaomi_mimo_token_plan_api_key: Option<OsString>,
    mimo_token_plan_api_key: Option<OsString>,
    xiaomi_mimo_api_key: Option<OsString>,
    xiaomi_api_key: Option<OsString>,
    mimo_api_key: Option<OsString>,
    xiaomi_mimo_base_url: Option<OsString>,
    mimo_base_url: Option<OsString>,
    xiaomi_mimo_model: Option<OsString>,
    mimo_model: Option<OsString>,
    xiaomi_mimo_mode: Option<OsString>,
    mimo_mode: Option<OsString>,
    novita_api_key: Option<OsString>,
    novita_base_url: Option<OsString>,
    novita_model: Option<OsString>,
    fireworks_api_key: Option<OsString>,
    fireworks_base_url: Option<OsString>,
    fireworks_model: Option<OsString>,
    siliconflow_api_key: Option<OsString>,
    siliconflow_base_url: Option<OsString>,
    siliconflow_model: Option<OsString>,
    arcee_api_key: Option<OsString>,
    arcee_base_url: Option<OsString>,
    arcee_model: Option<OsString>,
    moonshot_api_key: Option<OsString>,
    moonshot_base_url: Option<OsString>,
    moonshot_model: Option<OsString>,
    kimi_api_key: Option<OsString>,
    kimi_base_url: Option<OsString>,
    kimi_model: Option<OsString>,
    kimi_model_name: Option<OsString>,
    kimi_code_home: Option<OsString>,
    kimi_share_dir: Option<OsString>,
    sglang_api_key: Option<OsString>,
    sglang_base_url: Option<OsString>,
    sglang_model: Option<OsString>,
    vllm_api_key: Option<OsString>,
    vllm_base_url: Option<OsString>,
    vllm_model: Option<OsString>,
    ollama_cloud_api_key: Option<OsString>,
    ollama_cloud_base_url: Option<OsString>,
    ollama_cloud_model: Option<OsString>,
    ollama_api_key: Option<OsString>,
    ollama_base_url: Option<OsString>,
    ollama_model: Option<OsString>,
    huggingface_api_key: Option<OsString>,
    huggingface_token: Option<OsString>,
    huggingface_base_url: Option<OsString>,
    hf_base_url: Option<OsString>,
    huggingface_model: Option<OsString>,
    hf_model: Option<OsString>,
}

impl EnvGuard {
    fn new(home: &Path) -> Self {
        let home_str = OsString::from(home.as_os_str());
        let config_path = home.join(".deepseek").join("config.toml");
        let config_str = OsString::from(config_path.as_os_str());
        let home_prev = env::var_os("HOME");
        let userprofile_prev = env::var_os("USERPROFILE");
        let codewhale_home_prev = env::var_os("CODEWHALE_HOME");
        let codewhale_config_prev = env::var_os("CODEWHALE_CONFIG_PATH");
        let deepseek_config_prev = env::var_os("DEEPSEEK_CONFIG_PATH");
        let codewhale_secret_backend_prev = env::var_os("CODEWHALE_SECRET_BACKEND");
        let deepseek_secret_backend_prev = env::var_os("DEEPSEEK_SECRET_BACKEND");
        let deepseek_provider_prev = env::var_os("DEEPSEEK_PROVIDER");
        let api_key_prev = env::var_os("DEEPSEEK_API_KEY");
        let base_url_prev = env::var_os("DEEPSEEK_BASE_URL");
        let http_headers_prev = env::var_os("DEEPSEEK_HTTP_HEADERS");
        let model_prev = env::var_os("DEEPSEEK_MODEL");
        let default_text_model_prev = env::var_os("DEEPSEEK_DEFAULT_TEXT_MODEL");
        let codewhale_provider_prev = env::var_os("CODEWHALE_PROVIDER");
        let codewhale_model_prev = env::var_os("CODEWHALE_MODEL");
        let codewhale_base_url_prev = env::var_os("CODEWHALE_BASE_URL");
        let nvidia_api_key_prev = env::var_os("NVIDIA_API_KEY");
        let nvidia_nim_api_key_prev = env::var_os("NVIDIA_NIM_API_KEY");
        let nim_base_url_prev = env::var_os("NIM_BASE_URL");
        let nvidia_base_url_prev = env::var_os("NVIDIA_BASE_URL");
        let nvidia_nim_base_url_prev = env::var_os("NVIDIA_NIM_BASE_URL");
        let nvidia_nim_model_prev = env::var_os("NVIDIA_NIM_MODEL");
        let openai_api_key_prev = env::var_os("OPENAI_API_KEY");
        let openai_base_url_prev = env::var_os("OPENAI_BASE_URL");
        let openai_model_prev = env::var_os("OPENAI_MODEL");
        let atlascloud_api_key_prev = env::var_os("ATLASCLOUD_API_KEY");
        let atlascloud_base_url_prev = env::var_os("ATLASCLOUD_BASE_URL");
        let atlascloud_model_prev = env::var_os("ATLASCLOUD_MODEL");
        let wanjie_ark_api_key_prev = env::var_os("WANJIE_ARK_API_KEY");
        let wanjie_api_key_prev = env::var_os("WANJIE_API_KEY");
        let wanjie_maas_api_key_prev = env::var_os("WANJIE_MAAS_API_KEY");
        let wanjie_ark_base_url_prev = env::var_os("WANJIE_ARK_BASE_URL");
        let wanjie_base_url_prev = env::var_os("WANJIE_BASE_URL");
        let wanjie_maas_base_url_prev = env::var_os("WANJIE_MAAS_BASE_URL");
        let wanjie_ark_model_prev = env::var_os("WANJIE_ARK_MODEL");
        let wanjie_model_prev = env::var_os("WANJIE_MODEL");
        let wanjie_maas_model_prev = env::var_os("WANJIE_MAAS_MODEL");
        let openrouter_api_key_prev = env::var_os("OPENROUTER_API_KEY");
        let openrouter_base_url_prev = env::var_os("OPENROUTER_BASE_URL");
        let openrouter_model_prev = env::var_os("OPENROUTER_MODEL");
        let volcengine_api_key_prev = env::var_os("VOLCENGINE_API_KEY");
        let volcengine_ark_api_key_prev = env::var_os("VOLCENGINE_ARK_API_KEY");
        let ark_api_key_prev = env::var_os("ARK_API_KEY");
        let volcengine_base_url_prev = env::var_os("VOLCENGINE_BASE_URL");
        let volcengine_ark_base_url_prev = env::var_os("VOLCENGINE_ARK_BASE_URL");
        let ark_base_url_prev = env::var_os("ARK_BASE_URL");
        let volcengine_model_prev = env::var_os("VOLCENGINE_MODEL");
        let volcengine_ark_model_prev = env::var_os("VOLCENGINE_ARK_MODEL");
        let xiaomi_mimo_token_plan_api_key_prev = env::var_os("XIAOMI_MIMO_TOKEN_PLAN_API_KEY");
        let mimo_token_plan_api_key_prev = env::var_os("MIMO_TOKEN_PLAN_API_KEY");
        let xiaomi_mimo_api_key_prev = env::var_os("XIAOMI_MIMO_API_KEY");
        let xiaomi_api_key_prev = env::var_os("XIAOMI_API_KEY");
        let mimo_api_key_prev = env::var_os("MIMO_API_KEY");
        let xiaomi_mimo_base_url_prev = env::var_os("XIAOMI_MIMO_BASE_URL");
        let mimo_base_url_prev = env::var_os("MIMO_BASE_URL");
        let xiaomi_mimo_model_prev = env::var_os("XIAOMI_MIMO_MODEL");
        let mimo_model_prev = env::var_os("MIMO_MODEL");
        let xiaomi_mimo_mode_prev = env::var_os("XIAOMI_MIMO_MODE");
        let mimo_mode_prev = env::var_os("MIMO_MODE");
        let novita_api_key_prev = env::var_os("NOVITA_API_KEY");
        let novita_base_url_prev = env::var_os("NOVITA_BASE_URL");
        let novita_model_prev = env::var_os("NOVITA_MODEL");
        let fireworks_api_key_prev = env::var_os("FIREWORKS_API_KEY");
        let fireworks_base_url_prev = env::var_os("FIREWORKS_BASE_URL");
        let fireworks_model_prev = env::var_os("FIREWORKS_MODEL");
        let siliconflow_api_key_prev = env::var_os("SILICONFLOW_API_KEY");
        let siliconflow_base_url_prev = env::var_os("SILICONFLOW_BASE_URL");
        let siliconflow_model_prev = env::var_os("SILICONFLOW_MODEL");
        let arcee_api_key_prev = env::var_os("ARCEE_API_KEY");
        let arcee_base_url_prev = env::var_os("ARCEE_BASE_URL");
        let arcee_model_prev = env::var_os("ARCEE_MODEL");
        let moonshot_api_key_prev = env::var_os("MOONSHOT_API_KEY");
        let moonshot_base_url_prev = env::var_os("MOONSHOT_BASE_URL");
        let moonshot_model_prev = env::var_os("MOONSHOT_MODEL");
        let kimi_api_key_prev = env::var_os("KIMI_API_KEY");
        let kimi_base_url_prev = env::var_os("KIMI_BASE_URL");
        let kimi_model_prev = env::var_os("KIMI_MODEL");
        let kimi_model_name_prev = env::var_os("KIMI_MODEL_NAME");
        let kimi_code_home_prev = env::var_os("KIMI_CODE_HOME");
        let kimi_share_dir_prev = env::var_os("KIMI_SHARE_DIR");
        let sglang_api_key_prev = env::var_os("SGLANG_API_KEY");
        let sglang_base_url_prev = env::var_os("SGLANG_BASE_URL");
        let sglang_model_prev = env::var_os("SGLANG_MODEL");
        let vllm_api_key_prev = env::var_os("VLLM_API_KEY");
        let vllm_base_url_prev = env::var_os("VLLM_BASE_URL");
        let vllm_model_prev = env::var_os("VLLM_MODEL");
        let ollama_cloud_api_key_prev = env::var_os("OLLAMA_CLOUD_API_KEY");
        let ollama_cloud_base_url_prev = env::var_os("OLLAMA_CLOUD_BASE_URL");
        let ollama_cloud_model_prev = env::var_os("OLLAMA_CLOUD_MODEL");
        let ollama_api_key_prev = env::var_os("OLLAMA_API_KEY");
        let ollama_base_url_prev = env::var_os("OLLAMA_BASE_URL");
        let ollama_model_prev = env::var_os("OLLAMA_MODEL");
        let huggingface_api_key_prev = env::var_os("HUGGINGFACE_API_KEY");
        let huggingface_token_prev = env::var_os("HF_TOKEN");
        let huggingface_base_url_prev = env::var_os("HUGGINGFACE_BASE_URL");
        let hf_base_url_prev = env::var_os("HF_BASE_URL");
        let huggingface_model_prev = env::var_os("HUGGINGFACE_MODEL");
        let hf_model_prev = env::var_os("HF_MODEL");
        let sealed_home = EnvVarGuard::set("HOME", &home_str);
        let sealed_userprofile = EnvVarGuard::set("USERPROFILE", &home_str);
        let sealed_codewhale_home = EnvVarGuard::remove("CODEWHALE_HOME");
        let sealed_codewhale_config_path = EnvVarGuard::remove("CODEWHALE_CONFIG_PATH");
        let sealed_deepseek_config_path = EnvVarGuard::set("DEEPSEEK_CONFIG_PATH", &config_str);
        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::remove_var("CODEWHALE_SECRET_BACKEND");
            env::remove_var("DEEPSEEK_SECRET_BACKEND");
            env::remove_var("DEEPSEEK_PROVIDER");
            env::remove_var("DEEPSEEK_API_KEY");
            env::remove_var("DEEPSEEK_BASE_URL");
            env::remove_var("DEEPSEEK_HTTP_HEADERS");
            env::remove_var("DEEPSEEK_MODEL");
            env::remove_var("DEEPSEEK_DEFAULT_TEXT_MODEL");
            env::remove_var("CODEWHALE_PROVIDER");
            env::remove_var("CODEWHALE_MODEL");
            env::remove_var("CODEWHALE_BASE_URL");
            env::remove_var("NVIDIA_API_KEY");
            env::remove_var("NVIDIA_NIM_API_KEY");
            env::remove_var("NIM_BASE_URL");
            env::remove_var("NVIDIA_BASE_URL");
            env::remove_var("NVIDIA_NIM_BASE_URL");
            env::remove_var("NVIDIA_NIM_MODEL");
            env::remove_var("OPENAI_API_KEY");
            env::remove_var("OPENAI_BASE_URL");
            env::remove_var("OPENAI_MODEL");
            env::remove_var("ATLASCLOUD_API_KEY");
            env::remove_var("ATLASCLOUD_BASE_URL");
            env::remove_var("ATLASCLOUD_MODEL");
            env::remove_var("WANJIE_ARK_API_KEY");
            env::remove_var("WANJIE_API_KEY");
            env::remove_var("WANJIE_MAAS_API_KEY");
            env::remove_var("WANJIE_ARK_BASE_URL");
            env::remove_var("WANJIE_BASE_URL");
            env::remove_var("WANJIE_MAAS_BASE_URL");
            env::remove_var("WANJIE_ARK_MODEL");
            env::remove_var("WANJIE_MODEL");
            env::remove_var("WANJIE_MAAS_MODEL");
            env::remove_var("OPENROUTER_API_KEY");
            env::remove_var("OPENROUTER_BASE_URL");
            env::remove_var("OPENROUTER_MODEL");
            env::remove_var("VOLCENGINE_API_KEY");
            env::remove_var("VOLCENGINE_ARK_API_KEY");
            env::remove_var("ARK_API_KEY");
            env::remove_var("VOLCENGINE_BASE_URL");
            env::remove_var("VOLCENGINE_ARK_BASE_URL");
            env::remove_var("ARK_BASE_URL");
            env::remove_var("VOLCENGINE_MODEL");
            env::remove_var("VOLCENGINE_ARK_MODEL");
            env::remove_var("XIAOMI_MIMO_TOKEN_PLAN_API_KEY");
            env::remove_var("MIMO_TOKEN_PLAN_API_KEY");
            env::remove_var("XIAOMI_MIMO_API_KEY");
            env::remove_var("XIAOMI_API_KEY");
            env::remove_var("MIMO_API_KEY");
            env::remove_var("XIAOMI_MIMO_BASE_URL");
            env::remove_var("MIMO_BASE_URL");
            env::remove_var("XIAOMI_MIMO_MODEL");
            env::remove_var("MIMO_MODEL");
            env::remove_var("XIAOMI_MIMO_MODE");
            env::remove_var("MIMO_MODE");
            env::remove_var("NOVITA_API_KEY");
            env::remove_var("NOVITA_BASE_URL");
            env::remove_var("NOVITA_MODEL");
            env::remove_var("FIREWORKS_API_KEY");
            env::remove_var("FIREWORKS_BASE_URL");
            env::remove_var("FIREWORKS_MODEL");
            env::remove_var("SILICONFLOW_API_KEY");
            env::remove_var("SILICONFLOW_BASE_URL");
            env::remove_var("SILICONFLOW_MODEL");
            env::remove_var("ARCEE_API_KEY");
            env::remove_var("ARCEE_BASE_URL");
            env::remove_var("ARCEE_MODEL");
            env::remove_var("MOONSHOT_API_KEY");
            env::remove_var("MOONSHOT_BASE_URL");
            env::remove_var("MOONSHOT_MODEL");
            env::remove_var("KIMI_API_KEY");
            env::remove_var("KIMI_BASE_URL");
            env::remove_var("KIMI_MODEL");
            env::remove_var("KIMI_MODEL_NAME");
            env::remove_var("KIMI_CODE_HOME");
            env::remove_var("KIMI_SHARE_DIR");
            env::remove_var("SGLANG_API_KEY");
            env::remove_var("SGLANG_BASE_URL");
            env::remove_var("SGLANG_MODEL");
            env::remove_var("VLLM_API_KEY");
            env::remove_var("VLLM_BASE_URL");
            env::remove_var("VLLM_MODEL");
            env::remove_var("OLLAMA_CLOUD_API_KEY");
            env::remove_var("OLLAMA_CLOUD_BASE_URL");
            env::remove_var("OLLAMA_CLOUD_MODEL");
            env::remove_var("OLLAMA_API_KEY");
            env::remove_var("OLLAMA_BASE_URL");
            env::remove_var("OLLAMA_MODEL");
            env::remove_var("HUGGINGFACE_API_KEY");
            env::remove_var("HF_TOKEN");
            env::remove_var("HUGGINGFACE_BASE_URL");
            env::remove_var("HF_BASE_URL");
            env::remove_var("HUGGINGFACE_MODEL");
            env::remove_var("HF_MODEL");
        }
        Self {
            _sealed_home: sealed_home,
            _sealed_userprofile: sealed_userprofile,
            _sealed_codewhale_home: sealed_codewhale_home,
            _sealed_codewhale_config_path: sealed_codewhale_config_path,
            _sealed_deepseek_config_path: sealed_deepseek_config_path,
            home: home_prev,
            userprofile: userprofile_prev,
            codewhale_home: codewhale_home_prev,
            codewhale_config_path: codewhale_config_prev,
            deepseek_config_path: deepseek_config_prev,
            codewhale_secret_backend: codewhale_secret_backend_prev,
            deepseek_secret_backend: deepseek_secret_backend_prev,
            deepseek_provider: deepseek_provider_prev,
            deepseek_api_key: api_key_prev,
            deepseek_base_url: base_url_prev,
            deepseek_http_headers: http_headers_prev,
            deepseek_model: model_prev,
            deepseek_default_text_model: default_text_model_prev,
            codewhale_provider: codewhale_provider_prev,
            codewhale_model: codewhale_model_prev,
            codewhale_base_url: codewhale_base_url_prev,
            nvidia_api_key: nvidia_api_key_prev,
            nvidia_nim_api_key: nvidia_nim_api_key_prev,
            nim_base_url: nim_base_url_prev,
            nvidia_base_url: nvidia_base_url_prev,
            nvidia_nim_base_url: nvidia_nim_base_url_prev,
            nvidia_nim_model: nvidia_nim_model_prev,
            openai_api_key: openai_api_key_prev,
            openai_base_url: openai_base_url_prev,
            openai_model: openai_model_prev,
            atlascloud_api_key: atlascloud_api_key_prev,
            atlascloud_base_url: atlascloud_base_url_prev,
            atlascloud_model: atlascloud_model_prev,
            wanjie_ark_api_key: wanjie_ark_api_key_prev,
            wanjie_api_key: wanjie_api_key_prev,
            wanjie_maas_api_key: wanjie_maas_api_key_prev,
            wanjie_ark_base_url: wanjie_ark_base_url_prev,
            wanjie_base_url: wanjie_base_url_prev,
            wanjie_maas_base_url: wanjie_maas_base_url_prev,
            wanjie_ark_model: wanjie_ark_model_prev,
            wanjie_model: wanjie_model_prev,
            wanjie_maas_model: wanjie_maas_model_prev,
            openrouter_api_key: openrouter_api_key_prev,
            openrouter_base_url: openrouter_base_url_prev,
            openrouter_model: openrouter_model_prev,
            volcengine_api_key: volcengine_api_key_prev,
            volcengine_ark_api_key: volcengine_ark_api_key_prev,
            ark_api_key: ark_api_key_prev,
            volcengine_base_url: volcengine_base_url_prev,
            volcengine_ark_base_url: volcengine_ark_base_url_prev,
            ark_base_url: ark_base_url_prev,
            volcengine_model: volcengine_model_prev,
            volcengine_ark_model: volcengine_ark_model_prev,
            xiaomi_mimo_token_plan_api_key: xiaomi_mimo_token_plan_api_key_prev,
            mimo_token_plan_api_key: mimo_token_plan_api_key_prev,
            xiaomi_mimo_api_key: xiaomi_mimo_api_key_prev,
            xiaomi_api_key: xiaomi_api_key_prev,
            mimo_api_key: mimo_api_key_prev,
            xiaomi_mimo_base_url: xiaomi_mimo_base_url_prev,
            mimo_base_url: mimo_base_url_prev,
            xiaomi_mimo_model: xiaomi_mimo_model_prev,
            mimo_model: mimo_model_prev,
            xiaomi_mimo_mode: xiaomi_mimo_mode_prev,
            mimo_mode: mimo_mode_prev,
            novita_api_key: novita_api_key_prev,
            novita_base_url: novita_base_url_prev,
            novita_model: novita_model_prev,
            fireworks_api_key: fireworks_api_key_prev,
            fireworks_base_url: fireworks_base_url_prev,
            fireworks_model: fireworks_model_prev,
            siliconflow_api_key: siliconflow_api_key_prev,
            siliconflow_base_url: siliconflow_base_url_prev,
            siliconflow_model: siliconflow_model_prev,
            arcee_api_key: arcee_api_key_prev,
            arcee_base_url: arcee_base_url_prev,
            arcee_model: arcee_model_prev,
            moonshot_api_key: moonshot_api_key_prev,
            moonshot_base_url: moonshot_base_url_prev,
            moonshot_model: moonshot_model_prev,
            kimi_api_key: kimi_api_key_prev,
            kimi_base_url: kimi_base_url_prev,
            kimi_model: kimi_model_prev,
            kimi_model_name: kimi_model_name_prev,
            kimi_code_home: kimi_code_home_prev,
            kimi_share_dir: kimi_share_dir_prev,
            sglang_api_key: sglang_api_key_prev,
            sglang_base_url: sglang_base_url_prev,
            sglang_model: sglang_model_prev,
            vllm_api_key: vllm_api_key_prev,
            vllm_base_url: vllm_base_url_prev,
            vllm_model: vllm_model_prev,
            ollama_cloud_api_key: ollama_cloud_api_key_prev,
            ollama_cloud_base_url: ollama_cloud_base_url_prev,
            ollama_cloud_model: ollama_cloud_model_prev,
            ollama_api_key: ollama_api_key_prev,
            ollama_base_url: ollama_base_url_prev,
            ollama_model: ollama_model_prev,
            huggingface_api_key: huggingface_api_key_prev,
            huggingface_token: huggingface_token_prev,
            huggingface_base_url: huggingface_base_url_prev,
            hf_base_url: hf_base_url_prev,
            huggingface_model: huggingface_model_prev,
            hf_model: hf_model_prev,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            Self::restore_var("HOME", self.home.take());
            Self::restore_var("USERPROFILE", self.userprofile.take());
            Self::restore_var("CODEWHALE_HOME", self.codewhale_home.take());
            Self::restore_var("CODEWHALE_CONFIG_PATH", self.codewhale_config_path.take());
            Self::restore_var("DEEPSEEK_CONFIG_PATH", self.deepseek_config_path.take());
            Self::restore_var(
                "CODEWHALE_SECRET_BACKEND",
                self.codewhale_secret_backend.take(),
            );
            Self::restore_var(
                "DEEPSEEK_SECRET_BACKEND",
                self.deepseek_secret_backend.take(),
            );
            Self::restore_var("DEEPSEEK_PROVIDER", self.deepseek_provider.take());
            Self::restore_var("DEEPSEEK_API_KEY", self.deepseek_api_key.take());
            Self::restore_var("DEEPSEEK_BASE_URL", self.deepseek_base_url.take());
            Self::restore_var("DEEPSEEK_HTTP_HEADERS", self.deepseek_http_headers.take());
            Self::restore_var("DEEPSEEK_MODEL", self.deepseek_model.take());
            Self::restore_var(
                "DEEPSEEK_DEFAULT_TEXT_MODEL",
                self.deepseek_default_text_model.take(),
            );
            Self::restore_var("CODEWHALE_PROVIDER", self.codewhale_provider.take());
            Self::restore_var("CODEWHALE_MODEL", self.codewhale_model.take());
            Self::restore_var("CODEWHALE_BASE_URL", self.codewhale_base_url.take());
            Self::restore_var("NVIDIA_API_KEY", self.nvidia_api_key.take());
            Self::restore_var("NVIDIA_NIM_API_KEY", self.nvidia_nim_api_key.take());
            Self::restore_var("NIM_BASE_URL", self.nim_base_url.take());
            Self::restore_var("NVIDIA_BASE_URL", self.nvidia_base_url.take());
            Self::restore_var("NVIDIA_NIM_BASE_URL", self.nvidia_nim_base_url.take());
            Self::restore_var("NVIDIA_NIM_MODEL", self.nvidia_nim_model.take());
            Self::restore_var("OPENAI_API_KEY", self.openai_api_key.take());
            Self::restore_var("OPENAI_BASE_URL", self.openai_base_url.take());
            Self::restore_var("OPENAI_MODEL", self.openai_model.take());
            Self::restore_var("ATLASCLOUD_API_KEY", self.atlascloud_api_key.take());
            Self::restore_var("ATLASCLOUD_BASE_URL", self.atlascloud_base_url.take());
            Self::restore_var("ATLASCLOUD_MODEL", self.atlascloud_model.take());
            Self::restore_var("WANJIE_ARK_API_KEY", self.wanjie_ark_api_key.take());
            Self::restore_var("WANJIE_API_KEY", self.wanjie_api_key.take());
            Self::restore_var("WANJIE_MAAS_API_KEY", self.wanjie_maas_api_key.take());
            Self::restore_var("WANJIE_ARK_BASE_URL", self.wanjie_ark_base_url.take());
            Self::restore_var("WANJIE_BASE_URL", self.wanjie_base_url.take());
            Self::restore_var("WANJIE_MAAS_BASE_URL", self.wanjie_maas_base_url.take());
            Self::restore_var("WANJIE_ARK_MODEL", self.wanjie_ark_model.take());
            Self::restore_var("WANJIE_MODEL", self.wanjie_model.take());
            Self::restore_var("WANJIE_MAAS_MODEL", self.wanjie_maas_model.take());
            Self::restore_var("OPENROUTER_API_KEY", self.openrouter_api_key.take());
            Self::restore_var("OPENROUTER_BASE_URL", self.openrouter_base_url.take());
            Self::restore_var("OPENROUTER_MODEL", self.openrouter_model.take());
            Self::restore_var("VOLCENGINE_API_KEY", self.volcengine_api_key.take());
            Self::restore_var("VOLCENGINE_ARK_API_KEY", self.volcengine_ark_api_key.take());
            Self::restore_var("ARK_API_KEY", self.ark_api_key.take());
            Self::restore_var("VOLCENGINE_BASE_URL", self.volcengine_base_url.take());
            Self::restore_var(
                "VOLCENGINE_ARK_BASE_URL",
                self.volcengine_ark_base_url.take(),
            );
            Self::restore_var("ARK_BASE_URL", self.ark_base_url.take());
            Self::restore_var("VOLCENGINE_MODEL", self.volcengine_model.take());
            Self::restore_var("VOLCENGINE_ARK_MODEL", self.volcengine_ark_model.take());
            Self::restore_var(
                "XIAOMI_MIMO_TOKEN_PLAN_API_KEY",
                self.xiaomi_mimo_token_plan_api_key.take(),
            );
            Self::restore_var(
                "MIMO_TOKEN_PLAN_API_KEY",
                self.mimo_token_plan_api_key.take(),
            );
            Self::restore_var("XIAOMI_MIMO_API_KEY", self.xiaomi_mimo_api_key.take());
            Self::restore_var("XIAOMI_API_KEY", self.xiaomi_api_key.take());
            Self::restore_var("MIMO_API_KEY", self.mimo_api_key.take());
            Self::restore_var("XIAOMI_MIMO_BASE_URL", self.xiaomi_mimo_base_url.take());
            Self::restore_var("MIMO_BASE_URL", self.mimo_base_url.take());
            Self::restore_var("XIAOMI_MIMO_MODEL", self.xiaomi_mimo_model.take());
            Self::restore_var("MIMO_MODEL", self.mimo_model.take());
            Self::restore_var("XIAOMI_MIMO_MODE", self.xiaomi_mimo_mode.take());
            Self::restore_var("MIMO_MODE", self.mimo_mode.take());
            Self::restore_var("NOVITA_API_KEY", self.novita_api_key.take());
            Self::restore_var("NOVITA_BASE_URL", self.novita_base_url.take());
            Self::restore_var("NOVITA_MODEL", self.novita_model.take());
            Self::restore_var("FIREWORKS_API_KEY", self.fireworks_api_key.take());
            Self::restore_var("FIREWORKS_BASE_URL", self.fireworks_base_url.take());
            Self::restore_var("FIREWORKS_MODEL", self.fireworks_model.take());
            Self::restore_var("SILICONFLOW_API_KEY", self.siliconflow_api_key.take());
            Self::restore_var("SILICONFLOW_BASE_URL", self.siliconflow_base_url.take());
            Self::restore_var("SILICONFLOW_MODEL", self.siliconflow_model.take());
            Self::restore_var("ARCEE_API_KEY", self.arcee_api_key.take());
            Self::restore_var("ARCEE_BASE_URL", self.arcee_base_url.take());
            Self::restore_var("ARCEE_MODEL", self.arcee_model.take());
            Self::restore_var("MOONSHOT_API_KEY", self.moonshot_api_key.take());
            Self::restore_var("MOONSHOT_BASE_URL", self.moonshot_base_url.take());
            Self::restore_var("MOONSHOT_MODEL", self.moonshot_model.take());
            Self::restore_var("KIMI_API_KEY", self.kimi_api_key.take());
            Self::restore_var("KIMI_BASE_URL", self.kimi_base_url.take());
            Self::restore_var("KIMI_MODEL", self.kimi_model.take());
            Self::restore_var("KIMI_MODEL_NAME", self.kimi_model_name.take());
            Self::restore_var("KIMI_CODE_HOME", self.kimi_code_home.take());
            Self::restore_var("KIMI_SHARE_DIR", self.kimi_share_dir.take());
            Self::restore_var("SGLANG_API_KEY", self.sglang_api_key.take());
            Self::restore_var("SGLANG_BASE_URL", self.sglang_base_url.take());
            Self::restore_var("SGLANG_MODEL", self.sglang_model.take());
            Self::restore_var("VLLM_API_KEY", self.vllm_api_key.take());
            Self::restore_var("VLLM_BASE_URL", self.vllm_base_url.take());
            Self::restore_var("VLLM_MODEL", self.vllm_model.take());
            Self::restore_var("OLLAMA_CLOUD_API_KEY", self.ollama_cloud_api_key.take());
            Self::restore_var("OLLAMA_CLOUD_BASE_URL", self.ollama_cloud_base_url.take());
            Self::restore_var("OLLAMA_CLOUD_MODEL", self.ollama_cloud_model.take());
            Self::restore_var("OLLAMA_API_KEY", self.ollama_api_key.take());
            Self::restore_var("OLLAMA_BASE_URL", self.ollama_base_url.take());
            Self::restore_var("OLLAMA_MODEL", self.ollama_model.take());
            Self::restore_var("HUGGINGFACE_API_KEY", self.huggingface_api_key.take());
            Self::restore_var("HF_TOKEN", self.huggingface_token.take());
            Self::restore_var("HUGGINGFACE_BASE_URL", self.huggingface_base_url.take());
            Self::restore_var("HF_BASE_URL", self.hf_base_url.take());
            Self::restore_var("HUGGINGFACE_MODEL", self.huggingface_model.take());
            Self::restore_var("HF_MODEL", self.hf_model.take());
        }
    }
}

impl EnvGuard {
    /// Restore an env var to its prior value (or remove it if it was unset).
    ///
    /// # Safety
    /// Must only be called from test code guarded by a global mutex.
    unsafe fn restore_var(key: &str, prev: Option<OsString>) {
        if let Some(value) = prev {
            unsafe { env::set_var(key, value) };
        } else {
            unsafe { env::remove_var(key) };
        }
    }
}

#[test]
fn max_subagents_scenario() {
    // Scenario consolidation of: max_subagents_defaults_to_default_limit, max_subagents_clamps_subagents_max_concurrent
    // from max_subagents_defaults_to_default_limit
    {
        assert_eq!(Config::default().max_subagents(), DEFAULT_MAX_SUBAGENTS);
        assert_eq!(DEFAULT_MAX_SUBAGENTS, 64);
    }
    // from max_subagents_clamps_subagents_max_concurrent
    {
        let low = Config {
            subagents: Some(SubagentsConfig {
                max_concurrent: Some(0),
                ..SubagentsConfig::default()
            }),
            ..Config::default()
        };
        assert_eq!(low.max_subagents(), 1);

        let high = Config {
            subagents: Some(SubagentsConfig {
                max_concurrent: Some(MAX_SUBAGENTS + 10),
                ..SubagentsConfig::default()
            }),
            ..Config::default()
        };
        assert_eq!(high.max_subagents(), MAX_SUBAGENTS);
    }
}

#[test]
fn launch_concurrency_defaults_and_clamps_to_max_subagents() {
    // Unset launch_concurrency now defaults to the full resolved cap.
    assert_eq!(
        Config::default().launch_concurrency(),
        Config::default().max_subagents()
    );

    let mut config = Config {
        subagents: Some(SubagentsConfig {
            launch_concurrency: Some(50),
            ..SubagentsConfig::default()
        }),
        ..Config::default()
    };
    assert_eq!(config.launch_concurrency(), 50);

    config.subagents = Some(SubagentsConfig {
        launch_concurrency: Some(DEFAULT_MAX_SUBAGENTS + 10),
        ..SubagentsConfig::default()
    });
    assert_eq!(config.launch_concurrency(), config.max_subagents());

    config.subagents = Some(SubagentsConfig {
        launch_concurrency: Some(0),
        ..SubagentsConfig::default()
    });
    assert_eq!(config.launch_concurrency(), 1);

    config.subagents = Some(SubagentsConfig {
        launch_concurrency: Some(2),
        ..SubagentsConfig::default()
    });
    assert_eq!(config.launch_concurrency(), 2);
}

#[test]
fn subagent_budget_defaults_read_the_subagents_table() {
    // #5324: per-child step/wall-time defaults are operator config
    // (`[subagents]`), not per-call schema fields. `0` means unset (keep the
    // role / 1800s defaults).
    let cfg: SubagentsConfig =
        toml::from_str("default_max_steps = 90\ndefault_wall_time_secs = 600")
            .expect("parse [subagents] budget keys");
    assert_eq!(cfg.default_max_steps, Some(90));
    assert_eq!(cfg.default_wall_time_secs, Some(600));

    let mut config = Config {
        subagents: Some(SubagentsConfig {
            default_max_steps: Some(240),
            default_wall_time_secs: Some(2700),
            ..SubagentsConfig::default()
        }),
        ..Config::default()
    };
    assert_eq!(config.subagent_default_max_steps(), Some(240));
    assert_eq!(config.subagent_default_wall_time_secs(), Some(2700));

    config.subagents = Some(SubagentsConfig {
        default_max_steps: Some(0),
        default_wall_time_secs: Some(0),
        ..SubagentsConfig::default()
    });
    assert_eq!(config.subagent_default_max_steps(), None);
    assert_eq!(config.subagent_default_wall_time_secs(), None);

    assert_eq!(Config::default().subagent_default_max_steps(), None);
    assert_eq!(Config::default().subagent_default_wall_time_secs(), None);
}

#[test]
fn launch_concurrency_scenario() {
    // Scenario consolidation of: launch_concurrency_honors_deprecated_interactive_max_launch_alias, launch_concurrency_new_key_wins_over_deprecated_alias
    // from launch_concurrency_honors_deprecated_interactive_max_launch_alias
    {
        // The old TOML key `interactive_max_launch` still deserializes, via
        // #[serde(rename)], into the hidden legacy field, and the resolver
        // honors it when the new key is unset.
        let cfg: SubagentsConfig =
            toml::from_str("interactive_max_launch = 5").expect("parse legacy key");
        assert_eq!(cfg.interactive_max_launch_legacy, Some(5));
        assert_eq!(cfg.launch_concurrency, None);

        let config = Config {
            subagents: Some(cfg),
            ..Config::default()
        };
        assert_eq!(config.launch_concurrency(), 5);
    }
    // from launch_concurrency_new_key_wins_over_deprecated_alias
    {
        // When both keys are present the new `launch_concurrency` wins
        // deterministically, regardless of document order.
        let cfg: SubagentsConfig =
            toml::from_str("launch_concurrency = 3\ninteractive_max_launch = 7")
                .expect("parse both keys");
        assert_eq!(cfg.launch_concurrency, Some(3));
        assert_eq!(cfg.interactive_max_launch_legacy, Some(7));

        let config = Config {
            subagents: Some(cfg),
            ..Config::default()
        };
        assert_eq!(config.launch_concurrency(), 3);
    }
}

#[test]
fn fleet_role_model_keys_accept_canonical_and_legacy_names() {
    let canonical: Config = toml::from_str(
        r#"
[subagents]
scout_model = "scout-model"
planner_model = "planner-model"
reviewer_model = "reviewer-model"
"#,
    )
    .expect("parse canonical Fleet role keys");
    let overrides = canonical.subagent_model_overrides();
    assert_eq!(
        overrides.get("scout").map(String::as_str),
        Some("scout-model")
    );
    assert_eq!(
        overrides.get("planner").map(String::as_str),
        Some("planner-model")
    );
    assert_eq!(
        overrides.get("reviewer").map(String::as_str),
        Some("reviewer-model")
    );

    let legacy: Config = toml::from_str(
        r#"
[subagents]
explorer_model = "legacy-scout"
awaiter_model = "legacy-planner"
review_model = "legacy-reviewer"
"#,
    )
    .expect("parse v0.9.x role aliases");
    let overrides = legacy.subagent_model_overrides();
    assert_eq!(
        overrides.get("scout").map(String::as_str),
        Some("legacy-scout")
    );
    assert_eq!(
        overrides.get("planner").map(String::as_str),
        Some("legacy-planner")
    );
    assert_eq!(
        overrides.get("reviewer").map(String::as_str),
        Some("legacy-reviewer")
    );
}

#[test]
fn subagent_token_budget_is_optional_and_zero_disables() {
    assert_eq!(Config::default().subagent_token_budget(), None);

    let disabled = Config {
        subagents: Some(SubagentsConfig {
            token_budget: Some(0),
            ..SubagentsConfig::default()
        }),
        ..Config::default()
    };
    assert_eq!(disabled.subagent_token_budget(), None);

    let configured = Config {
        subagents: Some(SubagentsConfig {
            token_budget: Some(50_000),
            ..SubagentsConfig::default()
        }),
        ..Config::default()
    };
    assert_eq!(configured.subagent_token_budget(), Some(50_000));
}

#[test]
fn subagent_admission_limit_defaults_and_clamps() {
    assert_eq!(
        Config::default().max_admitted_subagents(),
        MAX_SUBAGENT_ADMISSION
    );

    let configured = Config {
        subagents: Some(SubagentsConfig {
            max_concurrent: Some(4),
            max_admitted: Some(80),
            ..SubagentsConfig::default()
        }),
        ..Config::default()
    };
    assert_eq!(configured.max_subagents(), 4);
    assert_eq!(configured.max_admitted_subagents(), 80);

    let low = Config {
        subagents: Some(SubagentsConfig {
            max_concurrent: Some(4),
            max_admitted: Some(1),
            ..SubagentsConfig::default()
        }),
        ..Config::default()
    };
    assert_eq!(low.max_admitted_subagents(), 4);

    let high = Config {
        subagents: Some(SubagentsConfig {
            max_admitted: Some(MAX_SUBAGENT_ADMISSION + 1),
            ..SubagentsConfig::default()
        }),
        ..Config::default()
    };
    assert_eq!(high.max_admitted_subagents(), MAX_SUBAGENT_ADMISSION);

    let alias_cfg: SubagentsConfig =
        toml::from_str("admission_limit = 80").expect("parse admission alias");
    assert_eq!(alias_cfg.max_admitted, Some(80));
}

#[test]
fn provider_subagent_profiles_override_global_limits_with_aliases() {
    let config: Config = toml::from_str(
        r#"
provider = "zai"

[subagents]
max_concurrent = 20
launch_concurrency = 20
max_admitted = 200
max_depth = 6
token_budget = 100000
api_timeout_secs = 900
heartbeat_timeout_secs = 1200

[subagents.providers.glm]
max_concurrent = 4
launch_concurrency = 3
max_admitted = 12
max_depth = 2
token_budget = 25000
api_timeout_secs = 180
heartbeat_timeout_secs = 240
"#,
    )
    .expect("parse provider subagent profile");

    assert_eq!(config.api_provider(), ApiProvider::Zai);
    assert_eq!(config.max_subagents(), 20);
    assert_eq!(config.max_subagents_for_provider(ApiProvider::Zai), 4);
    assert_eq!(config.launch_concurrency_for_provider(ApiProvider::Zai), 3);
    assert_eq!(
        config.max_admitted_subagents_for_provider(ApiProvider::Zai),
        12
    );
    assert_eq!(
        config.subagent_max_spawn_depth_for_provider(ApiProvider::Zai),
        2
    );
    assert_eq!(
        config.subagent_token_budget_for_provider(ApiProvider::Zai),
        Some(25_000)
    );
    assert_eq!(
        config.subagent_api_timeout_secs_for_provider(ApiProvider::Zai),
        180
    );
    // The explicit 240s provider override sits below the 300s tool timeout,
    // and a heartbeat under the tool timeout kills children mid-legitimate-
    // tool (activity is only recorded at step boundaries). The tool-timeout
    // floor lifts the resolved value above the override
    // (2026-08-04 sub-agent hunt, finding 4).
    assert_eq!(
        config.subagent_heartbeat_timeout_secs_for_provider(ApiProvider::Zai),
        DEFAULT_SUBAGENT_TOOL_TIMEOUT_SECS + 30
    );
}

#[test]
fn provider_request_concurrency_defaults_to_zai_and_can_be_overridden() {
    let default_zai: Config = toml::from_str(
        r#"
provider = "zai"
"#,
    )
    .expect("parse zai provider config");
    assert_eq!(
        default_zai.provider_max_concurrency(ApiProvider::Zai),
        Some(DEFAULT_ZAI_PROVIDER_MAX_CONCURRENCY)
    );
    assert_eq!(
        default_zai.provider_max_concurrency(ApiProvider::Deepseek),
        None
    );

    let configured: Config = toml::from_str(
        r#"
provider = "zai"

[providers.zhipu]
max-concurrency = 10
"#,
    )
    .expect("parse zhipu concurrency alias");
    assert_eq!(
        configured.provider_max_concurrency(ApiProvider::Zai),
        Some(10)
    );

    let disabled: Config = toml::from_str(
        r#"
provider = "zai"

[providers.zai]
maxConcurrency = 0
"#,
    )
    .expect("parse disabled concurrency cap");
    assert_eq!(disabled.provider_max_concurrency(ApiProvider::Zai), None);

    let clamped: Config = toml::from_str(
        r#"
[providers.openai]
concurrency = 999
"#,
    )
    .expect("parse openai concurrency alias");
    assert_eq!(
        clamped.provider_max_concurrency(ApiProvider::Openai),
        Some(MAX_PROVIDER_REQUEST_CONCURRENCY)
    );
}

#[test]
fn provider_subagent_profiles_inherit_and_clamp_against_provider_max() {
    let config: Config = toml::from_str(
        r#"
[subagents]
max_concurrent = 12
launch_concurrency = 8
max_depth = 5
api_timeout_secs = 300

[subagents.providers.deepseek_api]
max_concurrent = 30
launch_concurrency = 30
max_admitted = 1

[subagents.providers.anthropic]
enabled = false
"#,
    )
    .expect("parse inherited provider subagent profile");

    assert_eq!(config.max_subagents_for_provider(ApiProvider::Deepseek), 30);
    assert_eq!(
        config.launch_concurrency_for_provider(ApiProvider::Deepseek),
        30
    );
    assert_eq!(
        config.max_admitted_subagents_for_provider(ApiProvider::Deepseek),
        30
    );
    assert_eq!(
        config.subagent_max_spawn_depth_for_provider(ApiProvider::Deepseek),
        5
    );
    assert_eq!(
        config.subagent_api_timeout_secs_for_provider(ApiProvider::Deepseek),
        300
    );
    assert!(config.subagents_enabled_for_provider(ApiProvider::Deepseek));
    assert!(!config.subagents_enabled_for_provider(ApiProvider::Anthropic));
}

#[test]
fn subagents_max_concurrent_overrides_top_level_cap() {
    let config = Config {
        max_subagents: Some(3),
        subagents: Some(SubagentsConfig {
            max_concurrent: Some(12),
            ..SubagentsConfig::default()
        }),
        ..Config::default()
    };

    assert_eq!(config.max_subagents(), 12);
}

#[test]
fn subagents_enabled_reports_disable_precedence() {
    assert!(Config::default().subagents_enabled());

    let mut feature_disabled = Config::default();
    feature_disabled
        .set_feature("subagents", false)
        .expect("known feature");
    assert!(!feature_disabled.subagents_enabled());
    assert_eq!(
        feature_disabled.subagents_disabled_reason(),
        Some("features.subagents=false")
    );

    let explicit_disabled = Config {
        subagents: Some(SubagentsConfig {
            enabled: Some(false),
            max_concurrent: Some(0),
            max_depth: Some(0),
            ..SubagentsConfig::default()
        }),
        ..Config::default()
    };
    assert!(!explicit_disabled.subagents_enabled());
    assert_eq!(
        explicit_disabled.subagents_disabled_reason(),
        Some("subagents.enabled=false")
    );

    let zero_concurrency = Config {
        subagents: Some(SubagentsConfig {
            enabled: Some(true),
            max_concurrent: Some(0),
            max_depth: Some(1),
            ..SubagentsConfig::default()
        }),
        ..Config::default()
    };
    assert_eq!(
        zero_concurrency.subagents_disabled_reason(),
        Some("subagents.max_concurrent=0")
    );

    let zero_depth = Config {
        subagents: Some(SubagentsConfig {
            enabled: Some(true),
            max_concurrent: Some(1),
            max_depth: Some(0),
            ..SubagentsConfig::default()
        }),
        ..Config::default()
    };
    assert_eq!(
        zero_depth.subagents_disabled_reason(),
        Some("subagents.max_depth=0")
    );
}

#[test]
fn subagent_max_spawn_depth_defaults_allows_zero_and_clamps() {
    assert_eq!(
        Config::default().subagent_max_spawn_depth(),
        codewhale_config::DEFAULT_SPAWN_DEPTH
    );

    let disabled = Config {
        subagents: Some(SubagentsConfig {
            max_depth: Some(0),
            ..SubagentsConfig::default()
        }),
        ..Config::default()
    };
    assert_eq!(disabled.subagent_max_spawn_depth(), 0);

    let high = Config {
        subagents: Some(SubagentsConfig {
            max_depth: Some(codewhale_config::MAX_SPAWN_DEPTH_CEILING + 10),
            ..SubagentsConfig::default()
        }),
        ..Config::default()
    };
    assert_eq!(
        high.subagent_max_spawn_depth(),
        codewhale_config::MAX_SPAWN_DEPTH_CEILING
    );
}

#[test]
fn subagent_api_timeout_defaults_and_clamps() {
    assert_eq!(
        Config::default().subagent_api_timeout_secs(),
        DEFAULT_SUBAGENT_API_TIMEOUT_SECS
    );
    assert_eq!(DEFAULT_SUBAGENT_API_TIMEOUT_SECS, 600);

    let zero = Config {
        subagents: Some(SubagentsConfig {
            api_timeout_secs: Some(0),
            ..SubagentsConfig::default()
        }),
        ..Config::default()
    };
    assert_eq!(
        zero.subagent_api_timeout_secs(),
        DEFAULT_SUBAGENT_API_TIMEOUT_SECS
    );

    let explicit_min = Config {
        subagents: Some(SubagentsConfig {
            api_timeout_secs: Some(MIN_SUBAGENT_API_TIMEOUT_SECS),
            ..SubagentsConfig::default()
        }),
        ..Config::default()
    };
    assert_eq!(explicit_min.subagent_api_timeout_secs(), 1);

    let explicit_max = Config {
        subagents: Some(SubagentsConfig {
            api_timeout_secs: Some(3600),
            ..SubagentsConfig::default()
        }),
        ..Config::default()
    };
    assert_eq!(explicit_max.subagent_api_timeout_secs(), 3600);

    let beyond_max = Config {
        subagents: Some(SubagentsConfig {
            api_timeout_secs: Some(3601),
            ..SubagentsConfig::default()
        }),
        ..Config::default()
    };
    assert_eq!(beyond_max.subagent_api_timeout_secs(), 3600);

    let high = Config {
        subagents: Some(SubagentsConfig {
            api_timeout_secs: Some(MAX_SUBAGENT_API_TIMEOUT_SECS + 60),
            ..SubagentsConfig::default()
        }),
        ..Config::default()
    };
    assert_eq!(
        high.subagent_api_timeout_secs(),
        MAX_SUBAGENT_API_TIMEOUT_SECS
    );
}

#[test]
fn subagent_heartbeat_timeout_defaults_clamps_and_respects_api_timeout() {
    // With the 600s default API timeout, the heartbeat floor (api + 30s)
    // lifts the resolved default above the raw 300s constant. The tool
    // timeout floor (tool + 30s) also participates but sits below the API
    // floor at default settings.
    let resolved_default = DEFAULT_SUBAGENT_HEARTBEAT_TIMEOUT_SECS
        .max(DEFAULT_SUBAGENT_API_TIMEOUT_SECS + 30)
        .max(DEFAULT_SUBAGENT_TOOL_TIMEOUT_SECS + 30);
    assert_eq!(
        Config::default().subagent_heartbeat_timeout_secs(),
        resolved_default
    );

    let zero = Config {
        subagents: Some(SubagentsConfig {
            heartbeat_timeout_secs: Some(0),
            ..SubagentsConfig::default()
        }),
        ..Config::default()
    };
    assert_eq!(zero.subagent_heartbeat_timeout_secs(), resolved_default);

    // With a tiny API timeout the tool-timeout floor dominates: a single
    // tool execution can run the full tool timeout without touching the
    // heartbeat, so cleanup must not fire before tool_timeout + 30s even
    // though the API floor alone would be 31s (2026-08-04 sub-agent hunt,
    // finding 4 — this case resolved to 31 before the fix, which let
    // cleanup kill a child mid-legitimate-tool).
    let low = Config {
        subagents: Some(SubagentsConfig {
            api_timeout_secs: Some(1),
            heartbeat_timeout_secs: Some(1),
            ..SubagentsConfig::default()
        }),
        ..Config::default()
    };
    assert_eq!(
        low.subagent_heartbeat_timeout_secs(),
        DEFAULT_SUBAGENT_TOOL_TIMEOUT_SECS + 30
    );

    let follows_long_api_timeout = Config {
        subagents: Some(SubagentsConfig {
            api_timeout_secs: Some(900),
            heartbeat_timeout_secs: Some(300),
            ..SubagentsConfig::default()
        }),
        ..Config::default()
    };
    assert_eq!(
        follows_long_api_timeout.subagent_heartbeat_timeout_secs(),
        930
    );

    let high = Config {
        subagents: Some(SubagentsConfig {
            heartbeat_timeout_secs: Some(MAX_SUBAGENT_HEARTBEAT_TIMEOUT_SECS + 60),
            ..SubagentsConfig::default()
        }),
        ..Config::default()
    };
    assert_eq!(
        high.subagent_heartbeat_timeout_secs(),
        MAX_SUBAGENT_HEARTBEAT_TIMEOUT_SECS
    );
}

#[test]
fn subagent_heartbeat_floor_never_drops_below_tool_timeout_plus_margin() {
    // The safety property behind finding 4: for EVERY accepted combination of
    // `[subagents] api_timeout_secs` and `heartbeat_timeout_secs`, the
    // resolved heartbeat timeout stays above the tool timeout, because a tool
    // running up to `tool_timeout` produces no heartbeat activity. Corners
    // cover the smallest legal API timeout and heartbeat against both the
    // global and provider-specific resolvers.
    let corners: [Option<u64>; 4] = [
        Some(MIN_SUBAGENT_API_TIMEOUT_SECS),
        Some(1),
        None,
        Some(MAX_SUBAGENT_API_TIMEOUT_SECS),
    ];
    let heartbeats: [Option<u64>; 4] = [
        Some(MIN_SUBAGENT_HEARTBEAT_TIMEOUT_SECS),
        Some(1),
        None,
        Some(0),
    ];
    for api in corners {
        for heartbeat in heartbeats {
            let cfg = Config {
                subagents: Some(SubagentsConfig {
                    api_timeout_secs: api,
                    heartbeat_timeout_secs: heartbeat,
                    ..SubagentsConfig::default()
                }),
                ..Config::default()
            };
            let floor = DEFAULT_SUBAGENT_TOOL_TIMEOUT_SECS + 30;
            assert!(
                cfg.subagent_heartbeat_timeout_secs() >= floor,
                "global resolver: api={api:?} heartbeat={heartbeat:?} resolved {} < {floor}",
                cfg.subagent_heartbeat_timeout_secs()
            );
            assert!(
                cfg.subagent_heartbeat_timeout_secs_for_provider(ApiProvider::Deepseek) >= floor,
                "provider resolver: api={api:?} heartbeat={heartbeat:?} resolved {} < {floor}",
                cfg.subagent_heartbeat_timeout_secs_for_provider(ApiProvider::Deepseek)
            );
        }
    }
}

#[test]
fn tui_stream_chunk_timeout_defaults_env_and_clamps() {
    let _lock = lock_test_env();
    let previous = env::var_os(STREAM_CHUNK_TIMEOUT_ENV);
    unsafe {
        env::remove_var(STREAM_CHUNK_TIMEOUT_ENV);
    }

    assert_eq!(
        Config::default().stream_chunk_timeout_secs(),
        DEFAULT_STREAM_CHUNK_TIMEOUT_SECS
    );

    let zero = Config {
        tui: Some(TuiConfig {
            stream_chunk_timeout_secs: Some(0),
            max_model_steps: None,
            turn_wall_clock_secs: None,
            stream_max_content_mb: None,
            stream_max_duration_secs: None,
            ..TuiConfig::default()
        }),
        ..Config::default()
    };
    assert_eq!(
        zero.stream_chunk_timeout_secs(),
        DEFAULT_STREAM_CHUNK_TIMEOUT_SECS
    );

    let explicit_min = Config {
        tui: Some(TuiConfig {
            stream_chunk_timeout_secs: Some(MIN_STREAM_CHUNK_TIMEOUT_SECS),
            max_model_steps: None,
            turn_wall_clock_secs: None,
            stream_max_content_mb: None,
            stream_max_duration_secs: None,
            ..TuiConfig::default()
        }),
        ..Config::default()
    };
    assert_eq!(
        explicit_min.stream_chunk_timeout_secs(),
        MIN_STREAM_CHUNK_TIMEOUT_SECS
    );

    let high = Config {
        tui: Some(TuiConfig {
            stream_chunk_timeout_secs: Some(MAX_STREAM_CHUNK_TIMEOUT_SECS + 1),
            max_model_steps: None,
            turn_wall_clock_secs: None,
            stream_max_content_mb: None,
            stream_max_duration_secs: None,
            ..TuiConfig::default()
        }),
        ..Config::default()
    };
    assert_eq!(
        high.stream_chunk_timeout_secs(),
        MAX_STREAM_CHUNK_TIMEOUT_SECS
    );

    unsafe {
        env::set_var(STREAM_CHUNK_TIMEOUT_ENV, "123");
    }
    assert_eq!(Config::default().stream_chunk_timeout_secs(), 123);

    unsafe {
        env::set_var(STREAM_CHUNK_TIMEOUT_ENV, "0");
    }
    assert_eq!(
        Config::default().stream_chunk_timeout_secs(),
        DEFAULT_STREAM_CHUNK_TIMEOUT_SECS
    );

    unsafe {
        match previous {
            Some(value) => env::set_var(STREAM_CHUNK_TIMEOUT_ENV, value),
            None => env::remove_var(STREAM_CHUNK_TIMEOUT_ENV),
        }
    }
}

#[test]
fn save_api_key_writes_config_file_under_cfg_test() -> Result<()> {
    // `save_api_key` writes to the shared user config file. This
    // pins the boring v0.8.8 setup path and avoids platform
    // credential prompts during onboarding.
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let saved = save_api_key("test-key")?;
    let expected = temp_root.join(".deepseek").join("config.toml");
    assert_eq!(saved, SavedCredential::ConfigFile(expected.clone()));
    assert_eq!(saved.describe(), expected.display().to_string());

    let contents = fs::read_to_string(&expected)?;
    assert!(contents.contains("api_key = \""));

    #[cfg(unix)]
    {
        assert_eq!(fs::metadata(&expected)?.permissions().mode() & 0o777, 0o600);
        let parent = expected.parent().expect("config has parent dir");
        assert_eq!(fs::metadata(parent)?.permissions().mode() & 0o077, 0);

        fs::set_permissions(&expected, fs::Permissions::from_mode(0o644))?;
        save_api_key("second-test-key")?;
        assert_eq!(fs::metadata(&expected)?.permissions().mode() & 0o777, 0o600);
    }
    Ok(())
}

#[test]
fn policy_control_waits_for_foreign_test_env_overrides_to_restore() {
    // This is a deadlock ceiling, not a latency requirement. In the full
    // library suite, hundreds of environment-sensitive tests can acquire the
    // process-wide lock before this deliberately blocked reader resumes.
    const FULL_SUITE_SCHEDULING_TIMEOUT: Duration = Duration::from_secs(30);

    let temp = tempfile::tempdir().expect("tempdir");
    let config_path = temp.path().join("config.toml");
    let workspace = temp.path().join("workspace");
    let managed_config_path = temp.path().join("missing-managed.toml");
    let requirements_path = temp.path().join("missing-requirements.toml");
    let (started_tx, started_rx) = mpsc::channel();
    let (tx, rx) = mpsc::channel();

    let reader = {
        let lock = lock_test_env();
        let shell_override = EnvVarGuard::set("CODEWHALE_ALLOW_SHELL", "false");
        let approval_override = EnvVarGuard::set("DEEPSEEK_APPROVAL_POLICY", "never");
        let reader = std::thread::spawn(move || {
            started_tx.send(()).expect("signal policy read start");
            let config = Config {
                managed_config_path: Some(managed_config_path.display().to_string()),
                requirements_path: Some(requirements_path.display().to_string()),
                ..Config::default()
            };
            tx.send((
                config.allow_shell_control(Some(&config_path), None, &workspace),
                config.approval_policy_control(Some(&config_path), None, &workspace),
            ))
            .expect("send policy controls");
        });

        started_rx
            .recv_timeout(FULL_SUITE_SCHEDULING_TIMEOUT)
            .expect("reader reached policy read");
        assert!(
            rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "a foreign reader observed another test's temporary policy overrides"
        );
        drop(approval_override);
        drop(shell_override);
        drop(lock);
        reader
    };

    let (shell, approval) = rx
        .recv_timeout(FULL_SUITE_SCHEDULING_TIMEOUT)
        .expect("reader resumed after policy overrides were restored");
    reader.join().expect("reader thread");
    assert_eq!(shell, ShellAccessControl::Unset);
    assert_eq!(approval, ApprovalPolicyControl::Unset);
}

#[test]
fn base_url_reads_wait_for_foreign_test_env_overrides_to_restore() {
    let (started_tx, started_rx) = mpsc::channel();
    let (tx, rx) = mpsc::channel();

    let reader = {
        let lock = lock_test_env();
        let expected_after_restore = env_base_url_override();
        let override_guard =
            EnvVarGuard::set("CODEWHALE_BASE_URL", "https://temporary.test.invalid/v1");
        let reader = std::thread::spawn(move || {
            started_tx.send(()).expect("signal base URL read start");
            tx.send(env_base_url_override())
                .expect("send resolved base URL override");
        });

        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("reader reached base URL read");
        assert!(
            rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "a foreign reader observed another test's temporary base URL override"
        );
        drop(override_guard);
        drop(lock);
        (reader, expected_after_restore)
    };

    let observed = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("reader resumed after base URL override was restored");
    reader.0.join().expect("reader thread");
    assert_eq!(observed, reader.1);
}

#[test]
fn save_api_scenario() -> Result<()> {
    // Scenario consolidation of: save_api_key_onboarding_routes_openrouter_key_to_provider_table, save_api_key_rejects_empty_input, save_api_key_for_openai_codex_refuses_config_storage
    // from save_api_key_onboarding_routes_openrouter_key_to_provider_table
    {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-onboarding-provider-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let path = save_api_key_for(ApiProvider::Openrouter, "onboarding-openrouter-key")?;
        let contents = fs::read_to_string(&path)?;
        assert!(
            contents.contains("openrouter"),
            "expected OpenRouter provider table, got: {contents}"
        );
        assert!(contents.contains("onboarding-openrouter-key"));
    }
    // from save_api_key_rejects_empty_input
    {
        let _lock = lock_test_env();
        let err = save_api_key("   ").expect_err("empty should bail");
        assert!(
            err.to_string().contains("empty"),
            "expected error to mention empty, got: {err}"
        );
    }
    // from save_api_key_for_openai_codex_refuses_config_storage
    {
        let err = save_api_key_for(ApiProvider::OpenaiCodex, "codex-token")
            .expect_err("Codex OAuth tokens must not be persisted as provider API keys");

        let message = err.to_string();
        assert!(message.contains("OpenAI Codex uses OAuth"), "{message}");
        assert!(message.contains("codex login"), "{message}");
    }
    Ok(())
}

#[test]
fn ensure_config_file_exists_creates_first_run_template() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-first-run-config-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let created = ensure_config_file_exists(None)?.expect("should create config");
    let content = fs::read_to_string(&created)?;

    assert_eq!(created, temp_root.join(".deepseek").join("config.toml"));
    assert!(content.contains("default_text_model = \"deepseek-v4-pro\""));
    assert!(content.contains("reasoning_effort = \"auto\""));
    assert!(!content.contains("api_key ="));
    assert!(ensure_config_file_exists(None)?.is_none());
    Ok(())
}

#[test]
fn workspace_trust_round_trips_through_global_config() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-workspace-trust-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);
    let workspace = temp_root.join("project");
    fs::create_dir_all(&workspace)?;

    assert!(!is_workspace_trusted(&workspace));
    let saved = save_workspace_trust(&workspace)?;

    assert_eq!(saved, temp_root.join(".deepseek").join("config.toml"));
    assert!(is_workspace_trusted(&workspace));
    assert!(!crate::tui::onboarding::needs_trust(&workspace));
    assert!(
        !workspace.join(".deepseek").exists(),
        "trust persistence must not create a project-local .deepseek directory"
    );

    let parsed: toml::Value = toml::from_str(&fs::read_to_string(saved)?)?;
    assert_eq!(
        workspace_trust_level_from_doc(&parsed, &workspace),
        Some("trusted")
    );
    Ok(())
}

#[test]
fn workspace_trust_reads_existing_projects_table() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-existing-project-trust-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);
    let workspace = temp_root.join("project");
    fs::create_dir_all(&workspace)?;
    let config_path = temp_root.join(".deepseek").join("config.toml");
    fs::create_dir_all(config_path.parent().unwrap())?;
    fs::write(
        &config_path,
        format!(
            "[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            workspace_config_key(&workspace)
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
        ),
    )?;

    assert!(is_workspace_trusted(&workspace));
    assert!(!crate::tui::onboarding::needs_trust(&workspace));
    Ok(())
}

#[test]
fn saved_credential_describe_returns_config_file_path() {
    let cf = SavedCredential::ConfigFile(PathBuf::from("/tmp/x.toml"));
    assert_eq!(cf.describe(), "/tmp/x.toml");
}

/// The durable-store outcome makes it explicit that config contains metadata,
/// not a second plaintext credential copy.
#[test]
fn saved_credential_describe_lists_both_targets_for_keyring_and_config() {
    let dual = SavedCredential::KeyringAndConfigFile {
        backend: "system keyring".to_string(),
        path: PathBuf::from("/tmp/x.toml"),
    };
    assert_eq!(
        dual.describe(),
        "secret store (system keyring); credential-free config metadata in /tmp/x.toml"
    );
}

#[test]
fn save_deepseek_key_uses_isolated_file_store_without_plaintext_config() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let codewhale_home = temp_root.path().join("codewhale-home");
    let config_path = codewhale_home.join("config.toml");
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());
    let _config_path = EnvVarGuard::set("CODEWHALE_CONFIG_PATH", config_path.as_os_str());
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");

    let saved = save_api_key("deepseek-test-credential")?;
    assert!(matches!(
        saved,
        SavedCredential::KeyringAndConfigFile { .. }
    ));

    let config = fs::read_to_string(&config_path)?;
    assert!(!config.contains("deepseek-test-credential"), "{config}");
    assert!(
        !config
            .lines()
            .any(|line| line.trim_start().starts_with("api_key ="))
    );
    assert!(config.contains("auth_mode = \"api_key\""));
    assert_eq!(
        codewhale_secrets::Secrets::auto_detect().get("deepseek")?,
        Some("deepseek-test-credential".to_string())
    );
    Ok(())
}

/// #5196: logout must remove the durable credential, not just the config
/// file entry. After save + `clear_api_key()`, the whole read chain —
/// secret-store slot first, config file second — must find nothing.
#[test]
fn full_logout_clears_secret_store_slot_and_config_document() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    // Canonicalize: the xAI credential walker opens each path component with
    // O_NOFOLLOW, so the lexical `/var` symlink in macOS tempdirs fails.
    let temp_root = temp_root.path().canonicalize()?;
    let _guard = EnvGuard::new(&temp_root);
    let codewhale_home = temp_root.join("codewhale-home");
    let config_path = codewhale_home.join("config.toml");
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());
    let _config_path = EnvVarGuard::set("CODEWHALE_CONFIG_PATH", config_path.as_os_str());
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");

    let saved = save_api_key("logout-credential")?;
    assert!(matches!(
        saved,
        SavedCredential::KeyringAndConfigFile { .. }
    ));
    assert_eq!(
        codewhale_secrets::Secrets::auto_detect().get("deepseek")?,
        Some("logout-credential".to_string())
    );

    clear_api_key()?;

    assert_eq!(
        codewhale_secrets::Secrets::auto_detect().get("deepseek")?,
        None,
        "logout must delete the durable secret-store slot"
    );
    assert_eq!(
        provider_secret_store_api_key(&Config::default(), ApiProvider::Deepseek),
        None,
        "the read chain must not find a cleared credential"
    );
    let config = fs::read_to_string(&config_path)?;
    assert!(!config.contains("logout-credential"), "{config}");
    assert!(
        !config
            .lines()
            .any(|line| line.trim_start().starts_with("api_key =")),
        "{config}"
    );
    Ok(())
}

/// #5196: the single-provider clear used by TUI `/logout` must delete that
/// provider's secret-store slot as well as its config-file entry.
#[test]
fn single_provider_logout_clears_secret_store_slot() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let temp_root = temp_root.path().canonicalize()?;
    let _guard = EnvGuard::new(&temp_root);
    let codewhale_home = temp_root.join("codewhale-home");
    let config_path = codewhale_home.join("config.toml");
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());
    let _config_path = EnvVarGuard::set("CODEWHALE_CONFIG_PATH", config_path.as_os_str());
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");

    save_api_key_for(ApiProvider::Openrouter, "openrouter-logout-credential")?;
    assert_eq!(
        codewhale_secrets::Secrets::auto_detect().get("openrouter")?,
        Some("openrouter-logout-credential".to_string())
    );

    clear_active_provider_api_key("openrouter")?;

    assert_eq!(
        codewhale_secrets::Secrets::auto_detect().get("openrouter")?,
        None,
        "single-provider logout must delete the durable secret-store slot"
    );
    assert_eq!(
        provider_secret_store_api_key(&Config::default(), ApiProvider::Openrouter),
        None,
        "the read chain must not find a cleared credential"
    );
    Ok(())
}

fn inject_plaintext_openrouter_key(config_path: &std::path::Path) -> Result<()> {
    let contents = fs::read_to_string(config_path)?;
    anyhow::ensure!(
        contents.contains("[providers.openrouter]"),
        "save must have created the provider table: {contents}"
    );
    fs::write(
        config_path,
        contents.replace(
            "[providers.openrouter]",
            "[providers.openrouter]\napi_key = \"logout-lock-plaintext\"",
        ),
    )?;
    Ok(())
}

struct ReleaseOnDrop(Option<mpsc::Sender<()>>);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}

/// Logout used to mutate the config document and (for `/logout`) the durable
/// slot with no write lock held, so a save racing a logout on one slot could
/// leave the store and the config file disagreeing. Both logout paths now
/// hold that provider's lock across the whole sequence.
#[test]
fn single_provider_logout_holds_the_slot_write_lock_across_config_and_store() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let temp_root = temp_root.path().canonicalize()?;
    let _guard = EnvGuard::new(&temp_root);
    let codewhale_home = temp_root.join("codewhale-home");
    let config_path = codewhale_home.join("config.toml");
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());
    let _config_path = EnvVarGuard::set("CODEWHALE_CONFIG_PATH", config_path.as_os_str());
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");

    save_api_key_for(ApiProvider::Openrouter, "openrouter-lock-credential")?;
    inject_plaintext_openrouter_key(&config_path)?;

    let (held_tx, held_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let holder = std::thread::spawn(move || {
        crate::credentials::store::with_provider_write_lock("openrouter", || {
            let _ = held_tx.send(());
            let _ = release_rx.recv();
        });
    });
    held_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("holder acquired the slot lock");
    let release = ReleaseOnDrop(Some(release_tx));

    let ticket = env_scope_ticket();
    let (done_tx, done_rx) = mpsc::channel();
    let logout = std::thread::spawn(move || {
        let _membership = join_env_scope(ticket);
        done_tx
            .send(clear_active_provider_api_key("openrouter"))
            .expect("send logout result");
    });

    let deadline = std::time::Instant::now() + Duration::from_millis(400);
    while std::time::Instant::now() < deadline {
        assert!(
            matches!(done_rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "single-provider logout finished while the slot write lock was held"
        );
        let config = fs::read_to_string(&config_path)?;
        assert!(
            config.contains("logout-lock-plaintext"),
            "logout must not mutate the config document before it holds the slot lock: {config}"
        );
        assert_eq!(
            codewhale_secrets::Secrets::auto_detect().get("openrouter")?,
            Some("openrouter-lock-credential".to_string()),
            "logout must not delete the slot while the write lock is held"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    drop(release);
    holder.join().expect("holder thread");
    done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("logout finished")?;
    logout.join().expect("logout thread");

    assert_eq!(
        codewhale_secrets::Secrets::auto_detect().get("openrouter")?,
        None,
        "logout must delete the durable slot once the lock is released"
    );
    let config = fs::read_to_string(&config_path)?;
    assert!(
        !config.contains("logout-lock-plaintext"),
        "logout must strip the injected plaintext once the lock is released: {config}"
    );
    Ok(())
}

#[test]
fn full_logout_holds_every_slot_write_lock_across_config_and_store() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let temp_root = temp_root.path().canonicalize()?;
    let _guard = EnvGuard::new(&temp_root);
    let codewhale_home = temp_root.join("codewhale-home");
    let config_path = codewhale_home.join("config.toml");
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());
    let _config_path = EnvVarGuard::set("CODEWHALE_CONFIG_PATH", config_path.as_os_str());
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");

    save_api_key_for(ApiProvider::Openrouter, "openrouter-lock-credential")?;
    inject_plaintext_openrouter_key(&config_path)?;

    let (held_tx, held_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let holder = std::thread::spawn(move || {
        crate::credentials::store::with_provider_write_lock("openrouter", || {
            let _ = held_tx.send(());
            let _ = release_rx.recv();
        });
    });
    held_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("holder acquired the slot lock");
    let release = ReleaseOnDrop(Some(release_tx));

    let ticket = env_scope_ticket();
    let (done_tx, done_rx) = mpsc::channel();
    let logout = std::thread::spawn(move || {
        let _membership = join_env_scope(ticket);
        done_tx.send(clear_api_key()).expect("send logout result");
    });

    let deadline = std::time::Instant::now() + Duration::from_millis(400);
    while std::time::Instant::now() < deadline {
        assert!(
            matches!(done_rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "full logout finished while a slot write lock was held"
        );
        let config = fs::read_to_string(&config_path)?;
        assert!(
            config.contains("logout-lock-plaintext"),
            "full logout must not mutate the config document before it holds the slot locks: {config}"
        );
        assert_eq!(
            codewhale_secrets::Secrets::auto_detect().get("openrouter")?,
            Some("openrouter-lock-credential".to_string()),
            "full logout must not delete a slot while that slot's write lock is held"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    drop(release);
    holder.join().expect("holder thread");
    done_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("logout finished")?;
    logout.join().expect("logout thread");

    assert_eq!(
        codewhale_secrets::Secrets::auto_detect().get("openrouter")?,
        None,
        "full logout must delete the durable slot once the lock is released"
    );
    let config = fs::read_to_string(&config_path)?;
    assert!(
        !config.contains("logout-lock-plaintext"),
        "full logout must strip the injected plaintext once the lock is released: {config}"
    );
    Ok(())
}

/// #5194: when both a config-file api_key and the provider's secret-store
/// slot hold a credential, the shadowing warning names both sources, says
/// which won, and hands over the resolve command.
#[test]
fn config_api_key_shadow_warning_names_sources_winner_and_resolution() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let codewhale_home = temp_root.path().join("codewhale-home");
    let config_path = codewhale_home.join("config.toml");
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());
    let _config_path = EnvVarGuard::set("CODEWHALE_CONFIG_PATH", config_path.as_os_str());
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
    codewhale_secrets::Secrets::auto_detect().set("openrouter", "store-key")?;

    let mut config = Config::default();
    config
        .provider_config_for_mut(ApiProvider::Openrouter)
        .api_key = Some("plaintext-config-key".to_string());

    let warning = config_api_key_shadow_warning(
        &config,
        ApiProvider::Openrouter,
        "`providers.openrouter` api_key",
    )
    .expect("a live secret-store slot shadowed by a config key must warn");
    assert!(
        warning.contains("`providers.openrouter` api_key"),
        "warning must name the config-file source: {warning}"
    );
    assert!(
        warning.contains("secret-store slot \"openrouter\""),
        "warning must name the secret-store source: {warning}"
    );
    assert!(
        warning.contains("the config-file key won"),
        "warning must say which source won: {warning}"
    );
    assert!(
        warning.contains("codewhale auth set --provider openrouter"),
        "warning must name the resolve command: {warning}"
    );
    Ok(())
}

/// #5194: no secret-store credential, no shadow, no warning.
#[test]
fn config_api_key_shadow_warning_stays_quiet_without_a_store_slot() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let codewhale_home = temp_root.path().join("codewhale-home");
    let config_path = codewhale_home.join("config.toml");
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());
    let _config_path = EnvVarGuard::set("CODEWHALE_CONFIG_PATH", config_path.as_os_str());
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");

    let mut config = Config::default();
    config
        .provider_config_for_mut(ApiProvider::Openrouter)
        .api_key = Some("plaintext-config-key".to_string());

    assert_eq!(
        config_api_key_shadow_warning(
            &config,
            ApiProvider::Openrouter,
            "`providers.openrouter` api_key"
        ),
        None,
        "a config key with no secret-store slot behind it is not a shadow"
    );
    Ok(())
}

#[test]
fn whitespace_codewhale_home_never_opens_ambient_file_secret_store() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let ambient_home = temp_root.path().join("ambient-home");
    let config_path = temp_root.path().join("isolated-config.toml");
    fs::create_dir_all(&ambient_home)?;
    let _home = EnvVarGuard::set("HOME", &ambient_home);
    let _userprofile = EnvVarGuard::set("USERPROFILE", &ambient_home);
    let _codewhale_home_unset = EnvVarGuard::remove("CODEWHALE_HOME");
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
    let _config_path = EnvVarGuard::set("CODEWHALE_CONFIG_PATH", &config_path);
    let ambient_store = codewhale_secrets::Secrets::file_backed();
    ambient_store.set("deepseek", "ambient-secret-sentinel")?;
    let ambient_secret_path = ambient_home
        .join(".codewhale")
        .join("secrets")
        .join("secrets.json");
    let before = fs::read(&ambient_secret_path)?;
    let _whitespace_home = EnvVarGuard::set("CODEWHALE_HOME", " \t ");
    let resolved_config_path = codewhale_config::resolve_config_path(None)?;

    let read = provider_secret_store_api_key(&Config::default(), ApiProvider::Deepseek);
    let saved = save_api_key("replacement-secret-sentinel")?;
    let after = fs::read(&ambient_secret_path)?;

    assert_eq!(
        read, None,
        "whitespace must not opt tests into ambient reads"
    );
    assert_eq!(saved, SavedCredential::ConfigFile(resolved_config_path));
    assert_eq!(after, before, "ambient file secret store was modified");
    Ok(())
}

#[test]
fn save_non_deepseek_key_uses_isolated_file_store_without_plaintext_config() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let codewhale_home = temp_root.path().join("codewhale-home");
    let config_path = codewhale_home.join("config.toml");
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());
    let _config_path = EnvVarGuard::set("CODEWHALE_CONFIG_PATH", config_path.as_os_str());
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");

    save_api_key_for(ApiProvider::Openrouter, "openrouter-test-credential")?;

    let config = fs::read_to_string(&config_path)?;
    assert!(!config.contains("openrouter-test-credential"), "{config}");
    let parsed: toml::Value = toml::from_str(&config)?;
    let openrouter = parsed
        .get("providers")
        .and_then(|providers| providers.get("openrouter"))
        .expect("openrouter metadata table");
    assert!(openrouter.get("api_key").is_none());
    assert_eq!(
        openrouter.get("auth_mode").and_then(toml::Value::as_str),
        Some("api_key")
    );
    assert_eq!(
        codewhale_secrets::Secrets::auto_detect().get("openrouter")?,
        Some("openrouter-test-credential".to_string())
    );
    Ok(())
}

#[test]
fn provider_api_key_config_failure_restores_secret_and_keeps_external_route() -> Result<()> {
    let _lock = lock_test_env();
    for prior in [None, Some("prior-xai-secret")] {
        let temp_root = tempfile::tempdir()?;
        let _guard = EnvGuard::new(temp_root.path());
        let codewhale_home = temp_root.path().canonicalize()?.join("codewhale-home");
        fs::create_dir_all(&codewhale_home)?;
        let config_path = codewhale_home.join("config.toml");
        fs::create_dir(&config_path)?;
        let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
        let _config_path = EnvVarGuard::set("CODEWHALE_CONFIG_PATH", &config_path);
        let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
        let generation = "xai-auth-0123456789abcdef0123456789abcdef.json";
        codewhale_config::with_xai_oauth_lifecycle_lock(|store| {
            store.write(generation, b"prior-owned-epoch", false)
        })?;
        let secrets = codewhale_secrets::Secrets::auto_detect();
        if let Some(prior) = prior {
            secrets.set("xai", prior)?;
        }
        let external_path = temp_root.path().join("external-grok.json");
        let route_config = Config {
            provider: Some(ApiProvider::Xai.as_str().to_string()),
            providers: Some(ProvidersConfig {
                xai: ProviderConfig {
                    auth_mode: Some("oauth".to_string()),
                    oauth_credential_generation: Some(generation.to_string()),
                    external_credentials: Some(
                        codewhale_config::ExternalCredentialConsentToml::read_only(
                            codewhale_config::ProviderKind::Xai,
                            codewhale_config::ExternalCredentialSource::GrokCli,
                            external_path,
                        ),
                    ),
                    ..ProviderConfig::default()
                },
                ..ProvidersConfig::default()
            }),
            ..Config::default()
        };
        let identity = ProviderIdentity {
            provider: ApiProvider::Xai,
            key: ApiProvider::Xai.as_str().to_string(),
            exact_id: Some(ApiProvider::Xai.as_str().to_string()),
            migrated_legacy_ollama_cloud_route: false,
        };
        let error = save_api_key_for_identity(&identity, &route_config, "new-xai-secret")
            .expect_err("config directory must reject metadata mutation");
        assert!(error.to_string().contains("config"), "{error:#}");
        assert_eq!(secrets.get("xai")?, prior.map(str::to_string));
        let xai = route_config
            .provider_config_for(ApiProvider::Xai)
            .expect("unchanged live route");
        assert_eq!(xai.auth_mode.as_deref(), Some("oauth"));
        assert!(xai.external_credentials.is_some());
        assert!(config_path.is_dir());
        assert_eq!(
            fs::read(codewhale_home.join("credentials").join(generation))?,
            b"prior-owned-epoch",
            "failed API-key mode switch must restore the prior OAuth epoch"
        );
    }
    Ok(())
}

#[test]
fn root_api_key_config_failure_restores_absent_and_existing_secret_state() -> Result<()> {
    let _lock = lock_test_env();
    for prior in [None, Some("prior-deepseek-secret")] {
        let temp_root = tempfile::tempdir()?;
        let _guard = EnvGuard::new(temp_root.path());
        let codewhale_home = temp_root.path().join("codewhale-home");
        fs::create_dir_all(&codewhale_home)?;
        let config_path = codewhale_home.join("config.toml");
        fs::create_dir(&config_path)?;
        let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
        let _config_path = EnvVarGuard::set("CODEWHALE_CONFIG_PATH", &config_path);
        let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
        let secrets = codewhale_secrets::Secrets::auto_detect();
        if let Some(prior) = prior {
            secrets.set("deepseek", prior)?;
        }

        let error = save_api_key("new-deepseek-secret")
            .expect_err("config directory must reject root metadata mutation");
        assert!(error.to_string().contains("config"), "{error:#}");
        assert_eq!(secrets.get("deepseek")?, prior.map(str::to_string));
        assert!(config_path.is_dir());
    }
    Ok(())
}

#[test]
fn save_key_refuses_plaintext_config_when_isolated_file_store_is_unwritable() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let codewhale_home = temp_root.path().join("codewhale-home");
    let config_path = codewhale_home.join("config.toml");
    fs::create_dir_all(codewhale_home.join("secrets"))?;
    fs::write(
        codewhale_home.join("secrets/secrets.json"),
        "not valid json",
    )?;
    #[cfg(unix)]
    fs::set_permissions(
        codewhale_home.join("secrets/secrets.json"),
        fs::Permissions::from_mode(0o600),
    )?;
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());
    let _config_path = EnvVarGuard::set("CODEWHALE_CONFIG_PATH", config_path.as_os_str());
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
    let resolved_config_path = codewhale_config::resolve_config_path(None)?;

    let error = save_api_key("fallback-test-credential")
        .expect_err("secret-store failure must not downgrade to plaintext");
    let message = format!("{error:#}");
    assert!(message.contains("Secret storage"), "{message}");
    assert!(message.contains("Refusing"), "{message}");
    assert!(
        message.contains(&codewhale_config::quote_os_path(&resolved_config_path)),
        "{message}"
    );
    assert!(
        !resolved_config_path.exists(),
        "plaintext config must stay untouched"
    );
    Ok(())
}

#[test]
fn provider_key_refuses_plaintext_config_when_secret_store_snapshot_fails() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let codewhale_home = temp_root.path().join("codewhale-home");
    let config_path = codewhale_home.join("config.toml");
    fs::create_dir_all(codewhale_home.join("secrets"))?;
    fs::write(
        codewhale_home.join("secrets/secrets.json"),
        "not valid json",
    )?;
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
    let _config_path = EnvVarGuard::set("CODEWHALE_CONFIG_PATH", &config_path);
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
    let resolved_config_path = codewhale_config::resolve_config_path(None)?;
    let identity = ProviderIdentity {
        provider: ApiProvider::Openrouter,
        key: ApiProvider::Openrouter.as_str().to_string(),
        exact_id: Some(ApiProvider::Openrouter.as_str().to_string()),
        migrated_legacy_ollama_cloud_route: false,
    };

    let error = save_api_key_for_identity(&identity, &Config::default(), "provider-fallback-key")
        .expect_err("provider key must not downgrade to plaintext");
    let message = format!("{error:#}");
    assert!(message.contains("snapshot"), "{message}");
    assert!(
        message.contains(&codewhale_config::quote_os_path(&resolved_config_path)),
        "{message}"
    );
    assert!(
        !resolved_config_path.exists(),
        "plaintext config must stay untouched"
    );
    Ok(())
}

#[test]
fn relative_codewhale_home_key_save_creates_no_workspace_state() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let relative_home = PathBuf::from(format!(
        ".codewhale-relative-home-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    assert!(!relative_home.exists());
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", &relative_home);
    let _config_path = EnvVarGuard::remove("CODEWHALE_CONFIG_PATH");
    let _legacy_config_path = EnvVarGuard::remove("DEEPSEEK_CONFIG_PATH");
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");

    let error = save_api_key("never-persisted")
        .expect_err("relative CODEWHALE_HOME must fail before persistence");
    let message = format!("{error:#}");
    assert!(message.contains("CODEWHALE_HOME"), "{message}");
    assert!(message.contains("absolute"), "{message}");
    assert!(
        !relative_home.exists(),
        "relative home must not create workspace state"
    );
    Ok(())
}

#[test]
fn has_api_key_detects_in_memory_override_and_env_var() -> Result<()> {
    // Pins the v0.8.8 contract: `has_api_key` covers the prompt-free
    // sources used by `Config::deepseek_api_key` (in-memory override,
    // env var, config-file slot).
    let _lock = lock_test_env();
    // Explicit in-memory key wins over every other source per
    // `Config::deepseek_api_key`'s "Path 0" override.
    let cfg = Config {
        api_key: Some("sk-in-memory-override".to_string()),
        ..Default::default()
    };
    assert!(
        has_api_key(&cfg),
        "in-memory override must be detected as a usable key"
    );

    // Env var path.
    let env_cfg = Config::default();
    unsafe {
        std::env::set_var("DEEPSEEK_API_KEY", "env-key");
    }
    assert!(
        has_api_key(&env_cfg),
        "env-var key must be detected even with empty config"
    );
    unsafe {
        std::env::remove_var("DEEPSEEK_API_KEY");
    }
    Ok(())
}

#[test]
fn deepseek_dispatcher_env_key_overrides_config_key() -> Result<()> {
    let _lock = lock_test_env();
    let prev_source = std::env::var_os("DEEPSEEK_API_KEY_SOURCE");
    unsafe {
        std::env::set_var("DEEPSEEK_API_KEY", "ark-dispatcher-key");
        std::env::set_var("DEEPSEEK_API_KEY_SOURCE", "cli");
    }
    let config = Config {
        api_key: Some("saved-deepseek-key".to_string()),
        ..Default::default()
    };

    assert_eq!(config.deepseek_api_key()?, "ark-dispatcher-key");

    unsafe {
        std::env::remove_var("DEEPSEEK_API_KEY");
        match prev_source {
            Some(value) => std::env::set_var("DEEPSEEK_API_KEY_SOURCE", value),
            None => std::env::remove_var("DEEPSEEK_API_KEY_SOURCE"),
        }
    }
    Ok(())
}

#[test]
fn provider_neutral_scenario() -> Result<()> {
    // Scenario consolidation of: provider_neutral_cli_key_wins_after_profile_provider_switch, provider_neutral_cli_key_requires_dispatcher_source_marker
    // from provider_neutral_cli_key_wins_after_profile_provider_switch
    {
        let _lock = lock_test_env();
        let _source = EnvVarGuard::set(codewhale_config::CLI_API_KEY_SOURCE_ENV, "cli");
        let _cli_key = EnvVarGuard::set(codewhale_config::CLI_API_KEY_ENV, "explicit-profile-key");
        let _anthropic_env = EnvVarGuard::remove("ANTHROPIC_API_KEY");
        let mut providers = ProvidersConfig::default();
        providers.anthropic.api_key = Some("saved-anthropic-key".to_string());
        let config = Config {
            provider: Some("anthropic".to_string()),
            providers: Some(providers),
            ..Default::default()
        };

        assert_eq!(config.deepseek_api_key()?, "explicit-profile-key");
        assert!(has_api_key(&config));
        assert!(active_provider_has_env_api_key(&config));
    }
    // from provider_neutral_cli_key_requires_dispatcher_source_marker
    {
        let _lock = lock_test_env();
        let _source = EnvVarGuard::remove(codewhale_config::CLI_API_KEY_SOURCE_ENV);
        let _legacy_source = EnvVarGuard::remove(codewhale_config::LEGACY_CLI_API_KEY_SOURCE_ENV);
        let _cli_key = EnvVarGuard::set(codewhale_config::CLI_API_KEY_ENV, "untrusted-generic-key");
        let _anthropic_env = EnvVarGuard::remove("ANTHROPIC_API_KEY");
        let mut providers = ProvidersConfig::default();
        providers.anthropic.api_key = Some("saved-anthropic-key".to_string());
        let config = Config {
            provider: Some("anthropic".to_string()),
            providers: Some(providers),
            ..Default::default()
        };

        assert_eq!(config.deepseek_api_key()?, "saved-anthropic-key");
    }
    Ok(())
}

fn config_with_provider_scoped_key(provider: &str, api_key: &str) -> Config {
    let mut providers = ProvidersConfig::default();
    match provider {
        "deepseek" | "deepseek-cn" => {
            providers.deepseek.api_key = Some(api_key.to_string());
        }
        "nvidia-nim" => {
            providers.nvidia_nim.api_key = Some(api_key.to_string());
        }
        "openai" => {
            providers.openai.api_key = Some(api_key.to_string());
        }
        "wanjie-ark" => {
            providers.wanjie_ark.api_key = Some(api_key.to_string());
        }
        "openrouter" => {
            providers.openrouter.api_key = Some(api_key.to_string());
        }
        "novita" => {
            providers.novita.api_key = Some(api_key.to_string());
        }
        "fireworks" => {
            providers.fireworks.api_key = Some(api_key.to_string());
        }
        "siliconflow" => {
            providers.siliconflow.api_key = Some(api_key.to_string());
        }
        "sglang" => {
            providers.sglang.api_key = Some(api_key.to_string());
        }
        "vllm" => {
            providers.vllm.api_key = Some(api_key.to_string());
        }
        "ollama" => {
            providers.ollama.api_key = Some(api_key.to_string());
        }
        "huggingface" => {
            providers.huggingface.api_key = Some(api_key.to_string());
        }
        "qianfan" => {
            providers.qianfan.api_key = Some(api_key.to_string());
        }
        _ => panic!("unexpected provider {provider}"),
    }

    Config {
        provider: Some(provider.to_string()),
        providers: Some(providers),
        ..Config::default()
    }
}

#[test]
fn has_api_scenario() -> Result<()> {
    // Scenario consolidation of: has_api_key_uses_active_provider_scoped_config_key, has_api_key_uses_root_config_key_for_deepseek_variants, has_api_key_for_uses_deepseek_cn_provider_table, has_api_key_for_uses_root_config_key_for_deepseek_variants
    // from has_api_key_uses_active_provider_scoped_config_key
    {
        // `has_api_key` intentionally consults live endpoint env overrides. Keep
        // this config-only assertion out of the windows where another test owns a
        // process-global custom endpoint.
        let _lock = lock_test_env();
        for provider in [
            "openai",
            "wanjie-ark",
            "openrouter",
            "novita",
            "fireworks",
            "siliconflow",
            "qianfan",
        ] {
            let config = config_with_provider_scoped_key(provider, "provider-config-key");

            assert!(
                has_api_key(&config),
                "active provider config key must satisfy onboarding auth check for {provider}"
            );
        }
    }
    // from has_api_key_uses_root_config_key_for_deepseek_variants
    {
        // A concurrent CODEWHALE_BASE_URL override deliberately unbinds the saved
        // root key from the active endpoint. Serialize this assertion with the
        // tests that install those process-global overrides.
        let _lock = lock_test_env();
        for provider in ["deepseek", "deepseek-cn"] {
            let config = Config {
                provider: Some(provider.to_string()),
                api_key: Some("root-config-key".to_string()),
                ..Config::default()
            };

            assert!(
                has_api_key(&config),
                "root config api_key must satisfy onboarding auth check for {provider}"
            );
        }
    }
    // from has_api_key_for_uses_deepseek_cn_provider_table
    {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-has-key-cn-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let mut providers = ProvidersConfig::default();
        providers.deepseek_cn.api_key = Some("cn-file-key".to_string());
        let config = Config {
            providers: Some(providers),
            ..Config::default()
        };

        assert!(has_api_key_for(&config, ApiProvider::DeepseekCN));
    }
    // from has_api_key_for_uses_root_config_key_for_deepseek_variants
    {
        let _lock = lock_test_env();
        let config = Config {
            api_key: Some("root-config-key".to_string()),
            ..Config::default()
        };

        assert!(has_api_key_for(&config, ApiProvider::Deepseek));
        assert!(has_api_key_for(&config, ApiProvider::DeepseekCN));
    }
    Ok(())
}

#[test]
fn has_api_key_uses_active_provider_env_key() -> Result<()> {
    let _lock = lock_test_env();
    for (provider, env_var) in [
        ("openai", "OPENAI_API_KEY"),
        ("wanjie-ark", "WANJIE_ARK_API_KEY"),
        ("openrouter", "OPENROUTER_API_KEY"),
        ("novita", "NOVITA_API_KEY"),
        ("fireworks", "FIREWORKS_API_KEY"),
        ("siliconflow", "SILICONFLOW_API_KEY"),
        ("qianfan", "QIANFAN_API_KEY"),
    ] {
        unsafe {
            std::env::set_var(env_var, "provider-env-key");
        }

        let config = Config {
            provider: Some(provider.to_string()),
            ..Config::default()
        };

        assert!(
            has_api_key(&config),
            "active provider env key must satisfy onboarding auth check for {provider}"
        );

        unsafe {
            std::env::remove_var(env_var);
        }
    }
    Ok(())
}

/// Regression for #343: clear_api_key strips both the root `api_key`
/// and any nested `[providers.<name>].api_key` lines from config.toml
/// so a stale credential can't shadow a fresh login.
#[test]
fn clear_api_key_strips_root_and_provider_scoped_keys() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-clear-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let temp_root = temp_root.canonicalize()?;
    let _guard = EnvGuard::new(&temp_root);

    let config_dir = temp_root.join(".deepseek");
    fs::create_dir_all(&config_dir)?;
    let config_path = config_dir.join("config.toml");
    fs::write(
        &config_path,
        r#"api_key = "old-root-key"
default_text_model = "deepseek-v4-flash"

[providers.deepseek]
api_key = "old-provider-key"
base_url = "https://api.deepseek.com"

[providers.openrouter]
api_key = "old-openrouter-key"
"#,
    )?;

    clear_api_key()?;

    let after = fs::read_to_string(&config_path)?;
    assert!(
        !after.contains("old-root-key"),
        "root api_key must be stripped: {after}"
    );
    assert!(
        !after.contains("old-provider-key"),
        "provider-scoped codewhale key must be stripped: {after}"
    );
    assert!(
        !after.contains("old-openrouter-key"),
        "provider-scoped openrouter key must be stripped: {after}"
    );
    // Non-credential lines must survive.
    assert!(after.contains("default_text_model"));
    assert!(after.contains("base_url"));
    Ok(())
}

/// Finding #20 golden: a comment that merely mentions `api_key` used to
/// defeat the insert (the old `existing.contains("api_key")` scan treated it
/// as an existing assignment and never wrote the key). The TOML-aware path
/// must insert the real key and keep the comment.
#[test]
fn save_api_key_inserts_key_when_only_a_comment_mentions_it() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-api-key-comment-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    fs::create_dir_all(config_path.parent().unwrap())?;
    fs::write(
        &config_path,
        "# api_key = \"sk-placeholder\" (uncomment to set manually)\n\
         default_text_model = \"deepseek-v4-flash\"\n",
    )?;

    save_api_key("fresh-key")?;

    let after = fs::read_to_string(&config_path)?;
    assert!(
        after.contains("# api_key = \"sk-placeholder\""),
        "comment must survive: {after}"
    );
    assert!(
        after.contains("default_text_model = \"deepseek-v4-flash\""),
        "unrelated key must survive: {after}"
    );
    let parsed: toml::Value = toml::from_str(&after)?;
    assert_eq!(
        parsed.get("api_key").and_then(toml::Value::as_str),
        Some("fresh-key"),
        "real key must be inserted despite the comment: {after}"
    );
    Ok(())
}

/// Replacing an existing root api_key must keep surrounding comments,
/// including the trailing comment on the api_key line itself.
#[test]
fn save_api_key_replaces_existing_key_preserving_comments() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-api-key-replace-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    fs::create_dir_all(config_path.parent().unwrap())?;
    fs::write(
        &config_path,
        r#"# top note
api_key = "old-key" # keep secret
model = "deepseek-v4-pro"

# provider note
[providers.openrouter]
base_url = "https://openrouter.ai/api/v1"
"#,
    )?;

    save_api_key("new-key")?;

    let after = fs::read_to_string(&config_path)?;
    assert!(
        after.contains("api_key = \"new-key\" # keep secret"),
        "value must be replaced in place with its comment: {after}"
    );
    assert!(!after.contains("old-key"), "{after}");
    assert!(after.contains("# top note"), "{after}");
    assert!(after.contains("# provider note"), "{after}");
    Ok(())
}

/// Provider-scoped key saves used to round-trip through `toml::Value`
/// pretty-printing, which dropped every comment in the file.
#[test]
fn save_api_key_for_preserves_comments_in_provider_tables() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-provider-key-comments-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    fs::create_dir_all(config_path.parent().unwrap())?;
    fs::write(
        &config_path,
        r#"# root note
model = "deepseek-v4-pro"

# openrouter note
[providers.openrouter]
base_url = "https://openrouter.ai/api/v1" # pinned
"#,
    )?;

    save_api_key_for(ApiProvider::Openrouter, "or-key")?;

    let after = fs::read_to_string(&config_path)?;
    assert!(after.contains("# root note"), "{after}");
    assert!(after.contains("# openrouter note"), "{after}");
    assert!(
        after.contains("base_url = \"https://openrouter.ai/api/v1\" # pinned"),
        "inline comment must survive: {after}"
    );
    let parsed: toml::Value = toml::from_str(&after)?;
    assert_eq!(
        parsed
            .get("providers")
            .and_then(|providers| providers.get("openrouter"))
            .and_then(|entry| entry.get("api_key"))
            .and_then(toml::Value::as_str),
        Some("or-key"),
        "{after}"
    );
    Ok(())
}

/// Clearing credentials must not disturb comments, `api_key_env`, or
/// provider tables with quoted names.
#[test]
fn clear_api_key_preserves_comments_and_unrelated_keys() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-clear-comments-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let temp_root = temp_root.canonicalize()?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    fs::create_dir_all(config_path.parent().unwrap())?;
    fs::write(
        &config_path,
        r#"# root note
api_key = "old-root-key"
api_key_env = "MY_KEY_ENV"
model = "deepseek-v4-pro"

# provider note
[providers."quoted.provider"]
api_key = "old-quoted-key"
base_url = "https://quoted.example/v1"
"#,
    )?;

    clear_api_key()?;

    let after = fs::read_to_string(&config_path)?;
    assert!(!after.contains("old-root-key"), "{after}");
    assert!(
        !after.contains("old-quoted-key"),
        "quoted provider table key must be stripped: {after}"
    );
    assert!(
        after.contains("api_key_env = \"MY_KEY_ENV\""),
        "api_key_env must not be stripped: {after}"
    );
    assert!(after.contains("# root note"), "{after}");
    assert!(after.contains("# provider note"), "{after}");
    assert!(after.contains("model = \"deepseek-v4-pro\""), "{after}");
    assert!(
        after.contains("base_url = \"https://quoted.example/v1\""),
        "{after}"
    );
    Ok(())
}

/// The old line matcher compared against the literal `[providers.<name>]`
/// header, so a quoted header (`[providers."openrouter"]`) was never
/// matched and the key survived a targeted clear.
#[test]
fn clear_active_provider_api_key_handles_quoted_table_headers() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-clear-quoted-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    fs::create_dir_all(config_path.parent().unwrap())?;
    fs::write(
        &config_path,
        r#"api_key = "root-key"

[providers."openrouter"]
api_key = "old-openrouter-key"
base_url = "https://openrouter.ai/api/v1"
"#,
    )?;

    clear_active_provider_api_key("openrouter")?;

    let after = fs::read_to_string(&config_path)?;
    assert!(
        !after.contains("old-openrouter-key"),
        "quoted provider header must be matched: {after}"
    );
    assert!(
        after.contains("api_key = \"root-key\""),
        "root key belongs to deepseek and must survive: {after}"
    );
    assert!(
        after.contains("base_url = \"https://openrouter.ai/api/v1\""),
        "{after}"
    );
    Ok(())
}

#[test]
fn clear_active_provider_api_key_clears_deepseek_cn_root_scope() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-clear-deepseek-cn-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);
    let config_path = temp_root.join(".deepseek").join("config.toml");
    fs::create_dir_all(config_path.parent().unwrap())?;
    fs::write(
        &config_path,
        r#"provider = "deepseek-cn"
api_key = "deepseek-cn-root-key"

[providers.deepseek-cn]
api_key = "deepseek-cn-table-key"

[providers.openrouter]
api_key = "unrelated-key"
"#,
    )?;

    clear_active_provider_api_key("deepseek-cn")?;

    let after = fs::read_to_string(&config_path)?;
    assert!(!after.contains("deepseek-cn-root-key"), "{after}");
    assert!(!after.contains("deepseek-cn-table-key"), "{after}");
    assert!(after.contains("unrelated-key"), "{after}");
    Ok(())
}

#[test]
fn clear_active_provider_api_key_distinguishes_literal_and_named_custom_routes() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-clear-custom-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);
    let config_path = temp_root.join(".deepseek").join("config.toml");
    fs::create_dir_all(config_path.parent().unwrap())?;
    let contents = r#"provider = "custom"
api_key = "legacy-root-key"
base_url = "http://127.0.0.1:1234/v1"
default_text_model = "legacy-model"

[providers.lm-studio]
kind = "openai-compatible"
api_key = "named-route-key"
base_url = "http://127.0.0.1:5678/v1"
model = "named-model"
"#;
    fs::write(&config_path, contents)?;

    clear_active_provider_api_key("custom")?;

    let after_literal = fs::read_to_string(&config_path)?;
    assert!(
        !after_literal.contains("legacy-root-key"),
        "{after_literal}"
    );
    assert!(after_literal.contains("named-route-key"), "{after_literal}");

    fs::write(&config_path, contents)?;
    clear_active_provider_api_key("lm-studio")?;

    let after_named = fs::read_to_string(&config_path)?;
    assert!(after_named.contains("legacy-root-key"), "{after_named}");
    assert!(!after_named.contains("named-route-key"), "{after_named}");
    Ok(())
}

#[test]
fn clear_active_provider_api_key_prefers_exact_custom_table_over_legacy_root() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-clear-exact-custom-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);
    let config_path = temp_root.join(".deepseek").join("config.toml");
    fs::create_dir_all(config_path.parent().unwrap())?;
    fs::write(
        &config_path,
        r#"provider = "custom"
api_key = "legacy-root-key"
base_url = "http://127.0.0.1:1234/v1"
default_text_model = "legacy-model"

[providers.custom]
kind = "openai-compatible"
api_key = "exact-table-key"
base_url = "http://127.0.0.1:5678/v1"
model = "exact-model"
"#,
    )?;

    clear_active_provider_api_key("custom")?;

    let after = fs::read_to_string(&config_path)?;
    assert!(after.contains("legacy-root-key"), "{after}");
    assert!(!after.contains("exact-table-key"), "{after}");
    assert!(after.contains("[providers.custom]"), "{after}");
    Ok(())
}

/// Finding #19: workspace-trust saves used to round-trip through
/// `toml::to_string_pretty`, destroying comments in the whole file.
#[test]
fn save_workspace_trust_preserves_comments() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-trust-comments-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);
    let workspace = temp_root.join("project");
    fs::create_dir_all(&workspace)?;

    let config_path = temp_root.join(".deepseek").join("config.toml");
    fs::create_dir_all(config_path.parent().unwrap())?;
    fs::write(
        &config_path,
        r#"# top note
model = "deepseek-v4-pro"

# projects note
[projects."/existing/workspace"]
trust_level = "trusted" # granted earlier
"#,
    )?;

    save_workspace_trust(&workspace)?;

    let after = fs::read_to_string(&config_path)?;
    assert!(after.contains("# top note"), "{after}");
    assert!(after.contains("# projects note"), "{after}");
    assert!(after.contains("# granted earlier"), "{after}");
    assert!(
        after.contains("[projects.\"/existing/workspace\"]"),
        "existing project entry must survive: {after}"
    );
    assert!(is_workspace_trusted(&workspace));
    Ok(())
}

/// Regression for #343: explicit in-memory `api_key` (non-empty,
/// non-sentinel) wins over env/config so a freshly-typed onboarding
/// key takes effect immediately.
#[test]
fn deepseek_api_key_prefers_explicit_in_memory_override() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-override-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config = Config {
        api_key: Some("freshly-typed-key".to_string()),
        ..Config::default()
    };
    let resolved = config
        .deepseek_api_key()
        .expect("explicit override must resolve");
    assert_eq!(resolved, "freshly-typed-key");
    Ok(())
}

#[test]
fn deepseek_api_key_prefers_saved_config_over_stale_env() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-config-over-env-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    unsafe {
        env::set_var("DEEPSEEK_API_KEY", "stale-env-key");
    }
    let config = Config {
        api_key: Some("fresh-config-key".to_string()),
        ..Config::default()
    };
    assert_eq!(config.deepseek_api_key()?, "fresh-config-key");
    unsafe {
        env::remove_var("DEEPSEEK_API_KEY");
    }
    Ok(())
}

#[test]
fn standalone_tui_reads_saved_secret_before_ambient_env() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let codewhale_home = temp_root.path().join("isolated-codewhale");
    fs::create_dir_all(&codewhale_home)?;
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
    let _ambient_key = EnvVarGuard::set("DEEPSEEK_API_KEY", "stale-env-key");

    let secrets = codewhale_secrets::Secrets::auto_detect();
    secrets.set("deepseek", "saved-secret-key")?;

    let config = Config::default();
    assert_eq!(config.deepseek_api_key()?, "saved-secret-key");
    assert!(has_api_key(&config));
    assert!(active_provider_has_config_api_key(&config));

    let configured = Config {
        api_key: Some("fresh-config-key".to_string()),
        ..Config::default()
    };
    assert_eq!(configured.deepseek_api_key()?, "fresh-config-key");
    Ok(())
}

#[test]
fn authenticated_local_provider_reads_saved_secret() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let codewhale_home = temp_root.path().join("isolated-codewhale");
    fs::create_dir_all(&codewhale_home)?;
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
    let _ambient_key = EnvVarGuard::remove("VLLM_API_KEY");

    codewhale_secrets::Secrets::auto_detect().set("vllm", "saved-local-secret")?;

    let mut providers = ProvidersConfig::default();
    providers.vllm.base_url = Some("http://127.0.0.1:8000/v1".to_string());
    providers.vllm.auth_mode = Some("api_key".to_string());
    let config = Config {
        provider: Some("vllm".to_string()),
        providers: Some(providers),
        ..Config::default()
    };

    assert_eq!(config.deepseek_api_key()?, "saved-local-secret");
    assert!(has_api_key(&config));
    assert!(active_provider_has_config_api_key(&config));
    Ok(())
}

#[test]
fn named_custom_provider_never_reuses_generic_custom_secret() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let codewhale_home = temp_root.path().join("isolated-codewhale");
    fs::create_dir_all(&codewhale_home)?;
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");

    codewhale_secrets::Secrets::auto_detect().set("custom", "endpoint-a-secret")?;

    let mut providers = ProvidersConfig::default();
    providers.custom.insert(
        "endpoint-b".to_string(),
        ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("https://endpoint-b.example.test/v1".to_string()),
            model: Some("endpoint-b-model".to_string()),
            auth_mode: Some("api_key".to_string()),
            ..ProviderConfig::default()
        },
    );
    let config = Config {
        provider: Some("endpoint-b".to_string()),
        providers: Some(providers),
        ..Config::default()
    };

    assert!(config.should_skip_secret_store_for_provider(ApiProvider::Custom));
    assert!(provider_secret_store_api_key(&config, ApiProvider::Custom).is_none());
    assert!(config.deepseek_api_key().is_err());
    assert!(!has_api_key(&config));
    assert!(!active_provider_has_config_api_key(&config));
    Ok(())
}

#[test]
fn built_in_provider_custom_endpoint_never_reuses_global_credentials() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let codewhale_home = temp_root.path().join("isolated-codewhale");
    fs::create_dir_all(&codewhale_home)?;
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
    let _ambient = EnvVarGuard::set("OPENROUTER_API_KEY", "ambient-official-key");
    codewhale_secrets::Secrets::auto_detect().set("openrouter", "saved-official-key")?;

    let mut providers = ProvidersConfig::default();
    providers.openrouter.base_url = Some("https://gateway.example.test/v1".to_string());
    let config = Config {
        provider: Some("openrouter".to_string()),
        providers: Some(providers),
        ..Config::default()
    };

    assert!(config.provider_uses_custom_endpoint(ApiProvider::Openrouter));
    assert!(config.should_skip_secret_store_for_provider(ApiProvider::Openrouter));
    assert!(config.deepseek_api_key().is_err());
    assert!(!has_api_key(&config));
    assert!(!active_provider_has_config_api_key(&config));
    assert!(!active_provider_has_env_api_key(&config));
    Ok(())
}

#[test]
fn custom_endpoint_accepts_route_bound_api_key_env_and_reports_ready() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let _route_key = EnvVarGuard::set("MY_GATEWAY_ROUTE_KEY", "route-bound-key");
    let _ambient = EnvVarGuard::set("OPENROUTER_API_KEY", "ambient-official-key");

    let mut providers = ProvidersConfig::default();
    providers.openrouter.base_url = Some("https://gateway.example.test/v1".to_string());
    providers.openrouter.api_key_env = Some("MY_GATEWAY_ROUTE_KEY".to_string());
    let config = Config {
        provider: Some("openrouter".to_string()),
        providers: Some(providers),
        ..Config::default()
    };

    assert_eq!(config.deepseek_api_key()?, "route-bound-key");
    assert!(has_api_key_for(&config, ApiProvider::Openrouter));
    assert!(active_provider_has_env_api_key(&config));
    assert!(active_provider_uses_env_only_api_key(&config));
    Ok(())
}

#[test]
fn env_selected_custom_endpoints_do_not_rebind_file_credentials() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let config_path = temp_root.path().join("config.toml");
    let _route_key = EnvVarGuard::set("MY_GATEWAY_ROUTE_KEY", "route-bound-file-key");
    let _source = EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
    let _cli_key = EnvVarGuard::remove("CODEWHALE_CLI_API_KEY");

    fs::write(
        &config_path,
        r#"api_key = "saved-root-deepseek-key"
default_text_model = "deepseek-chat"
"#,
    )?;
    {
        let _generic_base = EnvVarGuard::set(
            "CODEWHALE_BASE_URL",
            "https://generic-gateway.example.test/v1",
        );
        let config = Config::load(Some(config_path.clone()), None)?;
        assert!(config.provider_uses_custom_endpoint(ApiProvider::Deepseek));
        assert!(config.deepseek_api_key().is_err());
        assert!(!active_provider_has_config_api_key(&config));
        assert!(!active_provider_has_env_api_key(&config));
        assert!(!has_api_key_for(&config, ApiProvider::Deepseek));
    }

    fs::write(
        &config_path,
        r#"provider = "openrouter"

[providers.openrouter]
api_key = "saved-openrouter-route-key"
api_key_env = "MY_GATEWAY_ROUTE_KEY"
model = "openai/gpt-5"
"#,
    )?;
    for (env_name, endpoint) in [
        (
            "CODEWHALE_BASE_URL",
            "https://generic-openrouter-gateway.example.test/v1",
        ),
        (
            "OPENROUTER_BASE_URL",
            "https://provider-openrouter-gateway.example.test/v1",
        ),
    ] {
        let _base = EnvVarGuard::set(env_name, endpoint);
        let config = Config::load(Some(config_path.clone()), None)?;
        assert_eq!(config.deepseek_base_url(), endpoint);
        assert!(config.provider_uses_custom_endpoint(ApiProvider::Openrouter));
        assert!(config.deepseek_api_key().is_err());
        assert!(!active_provider_has_config_api_key(&config));
        assert!(!active_provider_has_env_api_key(&config));
        assert!(!has_api_key_for(&config, ApiProvider::Openrouter));
    }

    Ok(())
}

#[test]
fn file_bound_custom_endpoints_keep_route_credentials() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let config_path = temp_root.path().join("config.toml");
    let _route_key = EnvVarGuard::set("MY_GATEWAY_ROUTE_KEY", "file-env-route-key");

    fs::write(
        &config_path,
        r#"api_key = "file-root-key"
base_url = "https://file-deepseek-gateway.example.test/v1"
default_text_model = "private-deepseek-model"
"#,
    )?;
    let root = Config::load(Some(config_path.clone()), None)?;
    assert_eq!(root.deepseek_api_key()?, "file-root-key");
    assert!(active_provider_has_config_api_key(&root));
    assert!(has_api_key_for(&root, ApiProvider::Deepseek));

    fs::write(
        &config_path,
        r#"provider = "openrouter"

[providers.openrouter]
api_key = "file-provider-key"
base_url = "https://file-openrouter-gateway.example.test/v1"
model = "private-openrouter-model"
"#,
    )?;
    let provider_key = Config::load(Some(config_path.clone()), None)?;
    assert_eq!(provider_key.deepseek_api_key()?, "file-provider-key");
    assert!(active_provider_has_config_api_key(&provider_key));
    assert!(has_api_key_for(&provider_key, ApiProvider::Openrouter));

    fs::write(
        &config_path,
        r#"provider = "openrouter"

[providers.openrouter]
api_key_env = "MY_GATEWAY_ROUTE_KEY"
base_url = "https://file-openrouter-gateway.example.test/v1"
model = "private-openrouter-model"
"#,
    )?;
    let route_env = Config::load(Some(config_path), None)?;
    assert_eq!(route_env.deepseek_api_key()?, "file-env-route-key");
    assert!(active_provider_has_env_api_key(&route_env));
    assert!(has_api_key_for(&route_env, ApiProvider::Openrouter));
    Ok(())
}

/// A session-scoped `[providers.*]` fixture with credentials but no endpoints,
/// so every base URL in these tests comes from the resolver rather than a file.
const CROSS_PROVIDER_ROUTE_FIXTURE: &str = r#"api_key = "session-deepseek-key"
default_text_model = "deepseek-chat"

[providers.moonshot]
api_key = "moonshot-route-key"

[providers.zai]
api_key = "zai-route-key"

[providers.minimax]
api_key = "minimax-route-key"
"#;

#[test]
fn generic_base_url_override_never_reaches_pinned_child_routes() -> Result<()> {
    // #4093-class routing truth: every cross-provider seam (pinned subagent /
    // fleet child, per-turn auto-router, tool routing, picker preview) clones
    // the session config and re-points `provider`. The generic endpoint
    // override belongs to the DeepSeek session that set it and must never
    // follow a child to another vendor's route.
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let config_path = temp_root.path().join("config.toml");
    fs::write(&config_path, CROSS_PROVIDER_ROUTE_FIXTURE)?;

    for env_name in ["CODEWHALE_BASE_URL", "DEEPSEEK_BASE_URL"] {
        let session_host = "https://session-gateway.example.test/v1";
        let _base = EnvVarGuard::set(env_name, session_host);
        let config = Config::load(Some(config_path.clone()), None)?;

        // Documented behavior for the active DeepSeek route is unchanged.
        assert_eq!(config.api_provider(), ApiProvider::Deepseek);
        assert_eq!(config.deepseek_base_url(), session_host);
        assert!(config.provider_uses_custom_endpoint(ApiProvider::Deepseek));

        for (provider, expected) in [
            (ApiProvider::Moonshot, DEFAULT_MOONSHOT_BASE_URL),
            (ApiProvider::Zai, DEFAULT_ZAI_BASE_URL),
            (ApiProvider::Minimax, DEFAULT_MINIMAX_BASE_URL),
        ] {
            assert_eq!(
                config.base_url_for_route(provider),
                expected,
                "{env_name}: {provider:?} must resolve from its own identity table"
            );
            assert!(
                !config.provider_uses_custom_endpoint(provider),
                "{env_name}: {provider:?} is on its canonical host, not a custom one"
            );

            let route = crate::route_runtime::resolve_runtime_route(&config, provider, None)
                .unwrap_or_else(|err| panic!("{env_name}: {provider:?} child route: {err}"));
            // The scoped config and the executable candidate must agree, and
            // neither may name the session host.
            assert_eq!(route.config.deepseek_base_url(), expected);
            assert_eq!(route.candidate.endpoint().base_url, expected);
            assert_ne!(route.candidate.endpoint().base_url, session_host);
        }

        // An unknown/custom identity fails closed on the loopback placeholder
        // instead of borrowing the DeepSeek session route.
        let custom_placeholder = normalize_base_url(
            codewhale_config::ProviderKind::Custom
                .provider()
                .default_base_url(),
        );
        assert_eq!(
            config.base_url_for_route(ApiProvider::Custom),
            custom_placeholder
        );
        assert_ne!(config.base_url_for_route(ApiProvider::Custom), session_host);
    }

    Ok(())
}

#[test]
fn provider_scoped_base_url_env_applies_only_to_its_own_route() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let config_path = temp_root.path().join("config.toml");
    fs::write(&config_path, CROSS_PROVIDER_ROUTE_FIXTURE)?;

    let moonshot_host = "https://moonshot-gateway.example.test/v1";
    let _moonshot = EnvVarGuard::set("MOONSHOT_BASE_URL", moonshot_host);

    // Without a generic override the active DeepSeek route keeps its default:
    // a provider-scoped variable names exactly one provider.
    let config = Config::load(Some(config_path.clone()), None)?;
    assert_eq!(config.deepseek_base_url(), DEFAULT_DEEPSEEK_BASE_URL);
    assert_eq!(
        config.base_url_for_route(ApiProvider::Zai),
        DEFAULT_ZAI_BASE_URL
    );
    assert_eq!(
        config.base_url_for_route(ApiProvider::Moonshot),
        moonshot_host
    );
    let route = crate::route_runtime::resolve_runtime_route(&config, ApiProvider::Moonshot, None)
        .expect("Moonshot child route");
    assert_eq!(route.candidate.endpoint().base_url, moonshot_host);

    // With both set, each override stays on its own route.
    let session_host = "https://session-gateway.example.test/v1";
    let _base = EnvVarGuard::set("CODEWHALE_BASE_URL", session_host);
    let config = Config::load(Some(config_path), None)?;
    assert_eq!(config.deepseek_base_url(), session_host);
    assert_eq!(
        config.base_url_for_route(ApiProvider::Moonshot),
        moonshot_host
    );
    assert_eq!(
        config.base_url_for_route(ApiProvider::Zai),
        DEFAULT_ZAI_BASE_URL
    );
    let route = crate::route_runtime::resolve_runtime_route(&config, ApiProvider::Moonshot, None)
        .expect("Moonshot child route");
    assert_eq!(route.config.deepseek_base_url(), moonshot_host);
    assert_eq!(route.candidate.endpoint().base_url, moonshot_host);
    assert_ne!(route.candidate.endpoint().base_url, session_host);

    Ok(())
}

#[test]
fn source_marked_cli_key_can_follow_cli_forwarded_custom_base_url() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let config_path = temp_root.path().join("config.toml");
    fs::write(
        &config_path,
        r#"api_key = "saved-root-key"
base_url = "https://api.deepseek.com/v1"
default_text_model = "deepseek-chat"
"#,
    )?;
    let _base = EnvVarGuard::set(
        "DEEPSEEK_BASE_URL",
        "https://explicit-cli-gateway.example.test/v1",
    );
    let _source = EnvVarGuard::set(codewhale_config::CLI_API_KEY_SOURCE_ENV, "cli");
    let _cli_key = EnvVarGuard::set(codewhale_config::CLI_API_KEY_ENV, "explicit-cli-key");

    let config = Config::load(Some(config_path), None)?;
    assert_eq!(config.deepseek_api_key()?, "explicit-cli-key");
    assert!(!active_provider_has_config_api_key(&config));
    assert!(active_provider_has_env_api_key(&config));
    assert!(active_provider_uses_env_only_api_key(&config));
    assert!(has_api_key_for(&config, ApiProvider::Deepseek));
    Ok(())
}

#[test]
fn managed_file_endpoint_replaces_lower_env_provenance() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let config_path = temp_root.path().join("config.toml");
    let managed_path = temp_root.path().join("managed.toml");
    fs::write(
        &managed_path,
        r#"[providers.openrouter]
api_key = "managed-route-key"
base_url = "https://managed-gateway.example.test/v1"
model = "managed-model"
"#,
    )?;
    fs::write(
        &config_path,
        format!(
            "provider = \"openrouter\"\nmanaged_config_path = {:?}\n",
            managed_path.display().to_string()
        ),
    )?;
    let _base = EnvVarGuard::set(
        "CODEWHALE_BASE_URL",
        "https://lower-env-gateway.example.test/v1",
    );

    let config = Config::load(Some(config_path), None)?;
    assert_eq!(
        config.deepseek_base_url(),
        "https://managed-gateway.example.test/v1"
    );
    assert_eq!(config.deepseek_api_key()?, "managed-route-key");
    assert!(active_provider_has_config_api_key(&config));
    assert!(has_api_key_for(&config, ApiProvider::Openrouter));
    Ok(())
}

#[test]
fn managed_config_cannot_grant_external_credential_consent() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let config_path = temp_root.path().join("config.toml");
    let managed_path = temp_root.path().join("managed.toml");
    let external_path = temp_root.path().join("codex-auth.json");
    let external_raw = r#"{"tokens":{"access_token":"must-never-be-read"}}"#;
    fs::write(&external_path, external_raw)?;
    fs::write(
        &managed_path,
        format!(
            r#"[providers.openai_codex.external_credentials]
access = "read_only"
provider = "openai-codex"
source = "codex_cli"
path = {:?}
consent_version = 1
"#,
            external_path.display().to_string()
        ),
    )?;
    fs::write(
        &config_path,
        format!(
            "provider = \"openai-codex\"\nmanaged_config_path = {:?}\n",
            managed_path.display().to_string()
        ),
    )?;

    crate::external_credentials::reset_side_effect_trap();
    let config = Config::load(Some(config_path), None)?;
    assert!(
        config
            .provider_config_for(ApiProvider::OpenaiCodex)
            .and_then(|provider| provider.external_credentials.as_ref())
            .is_none()
    );
    assert!(!has_api_key_for(&config, ApiProvider::OpenaiCodex));
    assert_eq!(
        crate::external_credentials::side_effect_trap_counts(),
        (0, 0),
        "managed config must not consent to user-owned external credentials"
    );
    assert_eq!(fs::read_to_string(external_path)?, external_raw);
    Ok(())
}

#[test]
fn managed_disabled_external_policy_tightens_lower_user_consent() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let config_path = temp_root.path().join("config.toml");
    let managed_path = temp_root.path().join("managed.toml");
    let external_path = temp_root.path().join("codex-auth.json");
    fs::write(
        &external_path,
        r#"{"tokens":{"access_token":"must-not-read"}}"#,
    )?;
    fs::write(
        &managed_path,
        format!(
            r#"[providers.openai_codex.external_credentials]
access = "disabled"
provider = "openai-codex"
source = "codex_cli"
path = {:?}
consent_version = 1
"#,
            external_path.display().to_string()
        ),
    )?;
    fs::write(
        &config_path,
        format!(
            r#"provider = "openai-codex"
managed_config_path = {:?}

[providers.openai_codex]
auth_mode = "oauth"

[providers.openai_codex.external_credentials]
access = "read_only"
provider = "openai-codex"
source = "codex_cli"
path = {:?}
consent_version = 1
"#,
            managed_path.display().to_string(),
            external_path.display().to_string(),
        ),
    )?;
    let _auth_path = EnvVarGuard::set("OPENAI_CODEX_AUTH_FILE", &external_path);
    crate::external_credentials::reset_side_effect_trap();
    let config = Config::load(Some(config_path), None)?;
    let effective = config
        .provider_config_for(ApiProvider::OpenaiCodex)
        .and_then(|provider| provider.external_credentials.as_ref())
        .expect("managed disabled tombstone");
    assert_eq!(
        effective.access,
        codewhale_config::ExternalCredentialAccess::Disabled
    );
    assert!(!has_api_key_for(&config, ApiProvider::OpenaiCodex));
    assert_eq!(
        crate::external_credentials::complete_side_effect_trap_counts(),
        (0, 0, 0, 0, 0),
        "managed deny must suppress lower consent before every side effect"
    );
    Ok(())
}

#[test]
fn env_base_url_provenance_tracks_the_route_across_provider_switches() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let config_path = temp_root.path().join("config.toml");
    fs::write(
        &config_path,
        r#"provider = "openrouter"

[providers.openrouter]
api_key = "stale-openrouter-file-key"
model = "openrouter-model"

[providers.openai]
api_key = "file-openai-key"
base_url = "https://file-openai-gateway.example.test/v1"
model = "private-openai-model"

[providers.anthropic]
api_key = "stale-anthropic-file-key"
model = "claude-sonnet-5"
"#,
    )?;
    let _base = EnvVarGuard::set("CODEWHALE_BASE_URL", "https://env-gateway.example.test/v1");
    let mut config = Config::load(Some(config_path), None)?;
    assert!(config.deepseek_api_key().is_err());

    config.provider = Some("openai".to_string());
    assert_eq!(
        config.deepseek_base_url(),
        "https://file-openai-gateway.example.test/v1"
    );
    assert_eq!(config.deepseek_api_key()?, "file-openai-key");

    // Anthropic was never the route the environment addressed. Under the
    // endpoint-ownership receipt the generic override does not follow a
    // re-pointed config onto another vendor's route — that is the same
    // mechanism a pinned cross-provider child is resolved through, and it must
    // not be able to dispatch Anthropic traffic at the DeepSeek session's
    // gateway. Anthropic therefore resolves its own canonical endpoint, and
    // because it is no longer on an env-selected host its file-owned key is a
    // legitimate route-bound credential rather than one following a foreign
    // host.
    config.provider = Some("anthropic".to_string());
    assert_eq!(config.deepseek_base_url(), DEFAULT_ANTHROPIC_BASE_URL);
    assert_ne!(
        config.deepseek_base_url(),
        "https://env-gateway.example.test/v1"
    );
    assert_eq!(config.deepseek_api_key()?, "stale-anthropic-file-key");

    config.provider = Some("openrouter".to_string());
    assert!(
        config.deepseek_api_key().is_err(),
        "switching away and back must retain the env ownership receipt for that route"
    );
    Ok(())
}

#[test]
fn named_custom_api_key_env_satisfies_runtime_and_onboarding_readiness() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let _route_key = EnvVarGuard::set("NAMED_CUSTOM_ROUTE_KEY", "named-route-key");

    let mut providers = ProvidersConfig::default();
    providers.custom.insert(
        "acme".to_string(),
        ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("https://acme.example.test/v1".to_string()),
            model: Some("acme-model".to_string()),
            api_key_env: Some("NAMED_CUSTOM_ROUTE_KEY".to_string()),
            ..ProviderConfig::default()
        },
    );
    let config = Config {
        provider: Some("acme".to_string()),
        providers: Some(providers),
        ..Config::default()
    };

    assert_eq!(config.deepseek_api_key()?, "named-route-key");
    assert!(has_api_key_for(&config, ApiProvider::Custom));
    assert!(has_api_key(&config));
    Ok(())
}

#[test]
fn auth_mode_none_suppresses_config_env_secret_and_oauth_credentials() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let codewhale_home = temp_root.path().join("isolated-codewhale");
    fs::create_dir_all(&codewhale_home)?;
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
    let _ambient = EnvVarGuard::set("XAI_API_KEY", "ambient-xai-key");
    let oauth_path = temp_root.path().join("grok-auth.json");
    fs::write(&oauth_path, r#"{"access_token":"oauth-token"}"#)?;
    let _oauth_path = EnvVarGuard::set("GROK_AUTH_PATH", oauth_path.as_os_str());
    codewhale_secrets::Secrets::auto_detect().set("xai", "saved-xai-key")?;

    let mut providers = ProvidersConfig::default();
    providers.xai.auth_mode = Some("none".to_string());
    providers.xai.api_key = Some("configured-xai-key".to_string());
    providers.xai.http_headers = Some(HashMap::from([
        ("X-API-Key".to_string(), "configured-x-key".to_string()),
        ("Api-Key".to_string(), "configured-key".to_string()),
        (
            "Proxy-Authorization".to_string(),
            "Basic configured-proxy-secret".to_string(),
        ),
        (
            "X-Auth-Token".to_string(),
            "configured-auth-token".to_string(),
        ),
        (
            "X-Access-Token".to_string(),
            "configured-access-token".to_string(),
        ),
        (
            "X-Goog-Api-Key".to_string(),
            "configured-google-key".to_string(),
        ),
        ("Cookie".to_string(), "session=secret".to_string()),
        ("X-Route-Metadata".to_string(), "safe".to_string()),
    ]));
    let config = Config {
        provider: Some("xai".to_string()),
        http_headers: Some(HashMap::from([(
            "aUtHoRiZaTiOn".to_string(),
            "Bearer configured-secret".to_string(),
        )])),
        providers: Some(providers),
        ..Config::default()
    };

    assert_eq!(config.deepseek_api_key()?, "");
    assert!(
        has_api_key(&config),
        "no-auth routes are ready without a key"
    );
    assert!(!active_provider_has_config_api_key(&config));
    assert!(!active_provider_has_env_api_key(&config));
    let headers = config.http_headers();
    for name in [
        "authorization",
        "x-api-key",
        "api-key",
        "proxy-authorization",
        "x-auth-token",
        "x-access-token",
        "x-goog-api-key",
        "cookie",
    ] {
        assert!(
            !headers
                .keys()
                .any(|candidate| candidate.eq_ignore_ascii_case(name)),
            "disabled auth leaked {name}: {headers:?}"
        );
    }
    assert_eq!(
        headers.get("X-Route-Metadata").map(String::as_str),
        Some("safe")
    );
    Ok(())
}

#[test]
fn active_provider_detects_env_only_api_key() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root =
        env::temp_dir().join(format!("codewhale-tui-env-only-key-{}", std::process::id()));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    unsafe {
        env::set_var("DEEPSEEK_API_KEY", "env-only-key");
    }
    let mut config = Config::default();
    assert!(active_provider_has_env_api_key(&config));
    assert!(!active_provider_has_config_api_key(&config));
    assert!(active_provider_uses_env_only_api_key(&config));

    config.api_key = Some("config-key".to_string());
    assert!(active_provider_has_config_api_key(&config));
    assert!(!active_provider_uses_env_only_api_key(&config));

    unsafe {
        env::remove_var("DEEPSEEK_API_KEY");
    }
    Ok(())
}

#[test]
fn deepseek_api_key_ignores_sentinel_placeholder() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-sentinel-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config = Config {
        api_key: Some(API_KEYRING_SENTINEL.to_string()),
        ..Config::default()
    };
    // Sentinel must not be treated as a real key — the resolver should
    // fall through to env / config-provider and ultimately bail out
    // with a "key not found" error.
    let _err = config
        .deepseek_api_key()
        .expect_err("sentinel placeholder must not satisfy the API key check");
    Ok(())
}

#[test]
fn provider_sentinel_falls_through_to_route_env_then_fixture_store() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let codewhale_home = temp_root.path().join("isolated-codewhale");
    fs::create_dir_all(&codewhale_home)?;
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
    let config_path = temp_root.path().join("config.toml");
    let secrets = codewhale_secrets::Secrets::auto_detect();

    for sentinel in [API_KEYRING_SENTINEL, "  __KEYRING__  "] {
        fs::write(
            &config_path,
            format!("provider = \"openai\"\n\n[providers.openai]\napi_key = {sentinel:?}\n"),
        )?;
        let config = Config::load(Some(config_path.clone()), None)?;
        assert_eq!(
            config
                .provider_config()
                .and_then(|entry| entry.api_key.as_deref())
                .map(classify_config_api_key_value),
            Some(ConfigApiKeyValueKind::SecretStoreSentinel)
        );
        assert!(!active_provider_has_config_api_key(&config));
        assert!(!has_api_key_for(&config, ApiProvider::Openai));

        secrets.set("openai", "FIXTURE-STORED-KEY")?;
        assert_eq!(
            config.deepseek_api_key()?,
            "FIXTURE-STORED-KEY",
            "{sentinel:?} must fall through to the allowed fixture store"
        );
        assert!(active_provider_has_config_api_key(&config));
        assert!(has_api_key_for(&config, ApiProvider::Openai));
        secrets.delete("openai")?;

        let _route_env = EnvVarGuard::set("OFFICIAL_SENTINEL_ROUTE_KEY", "FIXTURE-ENV-KEY");
        fs::write(
            &config_path,
            format!(
                "provider = \"openai\"\n\n[providers.openai]\napi_key = {sentinel:?}\napi_key_env = \"OFFICIAL_SENTINEL_ROUTE_KEY\"\n"
            ),
        )?;
        let config = Config::load(Some(config_path.clone()), None)?;
        assert_eq!(
            config.deepseek_api_key()?,
            "FIXTURE-ENV-KEY",
            "route-bound api_key_env must outrank the store after {sentinel:?}"
        );
        assert!(!active_provider_has_config_api_key(&config));
        assert!(active_provider_has_env_api_key(&config));

        fs::write(&config_path, format!("api_key = {sentinel:?}\n"))?;
        let root = Config::load(Some(config_path.clone()), None)?;
        assert!(!active_provider_has_config_api_key(&root));
        assert!(!has_api_key_for(&root, ApiProvider::Deepseek));
        secrets.set("deepseek", "FIXTURE-DEEPSEEK-STORED-KEY")?;
        assert_eq!(
            root.deepseek_api_key()?,
            "FIXTURE-DEEPSEEK-STORED-KEY",
            "root {sentinel:?} must also fall through to the allowed fixture store"
        );
        assert!(active_provider_has_config_api_key(&root));
        secrets.delete("deepseek")?;
    }
    Ok(())
}

#[test]
fn custom_route_sentinel_is_never_a_key_and_requires_a_route_binding() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let config_path = temp_root.path().join("config.toml");

    for sentinel in [API_KEYRING_SENTINEL, "  __KEYRING__  "] {
        fs::write(
            &config_path,
            format!(
                "provider = \"acme\"\n\n[providers.acme]\nkind = \"openai-compatible\"\nbase_url = \"https://acme.example.test/v1\"\nmodel = \"acme-model\"\napi_key = {sentinel:?}\n"
            ),
        )?;
        let config = Config::load(Some(config_path.clone()), None)?;
        assert!(config.should_skip_secret_store_for_provider(ApiProvider::Custom));
        let error = config
            .deepseek_api_key()
            .expect_err("named custom sentinel must not become a bearer key");
        assert!(error.to_string().contains("must be bound explicitly"));
        assert!(!active_provider_has_config_api_key(&config));
        assert!(!has_api_key_for(&config, ApiProvider::Custom));

        let _route_env = EnvVarGuard::set("CUSTOM_SENTINEL_ROUTE_KEY", "FIXTURE-CUSTOM-ENV-KEY");
        fs::write(
            &config_path,
            format!(
                "provider = \"acme\"\n\n[providers.acme]\nkind = \"openai-compatible\"\nbase_url = \"https://acme.example.test/v1\"\nmodel = \"acme-model\"\napi_key = {sentinel:?}\napi_key_env = \"CUSTOM_SENTINEL_ROUTE_KEY\"\n"
            ),
        )?;
        let config = Config::load(Some(config_path.clone()), None)?;
        assert_eq!(config.deepseek_api_key()?, "FIXTURE-CUSTOM-ENV-KEY");
        assert!(!active_provider_has_config_api_key(&config));
        assert!(active_provider_has_env_api_key(&config));
    }

    fs::write(
        &config_path,
        format!(
            "provider = \"openrouter\"\n\n[providers.openrouter]\nbase_url = \"https://gateway.example.test/v1\"\napi_key = {API_KEYRING_SENTINEL:?}\n"
        ),
    )?;
    let custom_endpoint = Config::load(Some(config_path), None)?;
    assert!(custom_endpoint.should_skip_secret_store_for_provider(ApiProvider::Openrouter));
    assert!(custom_endpoint.deepseek_api_key().is_err());
    assert!(!active_provider_has_config_api_key(&custom_endpoint));
    assert!(!has_api_key_for(&custom_endpoint, ApiProvider::Openrouter));
    Ok(())
}

#[test]
fn default_user_paths_use_codewhale_home_for_fresh_installs() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-fresh-home-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    // EnvGuard pins DEEPSEEK_CONFIG_PATH for older tests; this test wants
    // the no-explicit-path startup behavior.
    unsafe {
        env::remove_var("DEEPSEEK_CONFIG_PATH");
    }

    let config = Config::default();
    assert_eq!(
        default_config_path().unwrap(),
        temp_root.join(".codewhale").join("config.toml")
    );
    assert_eq!(
        config.mcp_config_path(),
        temp_root.join(".codewhale").join("mcp.json")
    );
    assert_eq!(
        config.notes_path(),
        temp_root.join(".codewhale").join("notes.txt")
    );
    assert_eq!(
        config.memory_path(),
        temp_root.join(".codewhale").join("memory.md")
    );
    assert_eq!(
        config.skills_dir(),
        temp_root.join(".codewhale").join("skills")
    );

    Ok(())
}

#[test]
fn default_user_paths_preserve_existing_legacy_files() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-legacy-home-test-{}-{}",
        std::process::id(),
        nanos
    ));
    let legacy_home = temp_root.join(".deepseek");
    fs::create_dir_all(&legacy_home)?;
    for name in ["config.toml", "mcp.json", "notes.txt", "memory.md"] {
        fs::write(legacy_home.join(name), "")?;
    }
    fs::create_dir_all(legacy_home.join("skills"))?;
    let _guard = EnvGuard::new(&temp_root);

    unsafe {
        env::remove_var("DEEPSEEK_CONFIG_PATH");
    }

    let config = Config::default();
    assert_eq!(
        default_config_path().unwrap(),
        legacy_home.join("config.toml")
    );
    assert_eq!(config.mcp_config_path(), legacy_home.join("mcp.json"));
    assert_eq!(config.notes_path(), legacy_home.join("notes.txt"));
    assert_eq!(config.memory_path(), legacy_home.join("memory.md"));
    assert_eq!(config.skills_dir(), legacy_home.join("skills"));

    Ok(())
}

#[test]
fn explicit_codewhale_home_isolates_all_config_owned_user_paths() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let ambient_home = temp_root.path().join("ambient-home");
    let explicit_home = temp_root.path().join("explicit-home");
    let ambient_legacy = ambient_home.join(".deepseek");
    fs::create_dir_all(ambient_legacy.join("skills"))?;
    for name in ["mcp.json", "notes.txt", "memory.md"] {
        fs::write(ambient_legacy.join(name), "legacy")?;
    }
    let _home = EnvVarGuard::set("HOME", &ambient_home);
    let _userprofile = EnvVarGuard::set("USERPROFILE", &ambient_home);
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", &explicit_home);

    assert_eq!(default_skills_dir(), Some(explicit_home.join("skills")));
    assert_eq!(
        default_mcp_config_path(),
        Some(explicit_home.join("mcp.json"))
    );
    assert_eq!(default_notes_path(), Some(explicit_home.join("notes.txt")));
    assert_eq!(default_memory_path(), Some(explicit_home.join("memory.md")));
    Ok(())
}

#[test]
fn relative_mcp_config_path_falls_back_to_user_global_config() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let explicit_home = temp_root.path().join("codewhale-home");
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", &explicit_home);
    let config = Config {
        mcp_config_path: Some("relative/mcp.json".to_string()),
        ..Config::default()
    };

    assert_eq!(config.mcp_config_path(), explicit_home.join("mcp.json"));
    Ok(())
}

#[test]
fn absolute_mcp_config_path_remains_an_explicit_override() -> Result<()> {
    let temp_root = tempfile::tempdir()?;
    let explicit = temp_root.path().join("custom-mcp.json");
    let config = Config {
        mcp_config_path: Some(explicit.display().to_string()),
        ..Config::default()
    };

    assert_eq!(config.mcp_config_path(), explicit);
    Ok(())
}

#[test]
fn whitespace_codewhale_home_keeps_ambient_legacy_config_path_fallbacks() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let ambient_home = temp_root.path().join("ambient-home");
    let ambient_legacy = ambient_home.join(".deepseek");
    fs::create_dir_all(ambient_legacy.join("skills"))?;
    for name in ["mcp.json", "notes.txt", "memory.md"] {
        fs::write(ambient_legacy.join(name), "legacy")?;
    }
    let _home = EnvVarGuard::set("HOME", &ambient_home);
    let _userprofile = EnvVarGuard::set("USERPROFILE", &ambient_home);
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", " \t ");

    assert_eq!(default_skills_dir(), Some(ambient_legacy.join("skills")));
    assert_eq!(
        default_mcp_config_path(),
        Some(ambient_legacy.join("mcp.json"))
    );
    assert_eq!(default_notes_path(), Some(ambient_legacy.join("notes.txt")));
    assert_eq!(
        default_memory_path(),
        Some(ambient_legacy.join("memory.md"))
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn non_unicode_codewhale_home_is_preserved_by_config_owned_user_paths() -> Result<()> {
    use std::os::unix::ffi::OsStringExt;

    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let explicit_home = temp_root
        .path()
        .join(OsString::from_vec(b"codewhale-\xff-home".to_vec()));
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", &explicit_home);

    assert_eq!(default_skills_dir(), Some(explicit_home.join("skills")));
    assert_eq!(
        default_mcp_config_path(),
        Some(explicit_home.join("mcp.json"))
    );
    assert_eq!(default_notes_path(), Some(explicit_home.join("notes.txt")));
    assert_eq!(default_memory_path(), Some(explicit_home.join("memory.md")));
    Ok(())
}

#[test]
fn codewhale_config_path_env_wins_over_legacy_env() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-config-env-test-{}-{}",
        std::process::id(),
        nanos
    ));
    let preferred = temp_root.join("preferred.toml");
    let legacy = temp_root.join("legacy.toml");
    let _codewhale_config = EnvVarGuard::set("CODEWHALE_CONFIG_PATH", &preferred);
    let _legacy_config = EnvVarGuard::set("DEEPSEEK_CONFIG_PATH", &legacy);

    assert_eq!(env_config_path().unwrap().unwrap(), preferred);

    Ok(())
}

#[test]
fn test_tilde_expansion_in_paths() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-tilde-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config = Config {
        skills_dir: Some("~/.deepseek/skills".to_string()),
        ..Default::default()
    };
    let expected_skills = temp_root.join(".deepseek").join("skills");
    let actual_skills = config.skills_dir();
    assert_eq!(
        actual_skills.components().collect::<Vec<_>>(),
        expected_skills.components().collect::<Vec<_>>()
    );

    Ok(())
}

#[test]
fn skills_scan_codewhale_only_defaults_false_and_parses_true() -> Result<()> {
    assert!(!Config::default().skills_config().scan_codewhale_only());

    let config: Config = toml::from_str(
        r#"
[skills]
scan_codewhale_only = true
"#,
    )?;

    assert!(config.skills_config().scan_codewhale_only());
    Ok(())
}

#[test]
fn test_load_uses_tilde_expanded_deepseek_config_path() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-load-tilde-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".custom-deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(&config_path, "api_key = \"test-key\"\n")?;

    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe {
        env::set_var("DEEPSEEK_CONFIG_PATH", "~/.custom-deepseek/config.toml");
    }

    let config = Config::load(None, None)?;
    assert_eq!(config.api_key.as_deref(), Some("test-key"));
    Ok(())
}

#[test]
fn missing_env_config_path_does_not_fall_back_to_a_different_home_file() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-load-fallback-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let home_config = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&home_config)?;
    fs::write(&home_config, "api_key = \"home-key\"\n")?;

    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe {
        env::set_var(
            "DEEPSEEK_CONFIG_PATH",
            temp_root.join("missing-config.toml").as_os_str(),
        );
    }

    let config = Config::load(None, None)?;
    assert_eq!(
        config.api_key, None,
        "reads must honor the same missing env target that writes will create"
    );
    Ok(())
}

#[test]
fn save_then_load_uses_the_same_missing_absolute_env_config_path() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let config_path = temp_root.path().join("nested/config.toml");
    let _config_path = EnvVarGuard::set("CODEWHALE_CONFIG_PATH", &config_path);
    let _legacy_config_path = EnvVarGuard::remove("DEEPSEEK_CONFIG_PATH");
    let identity = ProviderIdentity {
        provider: ApiProvider::Openrouter,
        key: ApiProvider::Openrouter.as_str().to_string(),
        exact_id: Some(ApiProvider::Openrouter.as_str().to_string()),
        migrated_legacy_ollama_cloud_route: false,
    };

    let written =
        save_provider_model_for_identity(&identity, &Config::default(), "round-trip-model")?;
    assert_eq!(written, config_path);
    let loaded = Config::load(None, None)?;
    assert_eq!(
        loaded
            .provider_config_for(ApiProvider::Openrouter)
            .and_then(|provider| provider.model.as_deref()),
        Some("round-trip-model")
    );
    Ok(())
}

#[test]
fn relative_config_env_is_a_load_error() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let _config_path = EnvVarGuard::set("CODEWHALE_CONFIG_PATH", ".codewhale/config.toml");
    let _legacy_config_path = EnvVarGuard::remove("DEEPSEEK_CONFIG_PATH");

    let error = Config::load(None, None).expect_err("relative config path must fail closed");
    let message = format!("{error:#}");
    assert!(message.contains("CODEWHALE_CONFIG_PATH"), "{message}");
    assert!(message.contains("absolute"), "{message}");

    for error in [
        default_config_path().expect_err("default path must preserve the override error"),
        resolve_load_config_path(None)
            .expect_err("load-path helper must preserve the override error"),
        ensure_config_file_exists(None)
            .expect_err("first-run config creation must preserve the override error"),
    ] {
        let message = format!("{error:#}");
        assert!(message.contains("CODEWHALE_CONFIG_PATH"), "{message}");
        assert!(message.contains("absolute"), "{message}");
        assert!(!message.contains("home directory not found"), "{message}");
    }
    let error = env_config_path().expect_err("env helper must preserve the override error");
    let message = error.to_string();
    assert!(message.contains("CODEWHALE_CONFIG_PATH"), "{message}");
    assert!(workspace_trust_config_candidate_paths().is_empty());
    Ok(())
}

#[test]
fn test_nonexistent_profile_error() {
    let mut profiles = HashMap::new();
    profiles.insert("work".to_string(), Config::default());
    let config = ConfigFile {
        base: Config::default(),
        profiles: Some(profiles),
    };

    let err = apply_profile(config, Some("nonexistent")).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("Profile 'nonexistent' not found"));
    assert!(message.contains("Available profiles"));
    assert!(message.contains("work"));
}

#[test]
fn test_profile_with_no_profiles_section() {
    let config = ConfigFile {
        base: Config::default(),
        profiles: None,
    };

    let err = apply_profile(config, Some("missing")).unwrap_err();
    assert!(err.to_string().contains("Available profiles: none"));
}

#[test]
fn test_save_api_key_doesnt_match_similar_keys() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-api-key-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        "api_key_backup = \"old\"\napi_key = \"current\"\n",
    )?;
    let resolved_config_path = codewhale_config::resolve_config_path(None)?;

    let saved = save_api_key("new-key")?;
    assert_eq!(saved, SavedCredential::ConfigFile(resolved_config_path));

    let contents = fs::read_to_string(&config_path)?;
    assert!(contents.contains("api_key_backup = \"old\""));
    assert!(contents.contains("api_key = \""));
    Ok(())
}

#[test]
fn test_empty_api_key_rejected() {
    let config = Config {
        api_key: Some("   ".to_string()),
        ..Default::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn test_missing_api_key_allowed() -> Result<()> {
    let config = Config::default();
    config.validate()?;
    Ok(())
}

#[test]
fn apply_env_overrides_ignores_empty_api_key() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-empty-key-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    // Simulate a fresh user who copied .env.example to .env without
    // filling in DEEPSEEK_API_KEY: dotenv loads it as the empty string.
    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe {
        env::set_var("DEEPSEEK_API_KEY", "");
    }

    let mut config = Config {
        api_key: Some("from-config-file".to_string()),
        ..Default::default()
    };
    apply_env_overrides(&mut config, ConfigEnvironmentPolicy::Runtime);

    assert_eq!(config.api_key.as_deref(), Some("from-config-file"));
    config.validate()?;
    Ok(())
}

#[test]
fn apply_env_overrides_does_not_copy_api_key_into_config() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-env-key-not-config-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    unsafe {
        env::set_var("DEEPSEEK_API_KEY", "env-key");
    }
    let mut config = Config::default();
    apply_env_overrides(&mut config, ConfigEnvironmentPolicy::Runtime);

    assert_eq!(config.api_key, None);
    assert_eq!(config.deepseek_api_key()?, "env-key");
    unsafe {
        env::remove_var("DEEPSEEK_API_KEY");
    }
    Ok(())
}

#[test]
fn normalize_model_name_preserves_v_series_snapshots() {
    // v4 canonical forms still resolve
    assert_eq!(
        normalize_model_name("deepseek-v4-pro").as_deref(),
        Some("deepseek-v4-pro")
    );
    assert_eq!(
        normalize_model_name("deepseek-v4pro").as_deref(),
        Some("deepseek-v4-pro")
    );
    assert_eq!(
        normalize_model_name("pro").as_deref(),
        Some("deepseek-v4-pro")
    );
    assert_eq!(
        normalize_model_name("flash").as_deref(),
        Some("deepseek-v4-flash")
    );
    for alias in ["flash-vision", "deepseek-v4flashvisionexp"] {
        assert_eq!(
            canonical_model_name(alias),
            Some("deepseek-v4-flash-vision-exp")
        );
        assert_eq!(
            normalize_model_name(alias).as_deref(),
            Some("deepseek-v4-flash-vision-exp")
        );
        assert_eq!(
            normalize_model_name_for_provider(ApiProvider::Deepseek, alias).as_deref(),
            Some("deepseek-v4-flash-vision-exp")
        );
        assert!(validate_route(ApiProvider::Deepseek, alias).is_ok());
    }
    // v-series dated snapshots pass through unchanged
    assert_eq!(
        normalize_model_name("deepseek-v4-flash-20260423").as_deref(),
        Some("deepseek-v4-flash-20260423")
    );
    // future v-series identities pass through
    assert_eq!(
        normalize_model_name("deepseek-v5-pro-20270101").as_deref(),
        Some("deepseek-v5-pro-20270101")
    );
    // legacy names pass through unchanged — server decides
    assert_eq!(
        normalize_model_name("deepseek-chat").as_deref(),
        Some("deepseek-chat")
    );
    // cross-provider names still normalize
    assert_eq!(
        normalize_model_name("deepseek-ai/deepseek-v4-pro").as_deref(),
        Some("deepseek-ai/deepseek-v4-pro")
    );
    // preserve exact case for providers that require case-sensitive model IDs
    assert_eq!(
        normalize_model_name("DeepSeek-V4-Pro").as_deref(),
        Some("DeepSeek-V4-Pro")
    );
    assert_eq!(
        normalize_model_name("deepseek-ai/DeepSeek-V4-Pro").as_deref(),
        Some("deepseek-ai/DeepSeek-V4-Pro")
    );
}

#[test]
fn normalize_model_scenario() {
    // Scenario consolidation of: normalize_model_for_provider_keeps_provider_remaps_when_case_is_preserved, normalize_model_name_for_provider_maps_moonshot_aliases, normalize_model_name_for_provider_maps_minimax_direct_aliases, normalize_model_name_for_provider_maps_arcee_direct_aliases, normalize_model_name_rejects_invalid_or_non_deepseek_ids, normalize_model_name_accepts_provider_prefixed_deepseek_ids
    // from normalize_model_for_provider_keeps_provider_remaps_when_case_is_preserved
    {
        assert_eq!(
            normalize_model_for_provider(ApiProvider::Deepseek, "DeepSeek-V4-Pro").as_deref(),
            Some("DeepSeek-V4-Pro")
        );
        assert_eq!(
            normalize_model_for_provider(ApiProvider::NvidiaNim, "DeepSeek-V4-Pro").as_deref(),
            Some(DEFAULT_NVIDIA_NIM_MODEL)
        );
    }
    // from normalize_model_name_for_provider_maps_moonshot_aliases
    {
        for (alias, expected) in [
            ("kimi", DEFAULT_MOONSHOT_MODEL),
            ("kimi-k2.7", DEFAULT_MOONSHOT_MODEL),
            ("kimi-k2.7-code", DEFAULT_MOONSHOT_MODEL),
            ("kimi-code", DEFAULT_MOONSHOT_MODEL),
            ("kimi-k2.6", MOONSHOT_KIMI_K2_6_MODEL),
        ] {
            assert_eq!(
                normalize_model_name_for_provider(ApiProvider::Moonshot, alias).as_deref(),
                Some(expected)
            );
        }
    }
    // from normalize_model_name_for_provider_maps_minimax_direct_aliases
    {
        for (alias, expected) in [
            ("minimax", DEFAULT_MINIMAX_MODEL),
            ("minimax-m3", DEFAULT_MINIMAX_MODEL),
            ("minimax-m2.7", MINIMAX_M2_7_MODEL),
            ("minimax-m2-7-highspeed", MINIMAX_M2_7_HIGHSPEED_MODEL),
            ("minimax-m2.5", MINIMAX_M2_5_MODEL),
            ("minimax-m2-5-highspeed", MINIMAX_M2_5_HIGHSPEED_MODEL),
            ("minimax-m2.1", MINIMAX_M2_1_MODEL),
            ("minimax-m2-1-highspeed", MINIMAX_M2_1_HIGHSPEED_MODEL),
            ("minimax-m2", MINIMAX_M2_MODEL),
        ] {
            assert_eq!(
                normalize_model_name_for_provider(ApiProvider::Minimax, alias).as_deref(),
                Some(expected)
            );
        }
    }
    // from normalize_model_name_for_provider_maps_arcee_direct_aliases
    {
        for (alias, expected) in [
            ("trinity", DEFAULT_ARCEE_MODEL),
            ("arcee-trinity", DEFAULT_ARCEE_MODEL),
            ("trinity-large-thinking", DEFAULT_ARCEE_MODEL),
            ("arcee-trinity-large-thinking", DEFAULT_ARCEE_MODEL),
            ("arcee-trinity-mini", ARCEE_TRINITY_MINI_MODEL),
            ("trinity-mini", ARCEE_TRINITY_MINI_MODEL),
            (
                "arcee-trinity-large-preview",
                ARCEE_TRINITY_LARGE_PREVIEW_MODEL,
            ),
            ("TRINITY_LARGE_PREVIEW", ARCEE_TRINITY_LARGE_PREVIEW_MODEL),
        ] {
            assert_eq!(
                normalize_model_name_for_provider(ApiProvider::Arcee, alias).as_deref(),
                Some(expected)
            );
        }
    }
    // from normalize_model_name_rejects_invalid_or_non_deepseek_ids
    {
        assert!(normalize_model_name("qwen3-coder").is_none());
        assert!(normalize_model_name("codewhale v4").is_none());
        assert!(normalize_model_name("").is_none());
    }
    // from normalize_model_name_accepts_provider_prefixed_deepseek_ids
    {
        assert_eq!(
            normalize_model_name("accounts/fireworks/models/deepseek-v4-flash").as_deref(),
            Some("accounts/fireworks/models/deepseek-v4-flash")
        );
        assert_eq!(
            normalize_model_name("provider/deepseek-ai/deepseek-v4-pro").as_deref(),
            Some("provider/deepseek-ai/deepseek-v4-pro")
        );
    }
}

#[test]
fn normalize_model_name_for_provider_canonicalizes_deepseek_api_variants() {
    assert_eq!(
        normalize_model_name_for_provider(ApiProvider::Deepseek, "deepseek-ai/DeepSeek-V4-Pro")
            .as_deref(),
        Some("deepseek-v4-pro")
    );
    assert_eq!(
        normalize_model_name_for_provider(ApiProvider::Deepseek, "deepseek/deepseek-v4-flash")
            .as_deref(),
        Some("deepseek-v4-flash")
    );

    for provider in [
        ApiProvider::Deepseek,
        ApiProvider::DeepseekCN,
        ApiProvider::DeepseekAnthropic,
    ] {
        for alias in ["deepseek-chat", "deepseek-reasoner"] {
            assert_eq!(
                canonical_model_id_for_provider(provider, alias).as_deref(),
                Some(DEEPSEEK_ALIAS_REPLACEMENT),
                "{provider:?} must retire {alias} before the wire boundary"
            );
            assert_eq!(
                normalize_model_name_for_provider(provider, alias).as_deref(),
                Some(DEEPSEEK_ALIAS_REPLACEMENT),
                "{provider:?} config normalization must retire {alias}"
            );
        }
    }
}

#[test]
fn migrated_deepseek_alias_receipt_is_runtime_only_and_defaults_empty() {
    assert!(Config::default().migrated_deepseek_model_alias.is_none());

    let config: Config = toml::from_str(
        r#"
default_text_model = "deepseek-v4-flash"
migrated_deepseek_model_alias = "deepseek-chat"
"#,
    )
    .expect("deserialize config");
    assert!(config.migrated_deepseek_model_alias.is_none());
}

#[test]
fn retired_deepseek_aliases_keep_mode_intent_unless_effort_is_explicit() {
    // Model normalization reads the process-global model override. Without the
    // shared lock, env-focused config tests can replace these fixture aliases.
    let _lock = lock_test_env();
    for (alias, expected_effort) in [("deepseek-chat", "off"), ("deepseek-reasoner", "high")] {
        for provider in [
            ApiProvider::Deepseek,
            ApiProvider::DeepseekCN,
            ApiProvider::DeepseekAnthropic,
        ] {
            let mut config = Config {
                provider: Some(provider.as_str().to_string()),
                default_text_model: Some(alias.to_string()),
                ..Default::default()
            };
            normalize_model_config(&mut config);

            assert_eq!(
                config.default_text_model.as_deref(),
                Some(DEEPSEEK_ALIAS_REPLACEMENT)
            );
            assert_eq!(config.reasoning_effort.as_deref(), Some(expected_effort));
        }
    }

    let mut explicit = Config {
        provider: Some("deepseek".to_string()),
        default_text_model: Some("deepseek-chat".to_string()),
        reasoning_effort: Some("max".to_string()),
        ..Default::default()
    };
    normalize_model_config(&mut explicit);
    assert_eq!(
        explicit.default_text_model.as_deref(),
        Some(DEEPSEEK_ALIAS_REPLACEMENT)
    );
    assert_eq!(explicit.reasoning_effort.as_deref(), Some("max"));

    let mut provider_scoped = Config {
        provider: Some("deepseek-anthropic".to_string()),
        providers: Some(ProvidersConfig {
            deepseek_anthropic: ProviderConfig {
                model: Some("deepseek-reasoner".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };
    normalize_model_config(&mut provider_scoped);
    assert_eq!(
        provider_scoped
            .provider_config_for(ApiProvider::DeepseekAnthropic)
            .and_then(|entry| entry.model.as_deref()),
        Some(DEEPSEEK_ALIAS_REPLACEMENT)
    );
    assert_eq!(provider_scoped.reasoning_effort.as_deref(), Some("high"));

    let mut custom_endpoint = Config {
        provider: Some("deepseek".to_string()),
        base_url: Some("https://gateway.example/v1".to_string()),
        default_text_model: Some("deepseek-chat".to_string()),
        ..Default::default()
    };
    normalize_model_config(&mut custom_endpoint);
    assert_eq!(
        custom_endpoint.default_text_model.as_deref(),
        Some("deepseek-chat")
    );
    assert_eq!(custom_endpoint.reasoning_effort, None);
}

#[test]
fn retired_deepseek_aliases_do_not_escape_provider_owned_namespaces() {
    for provider in [
        ApiProvider::NvidiaNim,
        ApiProvider::Openrouter,
        ApiProvider::WanjieArk,
        ApiProvider::Custom,
    ] {
        for alias in ["deepseek-chat", "deepseek-reasoner"] {
            assert_eq!(
                canonical_model_id_for_provider(provider, alias).as_deref(),
                Some(alias),
                "{provider:?} owns the meaning of {alias}"
            );
        }
    }
}

#[test]
fn openrouter_hunyuan_aliases_resolve_to_hy3_preview() {
    for alias in [
        "tencent/hy3-preview",
        "hy3-preview",
        "hy3",
        "hunyuan",
        "tencent-hunyuan",
        "hunyuan-hy3",
    ] {
        assert_eq!(
            canonical_model_id_for_provider(ApiProvider::Openrouter, alias).as_deref(),
            Some(OPENROUTER_TENCENT_HY3_PREVIEW_MODEL),
            "{alias} should select the public Hy3 preview id"
        );
    }
    assert_ne!(
        canonical_model_id_for_provider(ApiProvider::Openrouter, "hy4").as_deref(),
        Some(OPENROUTER_TENCENT_HY3_PREVIEW_MODEL),
        "hy4 is not a released public model id"
    );
}

#[test]
fn deepseek_default_model_canonicalizes_provider_prefixed_ids() {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::new(temp_root.path());

    let config = Config {
        provider: Some("deepseek".to_string()),
        default_text_model: Some(DEFAULT_OPENROUTER_MODEL.to_string()),
        ..Default::default()
    };
    assert_eq!(config.default_model(), DEFAULT_TEXT_MODEL);

    let config = Config {
        provider: Some("deepseek".to_string()),
        providers: Some(ProvidersConfig {
            deepseek: ProviderConfig {
                model: Some(DEFAULT_OPENROUTER_MODEL.to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(config.default_model(), DEFAULT_TEXT_MODEL);
}

#[test]
fn requested_model_for_provider_is_permissive_off_deepseek() {
    // #3018: the provider API is the authority for non-DeepSeek routes.
    assert_eq!(
        requested_model_for_provider(ApiProvider::Moonshot, "kimi-k2.5").as_deref(),
        Some("kimi-k2.5")
    );
    assert_eq!(
        requested_model_for_provider(ApiProvider::Ollama, "qwen3:32b").as_deref(),
        Some("qwen3:32b")
    );
    // The official DeepSeek API stays strict.
    assert!(requested_model_for_provider(ApiProvider::Deepseek, "kimi-k2.5").is_none());
    assert_eq!(
        requested_model_for_provider(ApiProvider::Deepseek, "deepseek-v4-pro").as_deref(),
        Some("deepseek-v4-pro")
    );
}

#[test]
fn validate_route_rejects_mismatched_provider_model_tuple() {
    // #3227: the exact contamination — Z.ai provider paired with a
    // DeepSeek model — is rejected locally with a diagnostic that names
    // the incompatible pair, before any network call.
    let err = validate_route(ApiProvider::Zai, "deepseek-v4-pro")
        .expect_err("zai + deepseek model must be rejected");
    assert!(err.contains("deepseek-v4-pro"), "names the model: {err}");
    assert!(err.contains("zai"), "names the provider: {err}");

    // A DeepSeek-native provider rejects a non-DeepSeek model id.
    let err = validate_route(ApiProvider::Deepseek, "GLM-5.2")
        .expect_err("deepseek + GLM must be rejected");
    assert!(err.contains("GLM-5.2"), "names the model: {err}");

    // Coherent routes pass.
    assert!(validate_route(ApiProvider::Zai, "GLM-5.2").is_ok());
    assert!(validate_route(ApiProvider::Deepseek, "deepseek-v4-pro").is_ok());
    // `auto` is always acceptable; the per-turn router resolves it.
    assert!(validate_route(ApiProvider::Zai, "auto").is_ok());
    // Pass-through / aggregator providers stay permissive — the upstream
    // API remains the authority for them.
    assert!(validate_route(ApiProvider::Openai, "deepseek-v4-pro").is_ok());
    assert!(validate_route(ApiProvider::Openai, "qwen-plus").is_ok());
    assert!(validate_route(ApiProvider::Openrouter, "deepseek-v4-pro").is_ok());
    assert!(validate_route(ApiProvider::NvidiaNim, "deepseek-v4-pro").is_ok());
    assert!(validate_route(ApiProvider::Together, DEFAULT_TOGETHER_MODEL).is_ok());
    assert!(validate_route(ApiProvider::Together, DEFAULT_TOGETHER_FLASH_MODEL).is_ok());
    assert!(validate_route(ApiProvider::Together, "deepseek-v4-pro").is_ok());

    // Sakana AI (Fugu) is a native provider — DeepSeek ids must not cross-wire.
    let err = validate_route(ApiProvider::Sakana, "deepseek-v4-flash")
        .expect_err("sakana + deepseek flash must be rejected");
    assert!(err.contains("deepseek-v4-flash"), "names the model: {err}");
    assert!(err.contains("sakana"), "names the provider: {err}");
    assert!(validate_route(ApiProvider::Sakana, DEFAULT_SAKANA_MODEL).is_ok());
}

#[test]
fn wire_model_for_provider_matches_active_provider_shape() {
    assert_eq!(
        wire_model_for_provider(ApiProvider::Deepseek, DEFAULT_OPENROUTER_MODEL),
        DEFAULT_TEXT_MODEL
    );
    assert_eq!(
        wire_model_for_provider(ApiProvider::Openrouter, DEFAULT_TEXT_MODEL),
        DEFAULT_OPENROUTER_MODEL
    );
    assert_eq!(
        wire_model_for_provider(ApiProvider::NvidiaNim, DEFAULT_TEXT_MODEL),
        DEFAULT_NVIDIA_NIM_MODEL
    );
    assert_eq!(
        wire_model_for_provider(ApiProvider::Together, DEFAULT_TEXT_MODEL),
        DEFAULT_TOGETHER_MODEL
    );
    assert_eq!(
        wire_model_for_provider(ApiProvider::Together, "deepseek-v4-flash"),
        DEFAULT_TOGETHER_FLASH_MODEL
    );
    assert_eq!(
        wire_model_for_provider(ApiProvider::Together, "thinkingmachines/inkling"),
        TOGETHER_INKLING_MODEL
    );
    assert_eq!(
        wire_model_for_provider(ApiProvider::Together, "inkling"),
        TOGETHER_INKLING_MODEL
    );
    assert_eq!(
        wire_model_for_provider(ApiProvider::Together, "together-inkling"),
        TOGETHER_INKLING_MODEL
    );
    assert_eq!(
        wire_model_for_provider(ApiProvider::Openai, DEFAULT_OPENROUTER_MODEL),
        DEFAULT_OPENROUTER_MODEL
    );
    assert_eq!(
        wire_model_for_provider(ApiProvider::Openrouter, OPENROUTER_MINIMAX_M3_MODEL),
        OPENROUTER_MINIMAX_M3_MODEL
    );
    assert_eq!(
        wire_model_for_provider(ApiProvider::SiliconflowCn, DEFAULT_SILICONFLOW_MODEL),
        DEFAULT_SILICONFLOW_MODEL
    );
    assert_eq!(
        wire_model_for_provider(ApiProvider::SiliconflowCn, "deepseek-v4-pro"),
        DEFAULT_SILICONFLOW_MODEL
    );
}

#[test]
fn wire_model_route_retires_aliases_only_on_official_deepseek_endpoints() {
    for (provider, base_url) in [
        (ApiProvider::Deepseek, "https://api.deepseek.com"),
        (ApiProvider::Deepseek, "https://api.deepseek.com/v1"),
        (ApiProvider::DeepseekCN, "https://api.deepseek.com/beta"),
        (
            ApiProvider::DeepseekAnthropic,
            "https://api.deepseek.com/anthropic",
        ),
        (
            ApiProvider::DeepseekAnthropic,
            "https://api.deepseek.com/anthropic/v1/",
        ),
    ] {
        for alias in ["deepseek-chat", "deepseek-reasoner"] {
            assert_eq!(
                wire_model_for_provider_route(provider, base_url, alias),
                DEEPSEEK_ALIAS_REPLACEMENT,
                "{provider:?} {base_url} must not send {alias}"
            );
        }
    }

    for (provider, base_url, alias) in [
        (
            ApiProvider::Deepseek,
            "https://gateway.example/v1",
            "deepseek-chat",
        ),
        (
            ApiProvider::DeepseekAnthropic,
            "https://messages.example/v1",
            "deepseek-reasoner",
        ),
        (
            ApiProvider::WanjieArk,
            DEFAULT_WANJIE_ARK_BASE_URL,
            "deepseek-reasoner",
        ),
        (
            ApiProvider::NvidiaNim,
            DEFAULT_NVIDIA_NIM_BASE_URL,
            "deepseek-reasoner",
        ),
    ] {
        assert_eq!(
            wire_model_for_provider_route(provider, base_url, alias),
            alias,
            "{provider:?} owns the meaning of {alias}"
        );
    }
}

#[test]
fn normalize_model_name_for_provider_keeps_provider_specific_ids() {
    assert_eq!(
        normalize_model_name_for_provider(ApiProvider::NvidiaNim, "deepseek-v4-pro").as_deref(),
        Some(DEFAULT_NVIDIA_NIM_MODEL)
    );
    assert_eq!(
        normalize_model_name_for_provider(ApiProvider::Openrouter, "deepseek-v4-flash").as_deref(),
        Some(DEFAULT_OPENROUTER_FLASH_MODEL)
    );
    assert_eq!(
        normalize_model_name_for_provider(ApiProvider::Siliconflow, "deepseek-v4-pro").as_deref(),
        Some(DEFAULT_SILICONFLOW_MODEL)
    );
    assert_eq!(
        normalize_model_name_for_provider(ApiProvider::Siliconflow, "deepseek-reasoner").as_deref(),
        Some(DEFAULT_SILICONFLOW_MODEL)
    );
    assert_eq!(
        normalize_model_name_for_provider(ApiProvider::Siliconflow, "deepseek-r1").as_deref(),
        Some(DEFAULT_SILICONFLOW_MODEL)
    );
    assert_eq!(
        normalize_model_name_for_provider(ApiProvider::SiliconflowCn, "deepseek-reasoner")
            .as_deref(),
        Some(DEFAULT_SILICONFLOW_MODEL)
    );
    assert_eq!(
        normalize_model_name_for_provider(ApiProvider::Siliconflow, "deepseek-chat").as_deref(),
        Some(DEFAULT_SILICONFLOW_FLASH_MODEL)
    );
    assert_eq!(
        normalize_model_name_for_provider(ApiProvider::SiliconflowCn, "deepseek-chat").as_deref(),
        Some(DEFAULT_SILICONFLOW_FLASH_MODEL)
    );
    assert_eq!(
        normalize_model_name_for_provider(ApiProvider::Siliconflow, "deepseek-v3").as_deref(),
        Some(DEFAULT_SILICONFLOW_FLASH_MODEL)
    );
    assert_eq!(
        normalize_model_name_for_provider(ApiProvider::Siliconflow, "deepseek-v3.2").as_deref(),
        Some("deepseek-v3.2")
    );
    assert_eq!(
        normalize_model_name_for_provider(ApiProvider::Together, "deepseek-v4-pro").as_deref(),
        Some(DEFAULT_TOGETHER_MODEL)
    );
    assert_eq!(
        normalize_model_name_for_provider(ApiProvider::Together, "deepseek-chat").as_deref(),
        Some(DEFAULT_TOGETHER_FLASH_MODEL)
    );
}

#[test]
fn normalize_model_name_for_provider_maps_recent_openrouter_aliases() {
    for (alias, expected) in [
        (
            "trinity-large-thinking",
            OPENROUTER_ARCEE_TRINITY_LARGE_THINKING_MODEL,
        ),
        ("qwen3.6-flash", OPENROUTER_QWEN_3_6_FLASH_MODEL),
        ("qwen3.6-35b-a3b", OPENROUTER_QWEN_3_6_35B_A3B_MODEL),
        ("qwen3.6-max-preview", OPENROUTER_QWEN_3_6_MAX_PREVIEW_MODEL),
        ("qwen3.6-plus", OPENROUTER_QWEN_3_6_PLUS_MODEL),
        ("qwen3.7-plus", OPENROUTER_QWEN_3_7_PLUS_MODEL),
        ("qwen-3.7-plus", OPENROUTER_QWEN_3_7_PLUS_MODEL),
        ("mimo-v2.5-pro", OPENROUTER_XIAOMI_MIMO_V2_5_PRO_MODEL),
        ("kimi-k2.7-code", OPENROUTER_KIMI_K2_7_CODE_MODEL),
        ("kimi", OPENROUTER_KIMI_K2_7_CODE_MODEL),
        ("kimi-k2.6", OPENROUTER_KIMI_K2_6_MODEL),
        ("minimax-m3", OPENROUTER_MINIMAX_M3_MODEL),
        ("minimax-2.7", OPENROUTER_MINIMAX_M2_7_MODEL),
        ("gemma-4-31b-it", OPENROUTER_GEMMA_4_31B_MODEL),
        ("glm-5.1", OPENROUTER_GLM_5_1_MODEL),
        ("glm-5.2", OPENROUTER_GLM_5_2_MODEL),
    ] {
        assert_eq!(
            normalize_model_name_for_provider(ApiProvider::Openrouter, alias).as_deref(),
            Some(expected)
        );
    }
}

#[test]
fn normalize_xiaomi_mimo_aliases_for_provider() {
    assert_eq!(
        normalize_model_name_for_provider(ApiProvider::XiaomiMimo, "omni").as_deref(),
        Some("mimo-v2.5")
    );
    assert_eq!(
        normalize_model_name_for_provider(ApiProvider::XiaomiMimo, "tts").as_deref(),
        Some("mimo-v2.5-tts")
    );
    assert_eq!(
        normalize_model_name_for_provider(ApiProvider::XiaomiMimo, "voice-design").as_deref(),
        Some("mimo-v2.5-tts-voicedesign")
    );
    assert_eq!(
        wire_model_for_provider(ApiProvider::XiaomiMimo, "voiceclone"),
        "mimo-v2.5-tts-voiceclone"
    );
}

#[test]
fn model_completion_scenario() {
    // Scenario consolidation of: model_completion_names_for_xiaomi_mimo_include_chat_models, model_completion_names_for_deepseek_api_are_deduplicated_bare_ids, model_completion_names_for_openai_are_native_to_its_default_endpoint, model_completion_names_for_atlascloud_keep_its_provider_owned_default, model_completion_names_for_together_include_provider_owned_models, model_completion_names_for_wanjie_keep_legacy_default_and_v4_ids, model_completion_names_for_ollama_do_not_promote_static_remote_models, model_completion_names_for_openrouter_include_recent_large_models
    // from model_completion_names_for_xiaomi_mimo_include_chat_models
    {
        let models = model_completion_names_for_provider(ApiProvider::XiaomiMimo);
        for expected in ["mimo-v2.5-pro", "mimo-v2.5"] {
            assert!(models.contains(&expected), "missing {expected}");
        }
        for deprecated in ["mimo-v2-pro", "mimo-v2-omni", "mimo-v2-flash"] {
            assert!(
                !models.contains(&deprecated),
                "{deprecated} is deprecated and should not be promoted"
            );
        }
        for speech_model in [
            "mimo-v2.5-tts",
            "mimo-v2.5-tts-voicedesign",
            "mimo-v2.5-tts-voiceclone",
            "mimo-v2-tts",
        ] {
            assert!(
                !models.contains(&speech_model),
                "{speech_model} belongs in speech/TTS selection, not /model"
            );
        }
    }
    // from model_completion_names_for_deepseek_api_are_deduplicated_bare_ids
    {
        assert_eq!(
            model_completion_names_for_provider(ApiProvider::Deepseek),
            vec![
                "deepseek-v4-pro",
                "deepseek-v4-flash",
                "deepseek-v4-flash-vision-exp"
            ]
        );
    }
    // from model_completion_names_for_openai_are_native_to_its_default_endpoint
    {
        let models = model_completion_names_for_provider(ApiProvider::Openai);

        assert_eq!(models.first().copied(), Some("gpt-5.6"));
        assert!(models.iter().all(|model| !model.contains("deepseek")));
    }
    // from model_completion_names_for_atlascloud_keep_its_provider_owned_default
    {
        assert_eq!(
            model_completion_names_for_provider(ApiProvider::Atlascloud),
            vec![DEFAULT_ATLASCLOUD_MODEL]
        );
    }
    // from model_completion_names_for_together_include_provider_owned_models
    {
        assert_eq!(
            model_completion_names_for_provider(ApiProvider::Together),
            vec![DEFAULT_TOGETHER_MODEL, DEFAULT_TOGETHER_FLASH_MODEL]
        );
    }
    // from model_completion_names_for_wanjie_keep_legacy_default_and_v4_ids
    {
        let models = model_completion_names_for_provider(ApiProvider::WanjieArk);

        assert_eq!(models.first().copied(), Some(DEFAULT_WANJIE_ARK_MODEL));
        assert!(models.contains(&"deepseek-v4-pro"));
        assert!(models.contains(&"deepseek-v4-flash"));
    }
    // from model_completion_names_for_ollama_do_not_promote_static_remote_models
    {
        let models = model_completion_names_for_provider(ApiProvider::Ollama);

        assert!(models.is_empty());
    }
    // from model_completion_names_for_openrouter_include_recent_large_models
    {
        let models = model_completion_names_for_provider(ApiProvider::Openrouter);

        for expected in [
            DEFAULT_OPENROUTER_MODEL,
            DEFAULT_OPENROUTER_FLASH_MODEL,
            OPENROUTER_ARCEE_TRINITY_LARGE_THINKING_MODEL,
            OPENROUTER_XIAOMI_MIMO_V2_5_PRO_MODEL,
            OPENROUTER_MINIMAX_M3_MODEL,
            OPENROUTER_MINIMAX_M2_7_MODEL,
            OPENROUTER_QWEN_3_6_FLASH_MODEL,
            OPENROUTER_QWEN_3_6_35B_A3B_MODEL,
            OPENROUTER_QWEN_3_6_MAX_PREVIEW_MODEL,
            OPENROUTER_QWEN_3_6_27B_MODEL,
            OPENROUTER_QWEN_3_6_PLUS_MODEL,
            OPENROUTER_GLM_5_1_MODEL,
            OPENROUTER_GLM_5_2_MODEL,
            OPENROUTER_GEMMA_4_31B_MODEL,
        ] {
            assert!(models.contains(&expected), "missing {expected}");
        }
    }
}

#[test]
fn model_completion_scenario_2() {
    // Scenario consolidation of: model_completion_names_for_moonshot_uses_latest_platform_model, model_completion_names_for_zai_lists_default_5_1_and_turbo, model_completion_names_for_minimax_include_direct_chat_models, model_completion_names_for_minimax_anthropic_include_target_models, model_completion_names_for_sakana_include_fugu_models
    // from model_completion_names_for_moonshot_uses_latest_platform_model
    {
        let models = model_completion_names_for_provider(ApiProvider::Moonshot);

        assert_eq!(models.first().copied(), Some(DEFAULT_MOONSHOT_MODEL));
        // `kimi-k3` is served by this provider's default (direct platform) route
        // and must be offerable — a dogfood user on v0.9.1 could not find it.
        assert!(models.contains(&MOONSHOT_KIMI_K3_MODEL), "{models:?}");
        // The Kimi Code coding-plan ids belong to api.kimi.com/coding/v1, which
        // this base-URL-less list cannot express. Offering them here would
        // advertise a pairing `validate_kimi_code_api_model_id` rejects.
        assert!(!models.contains(&KIMI_CODE_K3_MODEL), "{models:?}");
        assert!(!models.contains(&DEFAULT_KIMI_CODE_MODEL), "{models:?}");
        for model in &models {
            let config = Config {
                provider: Some(ApiProvider::Moonshot.as_str().to_string()),
                default_text_model: Some((*model).to_string()),
                ..Default::default()
            };
            config
                .validate()
                .expect("every advertised Moonshot model must be valid on its default route");
        }
    }
    // from model_completion_names_for_zai_lists_default_5_1_and_turbo
    {
        let models = model_completion_names_for_provider(ApiProvider::Zai);

        // GLM-5.3 is the default and must be first; GLM-5.2 and GLM-5.1 stay
        // available, and GLM-5-Turbo is the faster sub-agent sibling.
        assert_eq!(models.first().copied(), Some(DEFAULT_ZAI_MODEL));
        assert_eq!(DEFAULT_ZAI_MODEL, ZAI_GLM_5_3_MODEL);
        assert!(models.contains(&ZAI_GLM_5_1_MODEL));
        assert!(models.contains(&ZAI_GLM_5_3_FLASH_MODEL));
        assert!(models.contains(&ZAI_GLM_5_TURBO_MODEL));
        // GLM-5.2 is still offered alongside the others but no longer takes the
        // default slot; explicit 5.2 routes are untouched.
        assert!(models.contains(&ZAI_GLM_5_2_MODEL));
        assert_ne!(models.first().copied(), Some(ZAI_GLM_5_2_MODEL));
        // No accidental duplicate entries.
        let mut sorted = models.to_vec();
        sorted.sort_unstable();
        let mut deduped = sorted.clone();
        deduped.dedup();
        assert_eq!(sorted, deduped);
    }
    // from model_completion_names_for_minimax_include_direct_chat_models
    {
        let models = model_completion_names_for_provider(ApiProvider::Minimax);

        for expected in [
            DEFAULT_MINIMAX_MODEL,
            MINIMAX_M2_7_MODEL,
            MINIMAX_M2_7_HIGHSPEED_MODEL,
            MINIMAX_M2_5_MODEL,
            MINIMAX_M2_5_HIGHSPEED_MODEL,
            MINIMAX_M2_1_MODEL,
            MINIMAX_M2_1_HIGHSPEED_MODEL,
            MINIMAX_M2_MODEL,
        ] {
            assert!(models.contains(&expected), "missing {expected}");
        }
        assert!(
            !models.contains(&OPENROUTER_MINIMAX_M3_MODEL),
            "direct MiniMax picker must not expose OpenRouter namespaced IDs"
        );
    }
    // from model_completion_names_for_minimax_anthropic_include_target_models
    {
        let models = model_completion_names_for_provider(ApiProvider::MinimaxAnthropic);

        assert!(models.contains(&DEFAULT_MINIMAX_MODEL));
        assert!(models.contains(&MINIMAX_M2_7_MODEL));
    }
    // from model_completion_names_for_sakana_include_fugu_models
    {
        assert_eq!(
            model_completion_names_for_provider(ApiProvider::Sakana),
            vec![DEFAULT_SAKANA_MODEL, SAKANA_FUGU_ULTRA_MODEL]
        );
    }
}

#[test]
fn normalize_model_name_for_zai_canonicalizes_current_glm_models() {
    for (alias, expected) in [
        ("glm-5.1", ZAI_GLM_5_1_MODEL),
        ("glm-5-1", ZAI_GLM_5_1_MODEL),
        ("glm-5.2", ZAI_GLM_5_2_MODEL),
        ("zai-glm-5-2", ZAI_GLM_5_2_MODEL),
        ("glm-5.3", DEFAULT_ZAI_MODEL),
        ("glm-5-3", ZAI_GLM_5_3_MODEL),
        ("zai-glm-5-3", ZAI_GLM_5_3_MODEL),
        ("glm-5.3-flash", ZAI_GLM_5_3_FLASH_MODEL),
        ("glm-5-3-flash", ZAI_GLM_5_3_FLASH_MODEL),
        ("zai-glm-5.3-flash", ZAI_GLM_5_3_FLASH_MODEL),
        ("glm-5-turbo", ZAI_GLM_5_TURBO_MODEL),
        ("zai-glm-5-turbo", ZAI_GLM_5_TURBO_MODEL),
    ] {
        assert_eq!(
            normalize_model_name_for_provider(ApiProvider::Zai, alias).as_deref(),
            Some(expected)
        );
    }
    // The 5.1-era bug shape: an alias silently resolving to the provider
    // default. Now that GLM-5.3 is the default, GLM-5.2 must keep its own id.
    assert_ne!(ZAI_GLM_5_2_MODEL, DEFAULT_ZAI_MODEL);
    assert_eq!(
        normalize_model_name_for_provider(ApiProvider::Zai, "glm-5.2").as_deref(),
        Some(ZAI_GLM_5_2_MODEL)
    );
    assert_eq!(
        normalize_model_name_for_provider(ApiProvider::Zai, "glm-next-preview").as_deref(),
        Some("glm-next-preview")
    );
}

#[test]
fn opencode_go_config_uses_only_current_chat_completions_models() -> Result<()> {
    let _lock = lock_test_env();
    let _api_key = EnvVarGuard::remove("OPENCODE_GO_API_KEY");
    let _base_url = EnvVarGuard::remove("OPENCODE_GO_BASE_URL");
    let _model = EnvVarGuard::remove("OPENCODE_GO_MODEL");

    let config: Config = toml::from_str(
        r#"
provider = "opencode_go"

[providers.opencode_go]
api_key = "go-config-key"
model = "opencode-go/glm-5.2"
"#,
    )?;

    assert_eq!(config.api_provider(), ApiProvider::OpencodeGo);
    assert_eq!(config.deepseek_base_url(), DEFAULT_OPENCODE_GO_BASE_URL);
    assert_eq!(config.default_model(), "glm-5.2");
    assert_eq!(config.deepseek_api_key()?, "go-config-key");
    assert_eq!(
        wire_model_for_provider(ApiProvider::OpencodeGo, "opencode-go/mimo-v2.5-pro"),
        "mimo-v2.5-pro"
    );
    assert_eq!(
        model_completion_names_for_provider(ApiProvider::OpencodeGo),
        OPENCODE_GO_CHAT_MODELS.to_vec()
    );
    for chat_model in OPENCODE_GO_CHAT_MODELS {
        assert_eq!(
            canonical_model_id_for_provider(ApiProvider::OpencodeGo, chat_model).as_deref(),
            Some(*chat_model)
        );
        assert!(validate_route(ApiProvider::OpencodeGo, chat_model).is_ok());
    }
    for messages_only in [
        "minimax-m3",
        "minimax-m2.7",
        "minimax-m2.5",
        "qwen3.7-max",
        "qwen3.7-plus",
        "qwen3.6-plus",
    ] {
        assert!(
            !model_completion_names_for_provider(ApiProvider::OpencodeGo).contains(&messages_only),
            "{messages_only} uses the Messages endpoint and must not be advertised"
        );
        assert!(
            canonical_model_id_for_provider(ApiProvider::OpencodeGo, messages_only).is_none(),
            "{messages_only} must not pass the explicit selector gate"
        );
        assert!(
            requested_model_for_provider(ApiProvider::OpencodeGo, messages_only).is_none(),
            "{messages_only} must not pass the runtime request gate"
        );
        assert!(validate_route(ApiProvider::OpencodeGo, messages_only).is_err());
        // Never substitute a different model. Keep the caller's spelling so
        // validate_route / the route resolver can reject by name. A base URL
        // override still cannot promote a Messages-only id onto Chat Completions.
        assert_eq!(
            wire_model_for_provider(ApiProvider::OpencodeGo, messages_only),
            messages_only,
            "must not silently rewrite {messages_only} to the Chat default"
        );
        assert_eq!(
            wire_model_for_provider_route(
                ApiProvider::OpencodeGo,
                "https://go-gateway.example/v1",
                messages_only,
            ),
            messages_only,
            "a base URL override must not rewrite or re-admit {messages_only}"
        );
    }

    Ok(())
}

#[test]
fn default_context_seams_are_opt_in() {
    let config = Config::default();
    assert!(!config.context.enabled.unwrap_or(false));
    assert_eq!(config.context.l1_threshold.unwrap_or(192_000), 192_000);
    assert_eq!(
        config
            .context
            .seam_model
            .as_deref()
            .unwrap_or("deepseek-v4-flash"),
        "deepseek-v4-flash"
    );
}

#[test]
fn profile_skills_config_merges_individual_fields() {
    let mut profiles = HashMap::new();
    profiles.insert(
        "strict".to_string(),
        Config {
            skills: Some(SkillsConfig {
                scan_codewhale_only: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    let config = ConfigFile {
        base: Config {
            skills: Some(SkillsConfig {
                registry_url: Some("https://registry.example/skills.json".to_string()),
                max_install_size_bytes: Some(1234),
                ..Default::default()
            }),
            ..Default::default()
        },
        profiles: Some(profiles),
    };

    let merged = apply_profile(config, Some("strict")).expect("profile");
    let skills = merged.skills.expect("merged skills config");
    assert_eq!(
        skills.registry_url.as_deref(),
        Some("https://registry.example/skills.json")
    );
    assert_eq!(skills.max_install_size_bytes, Some(1234));
    assert_eq!(skills.scan_codewhale_only, Some(true));
}

#[test]
fn removed_context_per_model_table_is_ignored_for_compatibility() -> Result<()> {
    let parsed: ConfigFile = toml::from_str(
        r#"
        [context]
        enabled = true

        [context.per_model.deepseek-v4-pro]
        l1_threshold = 111
        l2_threshold = 222
        l3_threshold = 333
        "#,
    )?;

    assert_eq!(parsed.base.context.enabled, Some(true));
    Ok(())
}

#[test]
fn project_context_pack_defaults_off_and_can_be_enabled() {
    // #4781: project context pack is opt-in (large pretty-printed tree).
    let mut config = Config::default();
    assert!(!config.project_context_pack_enabled());

    config.context.project_pack = Some(true);
    assert!(config.project_context_pack_enabled());

    config.context.project_pack = Some(false);
    assert!(!config.project_context_pack_enabled());
}

#[test]
fn validate_accepts_scenario() -> Result<()> {
    // Scenario consolidation of: validate_accepts_future_deepseek_model_id, validate_accepts_auto_default_text_model
    // from validate_accepts_future_deepseek_model_id
    {
        let config = Config {
            default_text_model: Some("deepseek-v4".to_string()),
            ..Default::default()
        };
        config.validate()?;
    }
    // from validate_accepts_auto_default_text_model
    {
        let config = Config {
            default_text_model: Some("auto".to_string()),
            ..Default::default()
        };
        config.validate()?;
        assert_eq!(config.default_model(), "auto");
    }
    Ok(())
}

#[test]
fn deepseek_provider_defaults_to_beta_endpoint() {
    let config = Config::default();

    assert_eq!(config.api_provider(), ApiProvider::Deepseek);
    assert_eq!(config.deepseek_base_url(), DEFAULT_DEEPSEEK_BASE_URL);
}

#[test]
fn explicit_deepseek_base_url_overrides_beta_default() {
    let config = Config {
        base_url: Some("https://api.deepseek.com".to_string()),
        ..Default::default()
    };

    assert_eq!(config.api_provider(), ApiProvider::Deepseek);
    assert_eq!(config.deepseek_base_url(), "https://api.deepseek.com");
}

#[test]
fn loopback_deepseek_base_url_runs_without_api_key() -> Result<()> {
    let _lock = lock_test_env();
    let config = Config {
        base_url: Some("http://127.0.0.1:8000/v1".to_string()),
        ..Default::default()
    };

    assert_eq!(config.api_provider(), ApiProvider::Deepseek);
    assert!(has_api_key(&config));
    assert_eq!(config.deepseek_api_key()?, "");
    Ok(())
}

#[test]
fn deepseek_model_env_overrides_default_text_model() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-model-env-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe {
        env::set_var("DEEPSEEK_MODEL", "deepseek-v4-flash-20260423");
    }

    let config = Config::load(None, None)?;
    // v-series snapshots pass through unchanged — no alias folding
    assert_eq!(
        config.default_text_model.as_deref(),
        Some("deepseek-v4-flash-20260423")
    );
    Ok(())
}

#[test]
fn retired_deepseek_aliases_from_env_are_migrated_before_runtime() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _managed_config = crate::test_support::EnvVarGuard::set(
        "DEEPSEEK_MANAGED_CONFIG_PATH",
        temp_root.path().join("missing-managed.toml"),
    );

    for (provider, alias, expected_effort) in [
        ("deepseek", "deepseek-chat", "off"),
        ("deepseek-cn", "deepseek-reasoner", "high"),
        ("deepseek-anthropic", "deepseek-chat", "off"),
    ] {
        let _guard = EnvGuard::new(temp_root.path());
        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var("CODEWHALE_PROVIDER", provider);
            env::set_var("CODEWHALE_MODEL", alias);
        }

        // Pass the isolated path explicitly: the process-wide default config
        // path is cached by earlier tests and can otherwise point back at the
        // developer's real provider-scoped model.
        let config = Config::load(
            Some(temp_root.path().join("isolated-alias-config.toml")),
            None,
        )?;
        assert_eq!(
            config.default_model(),
            DEEPSEEK_ALIAS_REPLACEMENT,
            "provider={provider} resolved={:?} root_model={:?} scoped_model={:?}",
            config.api_provider(),
            config.default_text_model,
            config
                .provider_config_for(config.api_provider())
                .and_then(|entry| entry.model.as_deref())
        );
        assert_eq!(config.reasoning_effort(), Some(expected_effort));
        let deprecation = config
            .active_deepseek_alias_deprecation()
            .expect("loaded config should retain the alias migration receipt");
        assert_eq!(deprecation.alias, alias);
        assert_eq!(deprecation.replacement, DEEPSEEK_ALIAS_REPLACEMENT);
    }

    Ok(())
}

#[test]
fn http_headers_load_from_root_config() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-http-headers-root-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"
api_key = "test-key"
http_headers = { "X-Model-Provider-Id" = "tongyi" }
"#,
    )?;

    let config = Config::load(None, None)?;
    assert_eq!(
        config
            .http_headers()
            .get("X-Model-Provider-Id")
            .map(String::as_str),
        Some("tongyi")
    );
    Ok(())
}

#[test]
fn provider_http_headers_extend_and_override_root_config() {
    let mut providers = ProvidersConfig::default();
    providers.deepseek.http_headers = Some(HashMap::from([
        ("X-Model-Provider-Id".to_string(), "tongyi".to_string()),
        ("X-Shared".to_string(), "provider".to_string()),
    ]));
    let config = Config {
        http_headers: Some(HashMap::from([
            ("X-Root".to_string(), "root".to_string()),
            ("X-Shared".to_string(), "root".to_string()),
        ])),
        providers: Some(providers),
        ..Default::default()
    };

    let headers = config.http_headers();
    assert_eq!(
        headers.get("X-Model-Provider-Id").map(String::as_str),
        Some("tongyi")
    );
    assert_eq!(headers.get("X-Root").map(String::as_str), Some("root"));
    assert_eq!(
        headers.get("X-Shared").map(String::as_str),
        Some("provider")
    );
}

#[test]
fn http_headers_env_overrides_config() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-http-headers-env-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"
api_key = "test-key"
http_headers = { "X-Model-Provider-Id" = "from-file" }
"#,
    )?;
    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe {
        env::set_var("DEEPSEEK_HTTP_HEADERS", "X-Model-Provider-Id=from-env");
    }

    let config = Config::load(None, None)?;
    assert_eq!(
        config
            .http_headers()
            .get("X-Model-Provider-Id")
            .map(String::as_str),
        Some("from-env")
    );
    Ok(())
}

#[test]
fn nvidia_nim_scenario() -> Result<()> {
    // Scenario consolidation of: nvidia_nim_provider_uses_nim_defaults, nvidia_nim_provider_normalizes_deepseek_v4_flash_alias, nvidia_nim_env_accepts_short_nim_base_url_alias
    // from nvidia_nim_provider_uses_nim_defaults
    {
        let config = Config {
            provider: Some("nvidia-nim".to_string()),
            ..Default::default()
        };

        config.validate()?;
        assert_eq!(config.api_provider(), ApiProvider::NvidiaNim);
        assert_eq!(config.default_model(), DEFAULT_NVIDIA_NIM_MODEL);
        assert_eq!(config.deepseek_base_url(), DEFAULT_NVIDIA_NIM_BASE_URL);
    }
    // from nvidia_nim_provider_normalizes_deepseek_v4_flash_alias
    {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-nim-flash-model-alias-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config = Config {
            provider: Some("nvidia-nim".to_string()),
            default_text_model: Some("deepseek-v4-flash".to_string()),
            ..Default::default()
        };

        config.validate()?;
        assert_eq!(config.default_model(), DEFAULT_NVIDIA_NIM_FLASH_MODEL);
    }
    // from nvidia_nim_env_accepts_short_nim_base_url_alias
    {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-nim-base-url-alias-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "nvidia-nim");
            env::set_var("NIM_BASE_URL", "https://short-nim.example/v1");
        }

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::NvidiaNim);
        assert_eq!(config.deepseek_base_url(), "https://short-nim.example/v1");
    }
    Ok(())
}

#[test]
fn nvidia_nim_provider_normalizes_deepseek_v4_pro_alias() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-nim-model-alias-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        "provider = \"nvidia-nim\"\ndefault_text_model = \"deepseek-v4-pro\"\napi_key = \"nim-key\"\n",
    )?;

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::NvidiaNim);
    assert_eq!(
        config.default_text_model.as_deref(),
        Some(DEFAULT_NVIDIA_NIM_MODEL)
    );
    Ok(())
}

#[test]
fn vendor_locked_providers_reject_foreign_root_default_model() {
    let _lock = lock_test_env();
    for (provider, expected) in [
        ("xai", DEFAULT_XAI_MODEL),
        ("openai", DEFAULT_OPENAI_MODEL),
        ("moonshot", DEFAULT_MOONSHOT_MODEL),
        ("mistral", DEFAULT_MISTRAL_MODEL),
    ] {
        let config = Config {
            provider: Some(provider.to_string()),
            default_text_model: Some("deepseek-v4-pro".to_string()),
            ..Default::default()
        };
        assert_eq!(
            config.default_model(),
            expected,
            "a root DeepSeek default must not leak onto the official {provider} endpoint"
        );
    }
}

#[test]
fn mistral_model_env_overrides_vendor_default() {
    let _lock = lock_test_env();
    let _model = EnvVarGuard::set("MISTRAL_MODEL", "mistral-medium-latest");
    let _generic_model = EnvVarGuard::remove("CODEWHALE_MODEL");
    let _legacy_model = EnvVarGuard::remove("DEEPSEEK_MODEL");
    let mut config = Config {
        provider: Some("mistral".to_string()),
        ..Config::default()
    };

    apply_env_overrides(&mut config, ConfigEnvironmentPolicy::Runtime);

    assert_eq!(config.api_provider(), ApiProvider::Mistral);
    assert_eq!(config.default_model(), "mistral-medium-latest");
}

#[test]
fn codewhale_model_precedes_mistral_model() {
    let _lock = lock_test_env();
    let _provider_model = EnvVarGuard::set("MISTRAL_MODEL", "mistral-small-latest");
    let _generic_model = EnvVarGuard::set("CODEWHALE_MODEL", "mistral-medium-latest");
    let _legacy_model = EnvVarGuard::remove("DEEPSEEK_MODEL");
    let mut config = Config {
        provider: Some("mistral".to_string()),
        ..Config::default()
    };

    apply_env_overrides(&mut config, ConfigEnvironmentPolicy::Runtime);

    assert_eq!(config.default_model(), "mistral-medium-latest");
}

#[test]
fn xai_custom_endpoint_keeps_root_default_model_pass_through() {
    let _lock = lock_test_env();
    let mut providers = ProvidersConfig::default();
    providers.xai.base_url = Some("https://proxy.example.test/v1".to_string());
    let config = Config {
        provider: Some("xai".to_string()),
        default_text_model: Some("deepseek-v4-pro".to_string()),
        providers: Some(providers),
        ..Default::default()
    };
    assert_eq!(
        config.default_model(),
        "deepseek-v4-pro",
        "custom compatible endpoints may serve any model id"
    );
}

#[test]
fn xai_explicit_provider_model_is_honored_over_vendor_default() {
    let _lock = lock_test_env();
    let mut providers = ProvidersConfig::default();
    providers.xai.model = Some("grok-4.5-mini".to_string());
    let config = Config {
        provider: Some("xai".to_string()),
        default_text_model: Some("deepseek-v4-pro".to_string()),
        providers: Some(providers),
        ..Default::default()
    };
    assert_eq!(config.default_model(), "grok-4.5-mini");
}

#[test]
fn nvidia_nim_env_overrides_provider_and_credentials() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-nim-env-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "nvidia-nim");
        env::set_var("NVIDIA_API_KEY", "nim-env-key");
        env::set_var("NVIDIA_NIM_MODEL", "deepseek-ai/deepseek-v4-pro");
    }

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::NvidiaNim);
    assert_eq!(config.deepseek_api_key()?, "nim-env-key");
    assert_eq!(config.default_model(), DEFAULT_NVIDIA_NIM_MODEL);
    Ok(())
}

#[test]
fn nvidia_nim_env_accepts_facade_base_url_forwarding() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-nim-forwarded-base-url-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "nvidia-nim");
        env::set_var("DEEPSEEK_BASE_URL", "https://forwarded-nim.example/v1");
    }

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::NvidiaNim);
    assert_eq!(
        config.deepseek_base_url(),
        "https://forwarded-nim.example/v1"
    );
    Ok(())
}

#[test]
fn openai_provider_uses_openai_compatible_defaults() -> Result<()> {
    let config = Config {
        provider: Some("openai".to_string()),
        ..Default::default()
    };

    config.validate()?;
    assert_eq!(config.api_provider(), ApiProvider::Openai);
    assert_eq!(config.default_model(), "gpt-5.6");
    assert_eq!(config.deepseek_base_url(), "https://api.openai.com/v1");
    assert_eq!(
        codewhale_config::provider::provider_for_kind(codewhale_config::ProviderKind::Openai)
            .default_model(),
        DEFAULT_OPENAI_MODEL
    );
    Ok(())
}

#[test]
fn openai_codex_default_model_falls_back_to_codex_model() {
    // The Codex Responses backend only accepts its own model family, and a
    // global `default_text_model` is validated to DeepSeek IDs (or "auto"),
    // so with the Codex provider it must resolve to the Codex default
    // instead of leaking a DeepSeek id the backend rejects.
    // Isolate from any ambient developer Codex roster: the seed fallback is
    // asserted here; the fresh-roster preference (#5034) is covered by
    // codex_switch_without_saved_model_prefers_fresh_roster_head.
    let _lock = lock_test_env();
    let empty_codex_home = tempfile::tempdir().expect("empty codex home");
    let _codex_home = EnvVarGuard::set("CODEX_HOME", empty_codex_home.path());
    let with_deepseek_default = Config {
        provider: Some("openai-codex".to_string()),
        default_text_model: Some(DEFAULT_TEXT_MODEL.to_string()),
        ..Default::default()
    };
    assert_eq!(
        with_deepseek_default.api_provider(),
        ApiProvider::OpenaiCodex
    );
    assert_eq!(
        with_deepseek_default.default_model(),
        DEFAULT_OPENAI_CODEX_MODEL
    );

    // No global default resolves the same way.
    let bare = Config {
        provider: Some("openai-codex".to_string()),
        ..Default::default()
    };
    assert_eq!(bare.default_model(), DEFAULT_OPENAI_CODEX_MODEL);

    // An explicit provider-scoped model still wins over the fallback.
    let mut providers = ProvidersConfig::default();
    providers.openai_codex.model = Some("gpt-5.5-codex-preview".to_string());
    let pinned = Config {
        provider: Some("openai-codex".to_string()),
        default_text_model: Some(DEFAULT_TEXT_MODEL.to_string()),
        providers: Some(providers),
        ..Default::default()
    };
    assert_eq!(pinned.default_model(), "gpt-5.5-codex-preview");
}

#[test]
fn direct_provider_ignores_foreign_deepseek_root_default_model() {
    let _lock = lock_test_env();

    let config = Config {
        provider: Some("zai".to_string()),
        default_text_model: Some(DEFAULT_TEXT_MODEL.to_string()),
        ..Default::default()
    };

    assert_eq!(config.api_provider(), ApiProvider::Zai);
    assert_eq!(config.default_model(), DEFAULT_ZAI_MODEL);
}

#[test]
fn insecure_skip_scenario() {
    // Scenario consolidation of: insecure_skip_tls_verify_is_scoped_to_active_provider, insecure_skip_tls_verify_reads_active_provider_table
    // from insecure_skip_tls_verify_is_scoped_to_active_provider
    {
        let mut providers = ProvidersConfig::default();
        providers.deepseek.insecure_skip_tls_verify = Some(true);
        providers.openai.insecure_skip_tls_verify = Some(false);
        let config = Config {
            provider: Some("openai".to_string()),
            providers: Some(providers),
            ..Default::default()
        };

        assert_eq!(config.api_provider(), ApiProvider::Openai);
        assert!(!config.insecure_skip_tls_verify());
    }
    // from insecure_skip_tls_verify_reads_active_provider_table
    {
        let mut providers = ProvidersConfig::default();
        providers.openai.insecure_skip_tls_verify = Some(true);
        let config = Config {
            provider: Some("openai".to_string()),
            providers: Some(providers),
            ..Default::default()
        };

        assert!(config.insecure_skip_tls_verify());
    }
}

#[test]
fn xiaomi_mimo_scenario() -> Result<()> {
    // Scenario consolidation of: xiaomi_mimo_provider_uses_documented_defaults, xiaomi_mimo_provider_honours_root_default_model_and_base_url, xiaomi_mimo_provider_drops_stale_deepseek_root_default_model, xiaomi_mimo_token_plan_mode_accepts_region_aliases, xiaomi_mimo_unknown_mode_stays_on_token_plan_endpoint
    // from xiaomi_mimo_provider_uses_documented_defaults
    {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-xiaomi-mimo-defaults-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        let config = Config {
            provider: Some("xiaomi-mimo".to_string()),
            ..Default::default()
        };

        config.validate()?;
        assert_eq!(config.api_provider(), ApiProvider::XiaomiMimo);
        assert_eq!(config.default_model(), DEFAULT_XIAOMI_MIMO_MODEL);
        assert_eq!(config.deepseek_base_url(), DEFAULT_XIAOMI_MIMO_BASE_URL);
    }
    // from xiaomi_mimo_provider_honours_root_default_model_and_base_url
    {
        let config = Config {
            provider: Some("xiaomi-mimo".to_string()),
            base_url: Some("https://token-plan-cn.xiaomimimo.com/v1".to_string()),
            default_text_model: Some("mimo-v2.5".to_string()),
            ..Default::default()
        };

        config.validate()?;
        assert_eq!(config.api_provider(), ApiProvider::XiaomiMimo);
        assert_eq!(config.default_model(), "mimo-v2.5");
        assert_eq!(
            config.deepseek_base_url(),
            "https://token-plan-cn.xiaomimimo.com/v1"
        );
    }
    // from xiaomi_mimo_provider_drops_stale_deepseek_root_default_model
    {
        // A leftover DeepSeek id after a provider switch must not be forwarded to
        // Xiaomi. Fall back to the MiMo seed default instead of substituting a
        // different *configured* model.
        let config = Config {
            provider: Some("xiaomi-mimo".to_string()),
            default_text_model: Some(DEFAULT_OPENROUTER_MODEL.to_string()),
            ..Default::default()
        };

        config.validate()?;
        assert_eq!(config.api_provider(), ApiProvider::XiaomiMimo);
        assert_eq!(config.default_model(), DEFAULT_XIAOMI_MIMO_MODEL);
    }
    // from xiaomi_mimo_token_plan_mode_accepts_region_aliases
    {
        let config: Config = toml::from_str(
            r#"
    provider = "mimo"

    [providers.mimo]
    mode = "token-plan-ams"
    "#,
        )?;

        config.validate()?;
        assert_eq!(config.api_provider(), ApiProvider::XiaomiMimo);
        assert_eq!(
            config.deepseek_base_url(),
            XIAOMI_MIMO_TOKEN_PLAN_AMS_BASE_URL
        );
    }
    // from xiaomi_mimo_unknown_mode_stays_on_token_plan_endpoint
    {
        let config: Config = toml::from_str(
            r#"
    provider = "mimo"

    [providers.mimo]
    mode = "token-plan-usa"
    "#,
        )?;

        config.validate()?;
        assert_eq!(config.api_provider(), ApiProvider::XiaomiMimo);
        assert_eq!(config.deepseek_base_url(), DEFAULT_XIAOMI_MIMO_BASE_URL);
    }
    Ok(())
}

#[test]
fn openai_codex_provider_ignores_legacy_root_base_url() -> Result<()> {
    let config = Config {
        provider: Some("openai-codex".to_string()),
        // `base_url` is the legacy DeepSeek setting in a normal multi-provider
        // config. Switching to Codex must not inherit it and make the official
        // CLI OAuth login ineligible.
        base_url: Some("https://api.deepseek.com".to_string()),
        default_text_model: Some("gpt-5.5".to_string()),
        ..Default::default()
    };

    config.validate()?;
    assert_eq!(config.api_provider(), ApiProvider::OpenaiCodex);
    assert_eq!(config.default_model(), "gpt-5.5");
    assert_eq!(config.deepseek_base_url(), DEFAULT_OPENAI_CODEX_BASE_URL);
    assert!(!config.provider_uses_custom_endpoint(ApiProvider::OpenaiCodex));
    Ok(())
}

#[test]
fn xiaomi_provider_alias_table_maps_to_mimo_config() -> Result<()> {
    let config: Config = toml::from_str(
        r#"
provider = "xiaomi-mimo"
default_text_model = "deepseek/deepseek-v4-pro"

[providers.xiaomi]
api_key = "mimo-table-key"
base_url = "https://token-plan-sgp.xiaomimimo.com/v1"
model = "mimo-v2.5-pro"
"#,
    )?;

    config.validate()?;
    assert_eq!(config.api_provider(), ApiProvider::XiaomiMimo);
    assert_eq!(config.deepseek_api_key()?, "mimo-table-key");
    assert_eq!(
        config.deepseek_base_url(),
        "https://token-plan-sgp.xiaomimimo.com/v1"
    );
    assert_eq!(config.default_model(), DEFAULT_XIAOMI_MIMO_MODEL);
    Ok(())
}

#[test]
fn xiaomi_token_plan_key_rewrites_saved_pay_as_you_go_base_url() -> Result<()> {
    let config: Config = toml::from_str(
        r#"
provider = "xiaomi-mimo"

[providers.xiaomi_mimo]
api_key = "tp-test-token-plan-key"
base_url = "https://api.xiaomimimo.com/v1"
model = "mimo-v2.5-pro"
"#,
    )?;

    config.validate()?;
    assert_eq!(config.api_provider(), ApiProvider::XiaomiMimo);
    assert_eq!(config.deepseek_base_url(), DEFAULT_XIAOMI_MIMO_BASE_URL);
    assert_eq!(config.default_model(), DEFAULT_XIAOMI_MIMO_MODEL);
    Ok(())
}

#[test]
fn xiaomi_mimo_custom_env_url_does_not_inherit_ambient_key() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-xiaomi-mimo-env-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "mimo");
        env::set_var("MIMO_API_KEY", "mimo-env-key");
        env::set_var("MIMO_BASE_URL", "https://mimo-gateway.example/v1");
        env::set_var("MIMO_MODEL", "mimo-v2.5");
    }

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::XiaomiMimo);
    let error = config
        .deepseek_api_key()
        .expect_err("ambient key must not follow a custom endpoint");
    assert!(error.to_string().contains("must be bound explicitly"));
    assert!(!has_api_key(&config));
    assert_eq!(
        config.deepseek_base_url(),
        "https://mimo-gateway.example/v1"
    );
    assert_eq!(config.default_model(), "mimo-v2.5");
    Ok(())
}

#[test]
fn xiaomi_mimo_env_token_plan_mode_uses_token_plan_key_and_endpoint() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-xiaomi-mimo-token-plan-env-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "xiaomi-mimo");
        env::set_var("XIAOMI_MIMO_MODE", "token-plan-cn");
        env::set_var("XIAOMI_MIMO_TOKEN_PLAN_API_KEY", "tp-env-key");
        env::set_var("XIAOMI_MIMO_API_KEY", "sk-env-key");
        env::set_var("XIAOMI_MIMO_MODEL", "voiceclone");
    }

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::XiaomiMimo);
    assert_eq!(config.deepseek_api_key()?, "tp-env-key");
    assert_eq!(
        config.deepseek_base_url(),
        XIAOMI_MIMO_TOKEN_PLAN_CN_BASE_URL
    );
    assert_eq!(config.default_model(), "voiceclone");
    Ok(())
}

#[test]
fn xiaomi_mimo_env_pay_as_you_go_mode_prefers_standard_key() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-xiaomi-mimo-payg-env-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "xiaomi-mimo");
        env::set_var("XIAOMI_MIMO_MODE", "pay-as-you-go");
        env::set_var("XIAOMI_MIMO_TOKEN_PLAN_API_KEY", "tp-env-key");
        env::set_var("XIAOMI_MIMO_API_KEY", "sk-env-key");
    }

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::XiaomiMimo);
    assert_eq!(config.deepseek_api_key()?, "sk-env-key");
    assert_eq!(
        config.deepseek_base_url(),
        XIAOMI_MIMO_PAY_AS_YOU_GO_BASE_URL
    );
    Ok(())
}

#[test]
fn atlascloud_provider_uses_documented_defaults() -> Result<()> {
    let config = Config {
        provider: Some("atlascloud".to_string()),
        ..Default::default()
    };

    config.validate()?;
    assert_eq!(config.api_provider(), ApiProvider::Atlascloud);
    assert_eq!(config.default_model(), DEFAULT_ATLASCLOUD_MODEL);
    assert_eq!(config.deepseek_base_url(), DEFAULT_ATLASCLOUD_BASE_URL);
    Ok(())
}

#[test]
fn atlascloud_env_overrides_provider_base_url_and_model() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-atlascloud-env-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "atlascloud");
        env::set_var("ATLASCLOUD_API_KEY", "atlascloud-env-key");
        env::set_var("ATLASCLOUD_BASE_URL", "https://api.atlascloud.ai/v1");
        env::set_var("ATLASCLOUD_MODEL", "deepseek-ai/deepseek-v4-flash");
    }

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Atlascloud);
    assert_eq!(config.deepseek_api_key()?, "atlascloud-env-key");
    assert_eq!(config.deepseek_base_url(), "https://api.atlascloud.ai/v1");
    assert_eq!(config.default_model(), "deepseek-ai/deepseek-v4-flash");
    Ok(())
}

#[test]
fn wanjie_ark_provider_uses_documented_defaults() -> Result<()> {
    let config = Config {
        provider: Some("wanjie-ark".to_string()),
        ..Default::default()
    };

    config.validate()?;
    assert_eq!(config.api_provider(), ApiProvider::WanjieArk);
    assert_eq!(config.default_model(), DEFAULT_WANJIE_ARK_MODEL);
    assert_eq!(config.deepseek_base_url(), DEFAULT_WANJIE_ARK_BASE_URL);
    Ok(())
}

#[test]
fn wanjie_ark_custom_env_url_does_not_inherit_ambient_key() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-wanjie-env-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "ark-wanjie");
        env::set_var("WANJIE_ARK_API_KEY", "wanjie-env-key");
        env::set_var("WANJIE_ARK_BASE_URL", "https://wanjie.example/api/v1");
        env::set_var("WANJIE_ARK_MODEL", "wanjie-model-id");
    }

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::WanjieArk);
    let error = config
        .deepseek_api_key()
        .expect_err("ambient key must not follow a custom endpoint");
    assert!(error.to_string().contains("must be bound explicitly"));
    assert!(!has_api_key(&config));
    assert_eq!(config.deepseek_base_url(), "https://wanjie.example/api/v1");
    assert_eq!(config.default_model(), "wanjie-model-id");
    Ok(())
}

#[test]
fn wanjie_ark_provider_accepts_custom_model_and_table_key() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-wanjie-table-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"provider = "wanjie-ark"

[providers.wanjie_ark]
api_key = "wanjie-table-key"
base_url = "https://maas-openapi.wanjiedata.com/api/v1"
model = "account-model-id"
"#,
    )?;

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::WanjieArk);
    assert_eq!(config.deepseek_api_key()?, "wanjie-table-key");
    assert_eq!(
        config.deepseek_base_url(),
        "https://maas-openapi.wanjiedata.com/api/v1"
    );
    assert_eq!(config.default_model(), "account-model-id");
    Ok(())
}

#[test]
fn openai_provider_accepts_custom_model_and_base_url() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-openai-table-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"provider = "openai"

[providers.openai]
api_key = "openai-table-key"
base_url = "https://openai-compatible.example/api/coding/paas/v4"
model = "glm-5"
"#,
    )?;

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Openai);
    assert_eq!(config.deepseek_api_key()?, "openai-table-key");
    assert_eq!(
        config.deepseek_base_url(),
        "https://openai-compatible.example/api/coding/paas/v4"
    );
    assert_eq!(config.default_model(), "glm-5");
    Ok(())
}

#[test]
fn openai_provider_accepts_dashscope_bailian_fixture() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-dashscope-openai-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"provider = "openai"

[providers.openai]
api_key = "dashscope-table-key"
base_url = "https://dashscope-intl.aliyuncs.com/compatible-mode/v1"
model = "qwen-plus"
"#,
    )?;

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Openai);
    assert_eq!(config.deepseek_api_key()?, "dashscope-table-key");
    assert_eq!(
        config.deepseek_base_url(),
        "https://dashscope-intl.aliyuncs.com/compatible-mode/v1"
    );
    assert_eq!(config.default_model(), "qwen-plus");
    Ok(())
}

#[test]
fn qianfan_provider_accepts_custom_model_and_base_url() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-qianfan-provider-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"provider = "qianfan"

[providers.qianfan]
api_key = "qianfan-table-key"
base_url = "https://qianfan.baidubce.com/v2"
model = "custom-qianfan-service-id"
"#,
    )?;

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Qianfan);
    assert_eq!(config.deepseek_api_key()?, "qianfan-table-key");
    assert_eq!(
        config.deepseek_base_url(),
        "https://qianfan.baidubce.com/v2"
    );
    assert_eq!(config.default_model(), "custom-qianfan-service-id");
    Ok(())
}

#[test]
fn provider_config_loads_reasoning_stream_style() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-reasoning-style-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"provider = "openai"

[providers.openai]
api_key = "openai-table-key"
base_url = "https://openai-compatible.example/v1"
model = "custom-reasoner"
reasoning_stream_style = "inline_tags"
"#,
    )?;

    let config = Config::load(None, None)?;
    let openai = config
        .provider_config_for(ApiProvider::Openai)
        .expect("openai provider config");
    assert_eq!(
        openai.reasoning_stream_style.as_deref(),
        Some("inline_tags")
    );
    Ok(())
}

// Regression for issue #1714: `codewhale --provider openai --model
// MiniMax-M2.7` forwards the choice via DEEPSEEK_MODEL (never
// OPENAI_MODEL) and uses the DEFAULT base_url. The explicit custom model
// must pass through verbatim instead of silently becoming a
// DeepSeek/provider default.
#[test]
fn deepseek_model_env_passes_custom_model_through_for_non_deepseek_providers() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-1714-passthrough-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;

    // (a) provider=openai + model="MiniMax-M2.7" via env, NO OPENAI_MODEL,
    // DEFAULT base_url.
    {
        let _guard = EnvGuard::new(&temp_root);
        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "openai");
            env::set_var("OPENAI_API_KEY", "openai-env-key");
            env::set_var("DEEPSEEK_MODEL", "MiniMax-M2.7");
        }

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Openai);
        assert_eq!(config.deepseek_base_url(), DEFAULT_OPENAI_BASE_URL);
        assert_eq!(config.default_model(), "MiniMax-M2.7");
    }

    // (b) a non-passthrough provider (novita) with an unknown custom model
    // and the DEFAULT base_url must also be preserved verbatim — never
    // rewritten to DEFAULT_NOVITA_MODEL.
    {
        let _guard = EnvGuard::new(&temp_root);
        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var("DEEPSEEK_PROVIDER", "novita");
            env::set_var("NOVITA_API_KEY", "novita-env-key");
            env::set_var("DEEPSEEK_MODEL", "MiniMax-M2.7");
        }

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Novita);
        assert_eq!(config.deepseek_base_url(), DEFAULT_NOVITA_BASE_URL);
        assert_ne!(config.default_model(), DEFAULT_NOVITA_MODEL);
        assert_eq!(config.default_model(), "MiniMax-M2.7");
    }

    Ok(())
}

#[test]
fn openai_custom_env_url_does_not_inherit_ambient_key() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-openai-env-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "openai");
        env::set_var("OPENAI_API_KEY", "openai-env-key");
        env::set_var("OPENAI_BASE_URL", "https://openai-compatible.example/v4");
        env::set_var("OPENAI_MODEL", "glm-5");
    }

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Openai);
    let error = config
        .deepseek_api_key()
        .expect_err("ambient key must not follow a custom endpoint");
    assert!(error.to_string().contains("must be bound explicitly"));
    assert!(!has_api_key(&config));
    assert_eq!(
        config.deepseek_base_url(),
        "https://openai-compatible.example/v4"
    );
    assert_eq!(config.default_model(), "glm-5");
    Ok(())
}

#[test]
fn openai_facade_custom_url_does_not_inherit_ambient_key() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-openai-forwarded-base-url-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "openai");
        env::set_var("OPENAI_API_KEY", "forwarded-openai-key");
        env::set_var("DEEPSEEK_BASE_URL", "https://forwarded-openai.example/v4");
        env::set_var("DEEPSEEK_MODEL", "glm-5");
    }

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Openai);
    let error = config
        .deepseek_api_key()
        .expect_err("ambient key must not follow a custom endpoint");
    assert!(error.to_string().contains("must be bound explicitly"));
    assert!(!has_api_key(&config));
    assert_eq!(
        config.deepseek_base_url(),
        "https://forwarded-openai.example/v4"
    );
    assert_eq!(config.default_model(), "glm-5");
    Ok(())
}

#[test]
fn openrouter_provider_uses_canonical_defaults() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-or-defaults-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config = Config {
        provider: Some("openrouter".to_string()),
        ..Default::default()
    };
    config.validate()?;
    assert_eq!(config.api_provider(), ApiProvider::Openrouter);
    assert_eq!(config.default_model(), DEFAULT_OPENROUTER_MODEL);
    assert_eq!(config.deepseek_base_url(), DEFAULT_OPENROUTER_BASE_URL);
    Ok(())
}

#[test]
fn novita_provider_uses_canonical_defaults() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-novita-defaults-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config = Config {
        provider: Some("novita".to_string()),
        ..Default::default()
    };
    config.validate()?;
    assert_eq!(config.api_provider(), ApiProvider::Novita);
    assert_eq!(config.default_model(), DEFAULT_NOVITA_MODEL);
    assert_eq!(config.deepseek_base_url(), DEFAULT_NOVITA_BASE_URL);
    Ok(())
}

#[test]
fn fireworks_provider_uses_canonical_defaults() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-fireworks-defaults-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config = Config {
        provider: Some("fireworks".to_string()),
        ..Default::default()
    };
    config.validate()?;
    assert_eq!(config.api_provider(), ApiProvider::Fireworks);
    assert_eq!(config.default_model(), DEFAULT_FIREWORKS_MODEL);
    assert_eq!(config.deepseek_base_url(), DEFAULT_FIREWORKS_BASE_URL);
    Ok(())
}

#[test]
fn fireworks_flash_alias_is_not_mapped_to_undocumented_model() -> Result<()> {
    let config = Config {
        provider: Some("fireworks".to_string()),
        default_text_model: Some("deepseek-v4-flash".to_string()),
        ..Default::default()
    };

    config.validate()?;
    assert_eq!(config.api_provider(), ApiProvider::Fireworks);
    assert_eq!(config.default_model(), "deepseek-v4-flash");
    Ok(())
}

#[test]
fn volcengine_provider_requires_api_key() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-volcengine-auth-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config = Config {
        provider: Some("volcengine".to_string()),
        ..Default::default()
    };

    config.validate()?;
    let err = config.deepseek_api_key().expect_err("missing key");
    assert!(err.to_string().contains("Volcengine Ark API key not found"));
    Ok(())
}

#[test]
fn volcengine_custom_env_url_does_not_inherit_ambient_key() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-volcengine-env-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "volcengine");
        env::set_var("ARK_API_KEY", "volc-env-key");
        env::set_var("VOLCENGINE_ARK_BASE_URL", "https://volc.example/v1");
        env::set_var("VOLCENGINE_ARK_MODEL", "DeepSeek-V4-Flash");
    }

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Volcengine);
    let error = config
        .deepseek_api_key()
        .expect_err("ambient key must not follow a custom endpoint");
    assert!(error.to_string().contains("must be bound explicitly"));
    assert!(!has_api_key(&config));
    assert_eq!(config.deepseek_base_url(), "https://volc.example/v1");
    assert_eq!(config.default_model(), "DeepSeek-V4-Flash");
    Ok(())
}

#[test]
fn siliconflow_provider_uses_canonical_defaults() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-siliconflow-defaults-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config = Config {
        provider: Some("siliconflow".to_string()),
        ..Default::default()
    };
    config.validate()?;
    assert_eq!(config.api_provider(), ApiProvider::Siliconflow);
    assert_eq!(config.default_model(), DEFAULT_SILICONFLOW_MODEL);
    assert_eq!(config.deepseek_base_url(), DEFAULT_SILICONFLOW_BASE_URL);
    assert_eq!(
        model_completion_names_for_provider(ApiProvider::Siliconflow),
        vec![DEFAULT_SILICONFLOW_MODEL, DEFAULT_SILICONFLOW_FLASH_MODEL]
    );
    Ok(())
}

#[test]
fn sglang_provider_works_without_api_key() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-sglang-defaults-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config = Config {
        provider: Some("sglang".to_string()),
        ..Default::default()
    };
    config.validate()?;
    assert_eq!(config.api_provider(), ApiProvider::Sglang);
    assert_eq!(config.default_model(), DEFAULT_SGLANG_MODEL);
    assert_eq!(config.deepseek_base_url(), DEFAULT_SGLANG_BASE_URL);
    assert_eq!(config.deepseek_api_key()?, "");
    assert!(has_api_key_for(&config, ApiProvider::Sglang));
    Ok(())
}

#[test]
fn ollama_provider_uses_local_defaults_without_api_key() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-ollama-defaults-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config = Config {
        provider: Some("ollama".to_string()),
        ..Default::default()
    };
    config.validate()?;
    assert_eq!(config.api_provider(), ApiProvider::Ollama);
    assert_eq!(config.default_model(), DEFAULT_OLLAMA_MODEL);
    assert_eq!(config.deepseek_base_url(), DEFAULT_OLLAMA_BASE_URL);
    assert_eq!(config.deepseek_api_key()?, "");
    assert!(has_api_key_for(&config, ApiProvider::Ollama));
    Ok(())
}

#[test]
fn ollama_cloud_resolves_env_key_and_is_not_keyless() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-ollama-cloud-env-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);
    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe { env::set_var("OLLAMA_API_KEY", "ollama-cloud-env-key") };

    let config = Config {
        provider: Some("ollama".to_string()),
        providers: Some(ProvidersConfig {
            ollama: ProviderConfig {
                base_url: Some("https://ollama.com/v1/".to_string()),
                ..ProviderConfig::default()
            },
            ..ProvidersConfig::default()
        }),
        ..Config::default()
    };

    assert_eq!(config.api_provider(), ApiProvider::OllamaCloud);
    assert!(!provider_route_is_keyless_self_hosted(
        ApiProvider::OllamaCloud,
        &config.deepseek_base_url()
    ));
    assert_eq!(config.deepseek_api_key()?, "ollama-cloud-env-key");
    assert!(has_api_key_for(&config, ApiProvider::OllamaCloud));
    Ok(())
}

#[test]
fn ollama_cloud_resolves_saved_provider_key() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let codewhale_home = temp_root.path().join("isolated-codewhale");
    fs::create_dir_all(&codewhale_home)?;
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");

    codewhale_secrets::Secrets::auto_detect().set("ollama", "ollama-cloud-saved-key")?;
    let config = Config {
        provider: Some("ollama".to_string()),
        providers: Some(ProvidersConfig {
            ollama: ProviderConfig {
                base_url: Some(codewhale_config::provider::OLLAMA_CLOUD_BASE_URL.to_string()),
                ..ProviderConfig::default()
            },
            ..ProvidersConfig::default()
        }),
        ..Config::default()
    };

    assert_eq!(config.api_provider(), ApiProvider::OllamaCloud);
    assert_eq!(config.deepseek_api_key()?, "ollama-cloud-saved-key");
    assert!(has_api_key_for(&config, ApiProvider::OllamaCloud));
    Ok(())
}

#[test]
fn explicit_ollama_cloud_uses_new_secret_slot_without_local_fallback() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let codewhale_home = temp_root.path().join("isolated-codewhale");
    fs::create_dir_all(&codewhale_home)?;
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");

    let secrets = codewhale_secrets::Secrets::auto_detect();
    secrets.set("ollama", "must-not-be-consumed")?;
    let config = Config {
        provider: Some("ollama-cloud".to_string()),
        providers: Some(ProvidersConfig {
            ollama: ProviderConfig {
                base_url: Some(codewhale_config::provider::OLLAMA_CLOUD_BASE_URL.to_string()),
                ..ProviderConfig::default()
            },
            ..ProvidersConfig::default()
        }),
        ..Config::default()
    };

    assert_eq!(config.api_provider(), ApiProvider::OllamaCloud);
    let identity = config
        .resolve_provider_identity("ollama-cloud")
        .expect("explicit Cloud identity");
    assert!(!identity.migrated_legacy_ollama_cloud_route);
    assert!(!has_api_key_for(&config, ApiProvider::OllamaCloud));
    assert!(config.deepseek_api_key().is_err());

    secrets.set("ollama-cloud", "cloud-slot-key")?;
    assert!(has_api_key_for(&config, ApiProvider::OllamaCloud));
    assert_eq!(config.deepseek_api_key()?, "cloud-slot-key");
    Ok(())
}

#[test]
fn ollama_cloud_env_precedence_is_cloud_name_then_official_name() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    // Safety: test-only environment mutation guarded by a global mutex and
    // restored by EnvGuard.
    unsafe {
        env::set_var("OLLAMA_CLOUD_API_KEY", "cloud-specific-key");
        env::set_var("OLLAMA_API_KEY", "official-fallback-key");
    }
    let config = Config {
        provider: Some("ollama-cloud".to_string()),
        ..Config::default()
    };

    assert_eq!(config.deepseek_api_key()?, "cloud-specific-key");
    // Safety: same serialized test and EnvGuard restore the prior value.
    unsafe { env::remove_var("OLLAMA_CLOUD_API_KEY") };
    assert_eq!(config.deepseek_api_key()?, "official-fallback-key");
    Ok(())
}

#[test]
fn migrated_ollama_cloud_scope_preserves_legacy_table_and_slot_read_only() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let codewhale_home = temp_root.path().join("isolated-codewhale");
    fs::create_dir_all(&codewhale_home)?;
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
    codewhale_secrets::Secrets::auto_detect().set("ollama", "legacy-cloud-key")?;

    let config = Config {
        provider: Some("deepseek".to_string()),
        providers: Some(ProvidersConfig {
            ollama: ProviderConfig {
                base_url: Some(codewhale_config::provider::OLLAMA_CLOUD_BASE_URL.to_string()),
                model: Some("legacy-cloud-model".to_string()),
                ..ProviderConfig::default()
            },
            ..ProvidersConfig::default()
        }),
        ..Config::default()
    };
    let identity = config
        .resolve_provider_identity("ollama")
        .expect("legacy identity migrates");
    assert_eq!(identity.provider, ApiProvider::OllamaCloud);
    assert_eq!(identity.key, "ollama-cloud");
    assert!(identity.migrated_legacy_ollama_cloud_route);

    let mut scoped = config.clone();
    scoped.scope_to_provider_identity(&identity);
    assert_eq!(scoped.api_provider(), ApiProvider::OllamaCloud);
    assert_eq!(
        scoped.deepseek_base_url(),
        codewhale_config::provider::OLLAMA_CLOUD_BASE_URL
    );
    assert_eq!(scoped.default_model(), "legacy-cloud-model");
    assert_eq!(scoped.deepseek_api_key()?, "legacy-cloud-key");
    assert!(
        scoped
            .providers
            .as_ref()
            .expect("providers")
            .ollama_cloud
            .api_key
            .is_none(),
        "migration must not copy secret material into the new config table"
    );
    assert_eq!(
        codewhale_secrets::Secrets::auto_detect().get("ollama-cloud")?,
        None,
        "migration must not copy the legacy secret into the new slot"
    );
    Ok(())
}

#[test]
fn ollama_cloud_without_key_fails_with_cloud_guidance() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-ollama-cloud-missing-key-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config = Config {
        provider: Some("ollama".to_string()),
        providers: Some(ProvidersConfig {
            ollama: ProviderConfig {
                base_url: Some("https://ollama.com/v1".to_string()),
                ..ProviderConfig::default()
            },
            ..ProvidersConfig::default()
        }),
        ..Config::default()
    };

    assert_eq!(config.api_provider(), ApiProvider::OllamaCloud);
    assert!(!has_api_key_for(&config, ApiProvider::OllamaCloud));
    let error = config
        .deepseek_api_key()
        .expect_err("Ollama Cloud must require an API key");
    let message = error.to_string();
    assert!(message.contains("Ollama Cloud API key not found"));
    assert!(message.contains("https://ollama.com/settings/keys"));
    assert!(message.contains("OLLAMA_CLOUD_API_KEY / OLLAMA_API_KEY"));
    Ok(())
}

#[test]
fn ollama_custom_remote_does_not_inherit_cloud_env_key() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-ollama-custom-remote-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe { env::set_var("OLLAMA_API_KEY", "must-not-cross-routes") };
    codewhale_secrets::Secrets::auto_detect().set("ollama", "must-not-cross-routes-either")?;

    let config = Config {
        provider: Some("ollama".to_string()),
        providers: Some(ProvidersConfig {
            ollama: ProviderConfig {
                base_url: Some("https://ollama-gateway.example/v1".to_string()),
                ..ProviderConfig::default()
            },
            ..ProvidersConfig::default()
        }),
        ..Config::default()
    };

    assert!(config.provider_uses_custom_endpoint(ApiProvider::Ollama));
    assert!(!has_api_key_for(&config, ApiProvider::Ollama));
    let error = config
        .deepseek_api_key()
        .expect_err("custom remote must bind its credential explicitly");
    assert!(
        error
            .to_string()
            .contains("Custom endpoint credentials for ollama must be bound explicitly")
    );
    Ok(())
}

#[test]
fn ollama_model_is_passed_through_verbatim() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-ollama-model-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"provider = "ollama"

[providers.ollama]
base_url = "http://127.0.0.1:11434/v1"
model = "qwen2.5-coder:7b"
"#,
    )?;

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Ollama);
    assert_eq!(config.default_model(), "qwen2.5-coder:7b");
    assert_eq!(config.deepseek_base_url(), "http://127.0.0.1:11434/v1");
    Ok(())
}

#[test]
fn deepseek_base_url_env_scopes_to_self_hosted_providers() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-self-hosted-base-url-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "ollama");
        env::set_var("DEEPSEEK_BASE_URL", "http://ollama.remote:11434/v1");
    }
    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Ollama);
    assert_eq!(config.deepseek_base_url(), "http://ollama.remote:11434/v1");

    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "vllm");
        env::set_var("DEEPSEEK_BASE_URL", "http://vllm.remote:8000/v1");
    }
    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Vllm);
    assert_eq!(config.deepseek_base_url(), "http://vllm.remote:8000/v1");
    Ok(())
}

#[test]
fn vllm_env_resolves_reported_lan_http_endpoint_and_model() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-vllm-lan-http-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "vllm");
        env::set_var("VLLM_BASE_URL", "http://192.168.0.110:8000/v1");
        env::set_var("DEEPSEEK_MODEL", "deepseek-v4-flash");
    }

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Vllm);
    assert_eq!(config.deepseek_base_url(), "http://192.168.0.110:8000/v1");
    assert_eq!(config.default_model(), "deepseek-v4-flash");
    Ok(())
}

#[test]
fn ollama_env_overrides_base_url_and_model() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-ollama-env-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "ollama-local");
        env::set_var("OLLAMA_BASE_URL", "http://ollama.example/v1");
        env::set_var("OLLAMA_MODEL", "deepseek-coder-v2:16b");
    }

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Ollama);
    assert_eq!(config.deepseek_base_url(), "http://ollama.example/v1");
    assert_eq!(config.default_model(), "deepseek-coder-v2:16b");
    Ok(())
}

#[test]
fn openrouter_env_api_key_resolves_via_deepseek_api_key() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-or-env-key-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "openrouter");
        env::set_var("OPENROUTER_API_KEY", "or-env-key");
        env::set_var("OPENROUTER_MODEL", "deepseek-v4-flash");
    }

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Openrouter);
    assert_eq!(config.deepseek_api_key()?, "or-env-key");
    assert_eq!(config.default_model(), DEFAULT_OPENROUTER_FLASH_MODEL);
    Ok(())
}

#[test]
fn novita_env_api_key_resolves_via_deepseek_api_key() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-novita-env-key-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "novita");
        env::set_var("NOVITA_API_KEY", "novita-env-key");
        env::set_var("NOVITA_MODEL", "deepseek-v4-flash");
    }

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Novita);
    assert_eq!(config.deepseek_api_key()?, "novita-env-key");
    assert_eq!(config.default_model(), DEFAULT_NOVITA_FLASH_MODEL);
    Ok(())
}

#[test]
fn fireworks_env_overrides_key_and_model() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-fireworks-env-key-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "fireworks");
        env::set_var("FIREWORKS_API_KEY", "fw-env-key");
        env::set_var(
            "FIREWORKS_MODEL",
            "accounts/fireworks/models/account-specific-model",
        );
    }

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Fireworks);
    assert_eq!(config.deepseek_api_key()?, "fw-env-key");
    assert_eq!(
        config.default_model(),
        "accounts/fireworks/models/account-specific-model"
    );
    Ok(())
}

#[test]
fn siliconflow_custom_env_url_does_not_inherit_ambient_key() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-siliconflow-env-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe {
        env::set_var("CODEWHALE_PROVIDER", "siliconflow");
        env::set_var("SILICONFLOW_API_KEY", "sf-env-key");
        env::set_var("SILICONFLOW_BASE_URL", "https://sf-mirror.example/v1");
        env::set_var("SILICONFLOW_MODEL", "deepseek-v4-flash");
    }

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Siliconflow);
    let error = config
        .deepseek_api_key()
        .expect_err("ambient key must not follow a custom endpoint");
    assert!(error.to_string().contains("must be bound explicitly"));
    assert!(!has_api_key(&config));
    assert_eq!(config.deepseek_base_url(), "https://sf-mirror.example/v1");
    assert_eq!(config.default_model(), "deepseek-v4-flash");
    Ok(())
}

#[test]
fn arcee_provider_uses_direct_defaults() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-arcee-defaults-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    unsafe {
        env::set_var("CODEWHALE_PROVIDER", "arcee");
        env::set_var("ARCEE_API_KEY", "arcee-env-key");
    }

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Arcee);
    assert_eq!(config.deepseek_api_key()?, "arcee-env-key");
    assert_eq!(config.deepseek_base_url(), DEFAULT_ARCEE_BASE_URL);
    assert_eq!(config.default_model(), DEFAULT_ARCEE_MODEL);
    Ok(())
}

#[test]
fn arcee_custom_env_url_does_not_inherit_ambient_key() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-arcee-env-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    unsafe {
        env::set_var("CODEWHALE_PROVIDER", "arcee");
        env::set_var("ARCEE_API_KEY", "arcee-env-key");
        env::set_var("ARCEE_BASE_URL", "https://arcee-mirror.example/api/v1");
        env::set_var("ARCEE_MODEL", "arcee-trinity-large-preview");
    }

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Arcee);
    let error = config
        .deepseek_api_key()
        .expect_err("ambient key must not follow a custom endpoint");
    assert!(error.to_string().contains("must be bound explicitly"));
    assert!(!has_api_key(&config));
    assert_eq!(
        config.deepseek_base_url(),
        "https://arcee-mirror.example/api/v1"
    );
    assert_eq!(config.default_model(), "arcee-trinity-large-preview");
    Ok(())
}

#[test]
fn arcee_provider_table_configures_direct_route() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-arcee-table-test-{}-{}",
        std::process::id(),
        nanos
    ));
    let config_dir = temp_root.join(".deepseek");
    fs::create_dir_all(&config_dir)?;
    let _guard = EnvGuard::new(&temp_root);
    fs::write(
        config_dir.join("config.toml"),
        r#"
provider = "arcee"

[providers.arcee]
api_key = "arcee-file-key"
base_url = "https://api.arcee.ai/api/v1"
model = "arcee-trinity-large-preview"
"#,
    )?;

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Arcee);
    assert_eq!(config.deepseek_api_key()?, "arcee-file-key");
    assert_eq!(config.deepseek_base_url(), DEFAULT_ARCEE_BASE_URL);
    assert_eq!(config.default_model(), ARCEE_TRINITY_LARGE_PREVIEW_MODEL);
    Ok(())
}

#[test]
fn siliconflow_cn_base_url_env_normalizes_model_aliases() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-siliconflow-cn-env-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe {
        env::set_var("CODEWHALE_PROVIDER", "siliconflow-CN");
        env::set_var("SILICONFLOW_API_KEY", "sf-env-key");
        env::set_var("SILICONFLOW_BASE_URL", "https://api.siliconflow.cn/v1");
        env::set_var("SILICONFLOW_MODEL", "deepseek-reasoner");
    }

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::SiliconflowCn);
    assert_eq!(config.deepseek_api_key()?, "sf-env-key");
    assert_eq!(config.deepseek_base_url(), "https://api.siliconflow.cn/v1");
    assert_eq!(config.default_model(), DEFAULT_SILICONFLOW_MODEL);
    Ok(())
}

#[test]
fn openrouter_base_url_env_overrides_default() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-or-base-url-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "openrouter");
        env::set_var("OPENROUTER_BASE_URL", "https://or-mirror.example/v1");
    }

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Openrouter);
    assert_eq!(config.deepseek_base_url(), "https://or-mirror.example/v1");
    Ok(())
}

#[test]
fn openrouter_reads_provider_table_from_config_file() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-or-table-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"provider = "openrouter"

[providers.openrouter]
api_key = "or-table-key"
base_url = "https://or-table.example/v1"
"#,
    )?;

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Openrouter);
    assert_eq!(config.deepseek_api_key()?, "or-table-key");
    assert_eq!(config.deepseek_base_url(), "https://or-table.example/v1");
    Ok(())
}

#[test]
fn siliconflow_reads_provider_table_from_config_file() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-siliconflow-table-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"provider = "siliconflow"

[providers.siliconflow]
api_key = "sf-table-key"
model = "deepseek-v4-flash"
"#,
    )?;

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Siliconflow);
    assert_eq!(config.deepseek_api_key()?, "sf-table-key");
    assert_eq!(config.deepseek_base_url(), DEFAULT_SILICONFLOW_BASE_URL);
    assert_eq!(config.default_model(), DEFAULT_SILICONFLOW_FLASH_MODEL);
    Ok(())
}

#[test]
fn siliconflow_cn_reads_hyphenated_provider_table_from_config_file() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-siliconflow-cn-table-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"provider = "siliconflow-CN"

[providers.siliconflow-CN]
api_key = "sf-cn-table-key"
base_url = "https://api.siliconflow.cn/v1"
model = "deepseek-reasoner"
"#,
    )?;

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::SiliconflowCn);
    assert_eq!(config.deepseek_api_key()?, "sf-cn-table-key");
    assert_eq!(config.deepseek_base_url(), DEFAULT_SILICONFLOW_CN_BASE_URL);
    assert_eq!(config.default_model(), DEFAULT_SILICONFLOW_MODEL);
    assert!(has_api_key_for(&config, ApiProvider::SiliconflowCn));
    Ok(())
}

#[test]
fn siliconflow_cn_preserves_reported_deepseek_prefixed_v4_route() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-siliconflow-cn-v4-report-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"provider = "siliconflow-CN"

[providers.siliconflow-CN]
api_key = "sf-cn-table-key"
base_url = "https://api.siliconflow.cn/v1"
model = "deepseek-ai/DeepSeek-V4-Pro"
"#,
    )?;

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::SiliconflowCn);
    assert_ne!(config.api_provider(), ApiProvider::Deepseek);
    assert_eq!(config.deepseek_api_key()?, "sf-cn-table-key");
    assert_eq!(config.deepseek_base_url(), DEFAULT_SILICONFLOW_CN_BASE_URL);
    assert_eq!(config.default_model(), DEFAULT_SILICONFLOW_MODEL);
    assert_eq!(
        wire_model_for_provider(config.api_provider(), &config.default_model()),
        DEFAULT_SILICONFLOW_MODEL
    );
    Ok(())
}

#[test]
fn siliconflow_cn_falls_back_to_shared_siliconflow_table_when_unset() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-siliconflow-cn-fallback-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"provider = "siliconflow-CN"

[providers.siliconflow]
api_key = "sf-shared-key"
base_url = "https://api.siliconflow.com/v1"
model = "deepseek-chat"

[providers.siliconflow_cn]
base_url = "https://api.siliconflow.cn/v1"
"#,
    )?;

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::SiliconflowCn);
    assert_eq!(config.deepseek_api_key()?, "sf-shared-key");
    assert_eq!(config.deepseek_base_url(), DEFAULT_SILICONFLOW_CN_BASE_URL);
    assert_eq!(config.default_model(), DEFAULT_SILICONFLOW_FLASH_MODEL);
    assert!(active_provider_has_config_api_key(&config));
    Ok(())
}

#[test]
fn siliconflow_cn_env_overrides_write_cn_table_only() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-siliconflow-cn-env-table-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"provider = "siliconflow-CN"

[providers.siliconflow]
api_key = "sf-shared-key"
base_url = "https://api.siliconflow.com/v1"
model = "deepseek-reasoner"
"#,
    )?;
    unsafe {
        env::set_var("SILICONFLOW_BASE_URL", "https://api.siliconflow.cn/v1");
        env::set_var("SILICONFLOW_MODEL", "deepseek-chat");
    }

    let config = Config::load(None, None)?;
    let providers = config.providers.as_ref().expect("providers");
    assert_eq!(
        providers.siliconflow.base_url.as_deref(),
        Some(DEFAULT_SILICONFLOW_BASE_URL)
    );
    assert_eq!(
        providers.siliconflow.model.as_deref(),
        Some(DEFAULT_SILICONFLOW_MODEL)
    );
    assert_eq!(
        providers.siliconflow_cn.base_url.as_deref(),
        Some(DEFAULT_SILICONFLOW_CN_BASE_URL)
    );
    assert_eq!(
        providers.siliconflow_cn.model.as_deref(),
        Some(DEFAULT_SILICONFLOW_FLASH_MODEL)
    );
    assert_eq!(config.deepseek_api_key()?, "sf-shared-key");
    assert_eq!(config.default_model(), DEFAULT_SILICONFLOW_FLASH_MODEL);
    Ok(())
}

#[test]
fn openrouter_custom_base_url_preserves_provider_model() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-or-custom-model-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"provider = "openrouter"

[providers.openrouter]
api_key = "or-table-key"
base_url = "https://gateway.example.com/v1"
model = "DeepSeek-V4-Pro"
"#,
    )?;

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Openrouter);
    assert_eq!(config.deepseek_api_key()?, "or-table-key");
    assert_eq!(config.deepseek_base_url(), "https://gateway.example.com/v1");
    assert_eq!(config.default_model(), "DeepSeek-V4-Pro");
    Ok(())
}

#[test]
fn novita_reads_provider_table_from_config_file() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-novita-table-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"provider = "novita"

[providers.novita]
api_key = "novita-table-key"
"#,
    )?;

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Novita);
    assert_eq!(config.deepseek_api_key()?, "novita-table-key");
    assert_eq!(config.deepseek_base_url(), DEFAULT_NOVITA_BASE_URL);
    Ok(())
}

#[test]
fn moonshot_kimi_import_is_api_key_only_and_never_reads_external_credentials() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-kimi-code-oauth-key-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let kimi_code_home = temp_root.join(".kimi-code");
    let credential_dir = kimi_code_home.join("credentials");
    fs::create_dir_all(&credential_dir)?;
    unsafe { env::set_var("KIMI_CODE_HOME", &kimi_code_home) };

    let credential = json!({
        "access_token": "must-never-be-read",
        "refresh_token": "must-never-be-used",
        "expires_at": SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs_f64()
            + 3600.0,
        "scope": "openid profile email",
        "token_type": "Bearer",
    });
    let credential_path = credential_dir.join("kimi-code.json");
    let credential_raw = serde_json::to_string(&credential)?;
    fs::write(&credential_path, &credential_raw)?;

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"provider = "moonshot"

[providers.moonshot]
auth_mode = "kimi_oauth"
api_key = "stale-api-key"
"#,
    )?;

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Moonshot);
    assert_eq!(config.deepseek_base_url(), DEFAULT_KIMI_CODE_BASE_URL);
    assert_eq!(config.default_model(), DEFAULT_KIMI_CODE_MODEL);
    let error = config
        .deepseek_api_key()
        .expect_err("Kimi external OAuth credentials are never imported");
    assert!(error.to_string().contains("does not impersonate"));
    assert!(
        error
            .to_string()
            .contains(KIMI_CODE_MEMBERSHIP_PLAN_CONSOLE_URL)
    );
    assert!(
        !error
            .to_string()
            .contains("https://platform.kimi.ai/console/api-keys")
    );
    assert!(!has_api_key_for(&config, ApiProvider::Moonshot));
    assert_eq!(
        fs::read_to_string(credential_path)?,
        credential_raw,
        "Codewhale must never read, refresh, or rewrite Kimi CLI credentials"
    );
    Ok(())
}

#[test]
fn moonshot_credential_help_keeps_direct_and_kimi_code_routes_distinct() {
    let direct =
        credential_help_for_provider_route(ApiProvider::Moonshot, DEFAULT_MOONSHOT_BASE_URL);
    assert_eq!(
        direct.credential_url,
        Some("https://platform.kimi.ai/console/api-keys")
    );
    assert_eq!(
        direct.docs_url,
        Some("https://platform.kimi.ai/docs/overview")
    );

    let kimi_code =
        credential_help_for_provider_route(ApiProvider::Moonshot, DEFAULT_KIMI_CODE_BASE_URL);
    assert_eq!(
        kimi_code.credential_url,
        Some(KIMI_CODE_MEMBERSHIP_PLAN_CONSOLE_URL)
    );
    assert_eq!(kimi_code.docs_url, None);
    assert!(kimi_code.guidance.contains("membership-plan API key"));
    assert!(
        kimi_code
            .guidance
            .contains("does not import Kimi CLI credentials")
    );
}

#[test]
fn codex_external_credentials_are_disabled_by_default_and_managed_fails_before_io() -> Result<()> {
    let _lock = lock_test_env();
    let temp = tempfile::tempdir()?;
    let temp_root = temp.path().canonicalize()?;
    let auth_path = temp_root.join("codex-auth.json");
    let token = crate::test_support::future_test_jwt("codex");
    let raw = serde_json::to_string_pretty(&json!({
        "tokens": {
            "access_token": token.clone(),
            "account_id": "acct-must-not-be-read",
            "refresh_token": "must-never-be-used"
        },
        "unknown": {"preserve": true}
    }))?;
    fs::write(&auth_path, &raw)?;
    let ambient_decoy = temp_root.join("ambient-decoy.json");
    let ambient_decoy_raw = r#"{"tokens":{"access_token":"must-not-be-read"}}"#;
    fs::write(&ambient_decoy, ambient_decoy_raw)?;
    let _auth_path = EnvVarGuard::set("OPENAI_CODEX_AUTH_FILE", &ambient_decoy);
    let _access = EnvVarGuard::remove("OPENAI_CODEX_ACCESS_TOKEN");
    let _legacy_access = EnvVarGuard::remove("CODEX_ACCESS_TOKEN");
    let _account = EnvVarGuard::remove("OPENAI_CODEX_ACCOUNT_ID");
    let _legacy_account = EnvVarGuard::remove("CODEX_ACCOUNT_ID");

    let disabled = Config {
        provider: Some(ApiProvider::OpenaiCodex.as_str().to_string()),
        ..Default::default()
    };
    crate::external_credentials::reset_side_effect_trap();
    assert!(!has_api_key_for(&disabled, ApiProvider::OpenaiCodex));
    let error = disabled
        .deepseek_api_key()
        .expect_err("external credentials default to disabled");
    assert!(error.to_string().contains("are disabled"));
    assert_eq!(disabled.codex_account_id(), None);
    assert_eq!(
        crate::external_credentials::side_effect_trap_counts(),
        (0, 0)
    );

    let mut managed_consent = codewhale_config::ExternalCredentialConsentToml::read_only(
        codewhale_config::ProviderKind::OpenaiCodex,
        codewhale_config::ExternalCredentialSource::CodexCli,
        auth_path.clone(),
    );
    managed_consent.access = codewhale_config::ExternalCredentialAccess::Managed;
    let managed = Config {
        provider: Some(ApiProvider::OpenaiCodex.as_str().to_string()),
        providers: Some(ProvidersConfig {
            openai_codex: ProviderConfig {
                auth_mode: Some("oauth".to_string()),
                external_credentials: Some(managed_consent),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };
    crate::external_credentials::reset_side_effect_trap();
    assert!(!has_api_key_for(&managed, ApiProvider::OpenaiCodex));
    let error = managed
        .deepseek_api_key()
        .expect_err("managed access needs a preservation adapter");
    assert!(
        error
            .to_string()
            .contains("schema-safe preservation adapter")
    );
    assert_eq!(
        crate::external_credentials::side_effect_trap_counts(),
        (0, 0)
    );
    assert_eq!(fs::read_to_string(&auth_path)?, raw);
    Ok(())
}

#[test]
fn codex_read_only_consent_reads_exact_file_without_mutation() -> Result<()> {
    let _lock = lock_test_env();
    let temp = tempfile::tempdir()?;
    let temp_root = temp.path().canonicalize()?;
    let auth_path = temp_root.join("codex-auth.json");
    let token = crate::test_support::future_test_jwt("codex");
    let raw = serde_json::to_string_pretty(&json!({
        "tokens": {
            "access_token": token.clone(),
            "account_id": "acct-read-only",
            "refresh_token": "must-never-be-used",
            "future_field": ["preserve"]
        },
        "future_top_level": true
    }))?;
    fs::write(&auth_path, &raw)?;
    let ambient_decoy = temp_root.join("ambient-decoy.json");
    let ambient_decoy_raw = r#"{"tokens":{"access_token":"must-not-be-read"}}"#;
    fs::write(&ambient_decoy, ambient_decoy_raw)?;
    let _auth_path = EnvVarGuard::set("OPENAI_CODEX_AUTH_FILE", &ambient_decoy);
    let _access = EnvVarGuard::remove("OPENAI_CODEX_ACCESS_TOKEN");
    let _legacy_access = EnvVarGuard::remove("CODEX_ACCESS_TOKEN");
    let config = Config {
        provider: Some(ApiProvider::OpenaiCodex.as_str().to_string()),
        providers: Some(ProvidersConfig {
            openai_codex: ProviderConfig {
                auth_mode: Some("oauth".to_string()),
                external_credentials: Some(
                    codewhale_config::ExternalCredentialConsentToml::read_only(
                        codewhale_config::ProviderKind::OpenaiCodex,
                        codewhale_config::ExternalCredentialSource::CodexCli,
                        auth_path.clone(),
                    ),
                ),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };

    let mut inactive = config.clone();
    inactive.provider = Some(ApiProvider::Deepseek.as_str().to_string());
    crate::external_credentials::reset_side_effect_trap();
    assert!(inactive.external_credential_read_consent_configured(
        ApiProvider::OpenaiCodex,
        codewhale_config::ExternalCredentialSource::CodexCli,
    ));
    let dormant_error = inactive
        .external_credential_read_grant(
            ApiProvider::OpenaiCodex,
            codewhale_config::ExternalCredentialSource::CodexCli,
            &ambient_decoy,
        )
        .expect_err("inactive providers cannot mint external read capabilities");
    assert!(dormant_error.to_string().contains("explicitly selected"));
    assert_eq!(
        crate::external_credentials::side_effect_trap_counts(),
        (0, 0)
    );

    let active_grant = config.external_credential_read_grant(
        ApiProvider::OpenaiCodex,
        codewhale_config::ExternalCredentialSource::CodexCli,
        &ambient_decoy,
    )?;
    assert_eq!(
        active_grant.path(),
        auth_path,
        "the selected route remains pinned to the persisted consent path"
    );

    crate::external_credentials::reset_side_effect_trap();
    assert_eq!(config.deepseek_api_key()?, token);
    assert_eq!(
        crate::external_credentials::side_effect_trap_counts(),
        (1, 1)
    );
    assert_eq!(fs::read_to_string(&auth_path)?, raw);
    assert_eq!(fs::read_to_string(&ambient_decoy)?, ambient_decoy_raw);

    drop(_access);
    let _process_access = EnvVarGuard::set("OPENAI_CODEX_ACCESS_TOKEN", "process-token");
    crate::external_credentials::reset_side_effect_trap();
    assert_eq!(config.deepseek_api_key()?, "process-token");
    assert_eq!(config.codex_account_id(), None);
    assert_eq!(
        crate::external_credentials::side_effect_trap_counts(),
        (0, 0),
        "process-scoped Codex auth must not be mixed with external-file metadata"
    );
    Ok(())
}

#[test]
fn moonshot_kimi_code_api_key_uses_coding_model() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-kimi-code-key-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"provider = "moonshot"

[providers.moonshot]
api_key = "kimi-code-key"
base_url = "https://api.kimi.com/coding/v1"
"#,
    )?;

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Moonshot);
    assert_eq!(config.deepseek_base_url(), DEFAULT_KIMI_CODE_BASE_URL);
    assert_eq!(config.default_model(), DEFAULT_KIMI_CODE_MODEL);
    assert_eq!(config.deepseek_api_key()?, "kimi-code-key");
    assert!(has_api_key_for(&config, ApiProvider::Moonshot));
    Ok(())
}

#[test]
fn moonshot_kimi_code_missing_key_reports_membership_plan_console() -> Result<()> {
    let _lock = lock_test_env();
    let temp = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp.path());
    let config = Config {
        provider: Some(ApiProvider::Moonshot.as_str().to_string()),
        providers: Some(ProvidersConfig {
            moonshot: ProviderConfig {
                base_url: Some(DEFAULT_KIMI_CODE_BASE_URL.to_string()),
                model: Some(KIMI_CODE_K3_MODEL.to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };

    let error = config
        .deepseek_api_key()
        .expect_err("Kimi Code route needs a membership-plan API key");
    let message = error.to_string();
    assert!(
        message.contains(KIMI_CODE_MEMBERSHIP_PLAN_CONSOLE_URL),
        "{message}"
    );
    assert!(message.contains("api.kimi.com/coding/v1"), "{message}");
    assert!(
        message.contains("does not import Kimi CLI credentials"),
        "{message}"
    );
    assert!(!message.contains("https://platform.kimi.ai/console/api-keys"));
    Ok(())
}

#[test]
fn moonshot_kimi_code_saved_claude_k3_1m_alias_fails_with_api_model_guidance() -> Result<()> {
    let _lock = lock_test_env();
    let temp = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp.path());
    let config_path = temp.path().join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"provider = "moonshot"

[providers.moonshot]
api_key = "kimi-code-key"
base_url = "https://api.kimi.com/coding/v1"
model = "k3[1m]"
"#,
    )?;

    let error = Config::load(None, None)
        .expect_err("saved Claude Code context hints must not become API model ids");
    let message = error.to_string();
    assert!(message.contains("model = \"k3\""), "{message}");
    assert!(message.contains("context_window = 1048576"), "{message}");
    assert!(message.contains("plan includes 1M context"), "{message}");
    assert!(message.contains("262144 safe default"), "{message}");
    Ok(())
}

/// Env-var-only path: `CODEWHALE_BASE_URL=https://api.kimi.com/coding/v1`
/// combined with `CODEWHALE_PROVIDER=moonshot` must trigger Kimi Code
/// model selection even when the TOML has no `base_url`.
#[test]
fn moonshot_kimi_code_env_base_url_selects_coding_model() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-kimi-code-env-url-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"[providers.moonshot]
api_key = "kimi-code-env-key"
"#,
    )?;
    // Safety: test-only env mutation guarded by lock_test_env().
    unsafe {
        env::set_var("CODEWHALE_PROVIDER", "moonshot");
        env::set_var("CODEWHALE_BASE_URL", "https://api.kimi.com/coding/v1");
    }

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Moonshot);
    assert_eq!(config.deepseek_base_url(), DEFAULT_KIMI_CODE_BASE_URL);
    assert_eq!(config.default_model(), DEFAULT_KIMI_CODE_MODEL);
    assert_eq!(config.deepseek_api_key()?, "kimi-code-env-key");
    assert!(has_api_key_for(&config, ApiProvider::Moonshot));
    Ok(())
}

/// Regression for issue #2160: a stale root `default_text_model` carried
/// over from a DeepSeek setup must not steer the Kimi Code endpoint to
/// `deepseek-v4-pro`. The user-facing trigger here is the legacy
/// `DEEPSEEK_PROVIDER` env var (still produced by the `codewhale
/// --provider moonshot` dispatcher for compat); the test also has a
/// `CODEWHALE_PROVIDER` twin below for the public env path.
#[test]
fn moonshot_kimi_code_model_overrides_root_deepseek_default() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-kimi-code-root-model-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"provider = "deepseek"
default_text_model = "deepseek-v4-pro"

[providers.moonshot]
api_key = "kimi-code-key"
base_url = "https://api.kimi.com/coding/v1"
"#,
    )?;
    // Safety: test-only env mutation guarded by lock_test_env().
    unsafe { env::set_var("DEEPSEEK_PROVIDER", "moonshot") };

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Moonshot);
    assert_eq!(config.deepseek_base_url(), DEFAULT_KIMI_CODE_BASE_URL);
    assert_eq!(config.default_model(), DEFAULT_KIMI_CODE_MODEL);
    Ok(())
}

/// Same regression as above, but driven by the public `CODEWHALE_PROVIDER`
/// env var. Documents the recommended user-facing setup path: never
/// `DEEPSEEK_PROVIDER=moonshot`, always `CODEWHALE_PROVIDER=moonshot`
/// (or `codewhale --provider moonshot`, which also resolves through
/// this code path internally).
#[test]
fn moonshot_kimi_code_model_resolves_via_codewhale_provider_env() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-kimi-code-cw-env-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"provider = "deepseek"
default_text_model = "deepseek-v4-pro"

[providers.moonshot]
api_key = "kimi-code-key"
base_url = "https://api.kimi.com/coding/v1"
"#,
    )?;
    // Safety: test-only env mutation guarded by lock_test_env().
    unsafe { env::set_var("CODEWHALE_PROVIDER", "moonshot") };

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Moonshot);
    assert_eq!(config.deepseek_base_url(), DEFAULT_KIMI_CODE_BASE_URL);
    assert_eq!(config.default_model(), DEFAULT_KIMI_CODE_MODEL);
    Ok(())
}

/// `CODEWHALE_PROVIDER` wins when both it and the legacy
/// `DEEPSEEK_PROVIDER` are set, so a user adding the new alias to their
/// shell isn't surprised by a stale legacy export.
#[test]
fn codewhale_provider_env_takes_precedence_over_deepseek_provider() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-cw-vs-ds-provider-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(&config_path, "provider = \"deepseek\"\n")?;
    // Safety: test-only env mutation guarded by lock_test_env().
    unsafe {
        env::set_var("CODEWHALE_PROVIDER", "moonshot");
        env::set_var("DEEPSEEK_PROVIDER", "openrouter");
    }

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Moonshot);
    Ok(())
}

/// Moonshot Platform path: when [providers.moonshot] is empty (or
/// missing) and no Kimi Code endpoint is configured, the resolver
/// defaults to the Moonshot Platform base URL and the latest Kimi platform
/// model. This is the "I have a Moonshot Platform API key, not a
/// Kimi Code plan key" path.
#[test]
fn moonshot_platform_defaults_to_kimi_k27_code() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-moonshot-platform-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"provider = "moonshot"

[providers.moonshot]
api_key = "moonshot-platform-key"
"#,
    )?;

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Moonshot);
    assert_eq!(config.deepseek_base_url(), DEFAULT_MOONSHOT_BASE_URL);
    assert_eq!(config.default_model(), DEFAULT_MOONSHOT_MODEL);
    assert_eq!(config.deepseek_api_key()?, "moonshot-platform-key");
    Ok(())
}

#[test]
fn has_api_key_for_detects_env_and_config_per_provider() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-has-key-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let mut config = Config::default();
    assert!(!has_api_key_for(&config, ApiProvider::Openai));
    assert!(!has_api_key_for(&config, ApiProvider::WanjieArk));
    assert!(!has_api_key_for(&config, ApiProvider::Volcengine));
    assert!(!has_api_key_for(&config, ApiProvider::Openrouter));
    assert!(!has_api_key_for(&config, ApiProvider::XiaomiMimo));
    assert!(!has_api_key_for(&config, ApiProvider::Siliconflow));
    assert!(
        has_api_key_for(&config, ApiProvider::Sglang),
        "SGLang is self-hosted and does not require a key by default"
    );
    assert!(
        has_api_key_for(&config, ApiProvider::Vllm),
        "vLLM is self-hosted and does not require a key by default"
    );

    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe {
        env::set_var("OPENROUTER_API_KEY", "or-env");
        env::set_var("OPENAI_API_KEY", "openai-env");
        env::set_var("WANJIE_API_KEY", "wanjie-env");
        env::set_var("ARK_API_KEY", "volc-env");
        env::set_var("MIMO_API_KEY", "mimo-env");
        env::set_var("SILICONFLOW_API_KEY", "sf-env");
    }
    assert!(has_api_key_for(&config, ApiProvider::Openai));
    assert!(has_api_key_for(&config, ApiProvider::WanjieArk));
    assert!(has_api_key_for(&config, ApiProvider::Volcengine));
    assert!(has_api_key_for(&config, ApiProvider::Openrouter));
    assert!(has_api_key_for(&config, ApiProvider::XiaomiMimo));
    assert!(has_api_key_for(&config, ApiProvider::Siliconflow));
    assert!(!has_api_key_for(&config, ApiProvider::Novita));

    // Safety: test-only environment mutation guarded by a global mutex.
    unsafe {
        env::remove_var("OPENROUTER_API_KEY");
        env::remove_var("OPENAI_API_KEY");
        env::remove_var("WANJIE_API_KEY");
        env::remove_var("ARK_API_KEY");
        env::remove_var("MIMO_API_KEY");
        env::remove_var("SILICONFLOW_API_KEY");
    }
    let mut providers = ProvidersConfig::default();
    providers.openai.api_key = Some("file-openai".to_string());
    providers.wanjie_ark.api_key = Some("file-wanjie".to_string());
    providers.xiaomi_mimo.api_key = Some("file-mimo".to_string());
    providers.novita.api_key = Some("file-novita".to_string());
    providers.siliconflow.api_key = Some("file-siliconflow".to_string());
    config.providers = Some(providers);
    assert!(has_api_key_for(&config, ApiProvider::Openai));
    assert!(has_api_key_for(&config, ApiProvider::WanjieArk));
    assert!(has_api_key_for(&config, ApiProvider::XiaomiMimo));
    assert!(has_api_key_for(&config, ApiProvider::Novita));
    assert!(has_api_key_for(&config, ApiProvider::Siliconflow));
    assert!(!has_api_key_for(&config, ApiProvider::Openrouter));
    Ok(())
}

#[test]
fn provider_auth_source_metadata_is_not_a_runtime_credential() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let mut providers = ProvidersConfig::default();
    providers.openai.auth = Some(codewhale_config::ProviderAuthSourceToml {
        source: codewhale_config::AuthSourceKind::Command,
        command: vec!["secret-tool".to_string(), "lookup".to_string()],
        timeout_ms: Some(2000),
        secret_id: None,
    });
    let config = Config {
        provider: Some("openai".to_string()),
        providers: Some(providers),
        ..Config::default()
    };

    assert!(!has_api_key_for(&config, ApiProvider::Openai));
    assert!(config.deepseek_api_key().is_err());
    Ok(())
}

#[test]
fn xai_oauth_selection_falls_back_to_explicit_api_key_without_external_io() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-xai-auth-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let auth_path = temp_root.join("auth.json");
    let _auth_path = EnvVarGuard::set("GROK_AUTH_PATH", auth_path.as_os_str());
    let _xai_key = EnvVarGuard::remove("XAI_API_KEY");

    let mut providers = ProvidersConfig::default();
    providers.xai.api_key = Some("fake-xai-cfg-key".to_string());
    providers.xai.auth_mode = Some("oauth".to_string());
    let api_key_config = Config {
        provider: Some("xai".to_string()),
        providers: Some(providers),
        ..Config::default()
    };
    crate::external_credentials::reset_side_effect_trap();
    assert!(has_api_key_for(&api_key_config, ApiProvider::Xai));
    assert_eq!(api_key_config.deepseek_api_key()?, "fake-xai-cfg-key");
    assert_eq!(
        crate::external_credentials::side_effect_trap_counts(),
        (0, 0)
    );

    fs::write(&auth_path, "{}")?;
    assert!(!has_api_key_for(&Config::default(), ApiProvider::Xai));
    fs::remove_dir_all(temp_root)?;
    Ok(())
}

#[test]
fn xai_invalid_owned_generation_blocks_external_and_uses_api_key_fallback() -> Result<()> {
    let _lock = lock_test_env();
    let root = tempfile::tempdir()?;
    let root = root.path().canonicalize()?;
    let external_path = root.join("grok-auth.json");
    let external_raw = r#"{"token":"external-owner-bytes-must-not-be-read"}"#;
    fs::write(&external_path, external_raw)?;
    let _home = EnvVarGuard::set("CODEWHALE_HOME", &root);
    let _auth_path = EnvVarGuard::set("GROK_AUTH_PATH", &external_path);
    let _xai_key = EnvVarGuard::remove("XAI_API_KEY");
    let _xai_base_url = EnvVarGuard::remove("XAI_BASE_URL");

    let mut providers = ProvidersConfig::default();
    providers.xai.api_key = Some("fake-xai-cfg-key".to_string());
    providers.xai.auth_mode = Some("oauth".to_string());
    providers.xai.oauth_credential_generation = Some("../unsafe.json".to_string());
    providers.xai.external_credentials =
        Some(codewhale_config::ExternalCredentialConsentToml::read_only(
            codewhale_config::ProviderKind::Xai,
            codewhale_config::ExternalCredentialSource::GrokCli,
            external_path.clone(),
        ));
    let config = Config {
        provider: Some(ApiProvider::Xai.as_str().to_string()),
        providers: Some(providers),
        ..Config::default()
    };

    crate::external_credentials::reset_side_effect_trap();
    assert!(
        !crate::xai_oauth::credentials_present(&config),
        "an invalid owned generation pointer must not resolve external OAuth"
    );
    assert_eq!(config.deepseek_api_key()?, "fake-xai-cfg-key");
    assert_eq!(
        crate::external_credentials::side_effect_trap_counts(),
        (0, 0),
        "an unusable owned generation must not access the external Grok CLI"
    );
    assert_eq!(fs::read_to_string(external_path)?, external_raw);
    Ok(())
}

#[test]
fn save_api_key_for_openrouter_writes_provider_table() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-save-key-or-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);
    let config_path = temp_root.join(".deepseek").join("config.toml");
    let _config_path = EnvVarGuard::set("CODEWHALE_CONFIG_PATH", config_path.as_os_str());
    let _secret_backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "local");
    let resolved_config_path = codewhale_config::resolve_config_path(None)?;

    let path = save_api_key_for(ApiProvider::Openrouter, "or-saved-key")?;
    assert_eq!(path, resolved_config_path);
    let contents = fs::read_to_string(&path)?;
    let parsed: toml::Value = toml::from_str(&contents)?;
    assert_eq!(
        parsed
            .get("providers")
            .and_then(|p| p.get("openrouter"))
            .and_then(|t| t.get("api_key"))
            .and_then(toml::Value::as_str),
        Some("or-saved-key")
    );
    // Re-saving must not duplicate or wipe sibling tables.
    let novita_path = save_api_key_for(ApiProvider::Novita, "novita-saved-key")?;
    assert_eq!(novita_path.canonicalize()?, path.canonicalize()?);
    let contents = fs::read_to_string(&path)?;
    let parsed: toml::Value = toml::from_str(&contents)?;
    assert_eq!(
        parsed
            .get("providers")
            .and_then(|p| p.get("openrouter"))
            .and_then(|t| t.get("api_key"))
            .and_then(toml::Value::as_str),
        Some("or-saved-key")
    );
    assert_eq!(
        parsed
            .get("providers")
            .and_then(|p| p.get("novita"))
            .and_then(|t| t.get("api_key"))
            .and_then(toml::Value::as_str),
        Some("novita-saved-key")
    );
    for (provider, key) in [
        (ApiProvider::Openai, "openai-saved-key"),
        (ApiProvider::WanjieArk, "wanjie-saved-key"),
        (ApiProvider::Fireworks, "fireworks-saved-key"),
        (ApiProvider::XiaomiMimo, "mimo-saved-key"),
        (ApiProvider::Siliconflow, "sf-saved-key"),
        (ApiProvider::Sglang, "sglang-saved-key"),
    ] {
        assert_eq!(
            save_api_key_for(provider, key)?.canonicalize()?,
            path.canonicalize()?
        );
    }
    let contents = fs::read_to_string(&path)?;
    let parsed: toml::Value = toml::from_str(&contents)?;
    assert_eq!(
        parsed
            .get("providers")
            .and_then(|p| p.get("openai"))
            .and_then(|t| t.get("api_key"))
            .and_then(toml::Value::as_str),
        Some("openai-saved-key")
    );
    assert_eq!(
        parsed
            .get("providers")
            .and_then(|p| p.get("wanjie_ark"))
            .and_then(|t| t.get("api_key"))
            .and_then(toml::Value::as_str),
        Some("wanjie-saved-key")
    );
    assert_eq!(
        parsed
            .get("providers")
            .and_then(|p| p.get("fireworks"))
            .and_then(|t| t.get("api_key"))
            .and_then(toml::Value::as_str),
        Some("fireworks-saved-key")
    );
    assert_eq!(
        parsed
            .get("providers")
            .and_then(|p| p.get("xiaomi_mimo"))
            .and_then(|t| t.get("api_key"))
            .and_then(toml::Value::as_str),
        Some("mimo-saved-key")
    );
    assert_eq!(
        parsed
            .get("providers")
            .and_then(|p| p.get("siliconflow"))
            .and_then(|t| t.get("api_key"))
            .and_then(toml::Value::as_str),
        Some("sf-saved-key")
    );
    assert_eq!(
        parsed
            .get("providers")
            .and_then(|p| p.get("sglang"))
            .and_then(|t| t.get("api_key"))
            .and_then(toml::Value::as_str),
        Some("sglang-saved-key")
    );
    save_api_key_for(ApiProvider::SiliconflowCn, "sf-cn-saved-key")?;
    let contents = fs::read_to_string(&path)?;
    let parsed: toml::Value = toml::from_str(&contents)?;
    assert_eq!(
        parsed
            .get("providers")
            .and_then(|p| p.get("siliconflow_cn"))
            .and_then(|t| t.get("api_key"))
            .and_then(toml::Value::as_str),
        Some("sf-cn-saved-key")
    );
    assert_eq!(
        parsed
            .get("providers")
            .and_then(|p| p.get("siliconflow"))
            .and_then(|t| t.get("api_key"))
            .and_then(toml::Value::as_str),
        Some("sf-saved-key")
    );
    Ok(())
}

#[test]
fn save_api_key_for_deepseek_cn_uses_root_deepseek_storage() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-save-key-cn-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);
    let config_path = temp_root.join(".deepseek").join("config.toml");
    let _config_path = EnvVarGuard::set("CODEWHALE_CONFIG_PATH", config_path.as_os_str());
    let _secret_backend = EnvVarGuard::set("DEEPSEEK_SECRET_BACKEND", "local");

    let path = save_api_key_for(ApiProvider::DeepseekCN, "cn-saved-key")?;
    assert_eq!(path, config_path);
    let contents = fs::read_to_string(&path)?;
    let parsed: toml::Value = toml::from_str(&contents)?;

    assert_eq!(
        parsed.get("api_key").and_then(toml::Value::as_str),
        Some("cn-saved-key")
    );
    Ok(())
}

#[test]
fn modelstudio_variants_share_one_secret_slot_and_key_availability() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let codewhale_home = temp_root.path().join("codewhale-home");
    let config_path = codewhale_home.join("config.toml");
    let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());
    let _config_path = EnvVarGuard::set("CODEWHALE_CONFIG_PATH", config_path.as_os_str());
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
    let _ms_env = EnvVarGuard::remove("MODELSTUDIO_API_KEY");
    let _dashscope_env = EnvVarGuard::remove("DASHSCOPE_API_KEY");
    let _cli_source = EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
    let _cli_key = EnvVarGuard::remove("CODEWHALE_CLI_API_KEY");

    let variants = [
        ApiProvider::ModelstudioTokenPlan,
        ApiProvider::ModelstudioTokenPlanAnthropic,
        ApiProvider::ModelstudioCodingPlan,
        ApiProvider::ModelstudioCodingPlanAnthropic,
    ];
    for variant in variants {
        assert_eq!(
            provider_secret_store_slot(variant),
            "modelstudio-token-plan",
            "{variant:?} must share the family's one credential slot"
        );
    }

    // Saving on the Token Plan variant writes the single family slot only.
    save_api_key_for(ApiProvider::ModelstudioTokenPlan, "ms-family-key")?;
    let secrets = codewhale_secrets::Secrets::auto_detect();
    assert_eq!(
        secrets.get("modelstudio-token-plan")?,
        Some("ms-family-key".to_string())
    );
    assert_eq!(secrets.get("modelstudio-coding-plan")?, None);

    // Every variant — active or not — resolves the family key, so the picker
    // badge stops showing three bogus "missing key" rows after one save.
    let inactive_variants = Config::load(Some(config_path.clone()), None)?;
    for variant in variants {
        assert_eq!(
            provider_secret_store_api_key(&inactive_variants, variant).as_deref(),
            Some("ms-family-key"),
            "{variant:?} must read the family slot"
        );
        assert!(
            has_api_key_for(&inactive_variants, variant),
            "{variant:?} key-availability badge must resolve the family key"
        );
    }

    // Saving on any sibling variant overwrites the same shared slot.
    save_api_key_for(
        ApiProvider::ModelstudioCodingPlanAnthropic,
        "ms-family-key-v2",
    )?;
    assert_eq!(
        secrets.get("modelstudio-token-plan")?,
        Some("ms-family-key-v2".to_string())
    );
    assert_eq!(secrets.get("modelstudio-coding-plan-anthropic")?, None);
    let reloaded = Config::load(Some(config_path), None)?;
    for variant in variants {
        assert_eq!(
            provider_secret_store_api_key(&reloaded, variant).as_deref(),
            Some("ms-family-key-v2"),
            "{variant:?} must follow the family slot across saves"
        );
    }
    Ok(())
}

#[test]
fn nvidia_nim_reads_facade_provider_table() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-nim-provider-table-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"provider = "nvidia-nim"
default_text_model = "deepseek-v4-flash"

[providers.nvidia_nim]
api_key = "nim-table-key"
base_url = "https://nim-table.example/v1"
model = "deepseek-v4-pro"
"#,
    )?;

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::NvidiaNim);
    assert_eq!(config.deepseek_api_key()?, "nim-table-key");
    assert_eq!(config.deepseek_base_url(), "https://nim-table.example/v1");
    // Custom base URL preserves the user-specified model name; normalisation
    // is skipped because the gateway expects the model name as-provided.
    assert_eq!(config.default_model(), "deepseek-v4-pro");
    Ok(())
}

#[test]
fn nvidia_nim_provider_table_key_overrides_root_deepseek_key() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-nim-root-key-precedence-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"api_key = "codewhale-root-key"
provider = "nvidia-nim"

[providers.nvidia_nim]
api_key = "nim-table-key"
base_url = "https://integrate.api.nvidia.com/v1"
model = "deepseek-ai/deepseek-v4-pro"
"#,
    )?;

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::NvidiaNim);
    assert_eq!(config.deepseek_api_key()?, "nim-table-key");
    Ok(())
}

// ========================================================================
// Provider Capability Matrix tests
// ========================================================================

#[test]
fn provider_capability_scenario() {
    // Scenario consolidation of: provider_capability_deepseek_v4_pro_has_1m_window_and_thinking, provider_capability_deepseek_anthropic_uses_messages_payload, provider_capability_openmodel_uses_messages_payload, provider_capability_deepseek_v4_flash_has_1m_window_and_thinking, provider_capability_deepseek_chat_alias_has_v4_flash_caps_and_metadata, provider_capability_deepseek_reasoner_alias_has_v4_flash_caps_and_metadata, provider_capability_deepseek_v4_flash_has_no_alias_deprecation, provider_capability_nvidia_nim_v4_pro_maps_correctly
    // from provider_capability_deepseek_v4_pro_has_1m_window_and_thinking
    {
        let cap = provider_capability(ApiProvider::Deepseek, "deepseek-v4-pro");
        assert_eq!(
            cap.context_window,
            crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, Some(384_000));
        assert!(cap.thinking_supported);
        assert!(cap.cache_telemetry_supported);
        assert_eq!(
            cap.request_payload_mode,
            RequestPayloadMode::ChatCompletions
        );
    }
    // from provider_capability_deepseek_anthropic_uses_messages_payload
    {
        let cap = provider_capability(
            ApiProvider::DeepseekAnthropic,
            DEFAULT_DEEPSEEK_ANTHROPIC_MODEL,
        );
        assert_eq!(
            cap.context_window,
            crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, Some(384_000));
        assert!(cap.thinking_supported);
        assert!(!cap.cache_telemetry_supported);
        assert_eq!(
            cap.request_payload_mode,
            RequestPayloadMode::AnthropicMessages
        );
        assert!(cap.alias_deprecation.is_none());
    }
    // from provider_capability_openmodel_uses_messages_payload
    {
        let cap = provider_capability(ApiProvider::Openmodel, DEFAULT_OPENMODEL_MODEL);
        assert_eq!(cap.resolved_model, DEFAULT_OPENMODEL_MODEL);
        assert_eq!(
            cap.context_window,
            crate::models::context_window_for_model(DEFAULT_OPENMODEL_MODEL).unwrap_or(200_000)
        );
        assert_eq!(
            cap.max_output,
            Some(
                crate::models::max_output_tokens_for_model(DEFAULT_OPENMODEL_MODEL)
                    .unwrap_or(64_000)
            )
        );
        assert!(!cap.cache_telemetry_supported);
        assert_eq!(
            cap.request_payload_mode,
            RequestPayloadMode::AnthropicMessages
        );
        assert!(provider_passes_model_through(ApiProvider::Openmodel));
    }
    // from provider_capability_deepseek_v4_flash_has_1m_window_and_thinking
    {
        let cap = provider_capability(ApiProvider::Deepseek, "deepseek-v4-flash");
        assert_eq!(
            cap.context_window,
            crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, Some(384_000));
        assert!(cap.thinking_supported);
        assert!(cap.cache_telemetry_supported);
    }
    // from provider_capability_deepseek_chat_alias_has_v4_flash_caps_and_metadata
    {
        let cap = provider_capability(ApiProvider::Deepseek, "deepseek-chat");
        assert_eq!(
            cap.context_window,
            crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, Some(384_000));
        assert!(cap.thinking_supported);
        assert!(cap.cache_telemetry_supported);

        let deprecation = cap
            .alias_deprecation
            .as_ref()
            .expect("alias deprecation metadata");
        assert_eq!(deprecation.alias, "deepseek-chat");
        assert_eq!(deprecation.replacement, "deepseek-v4-flash");
        assert_eq!(deprecation.retirement_date, "2026-07-24");
        assert_eq!(deprecation.retirement_utc, "2026-07-24T15:59:00Z");
    }
    // from provider_capability_deepseek_reasoner_alias_has_v4_flash_caps_and_metadata
    {
        let cap = provider_capability(ApiProvider::Deepseek, "deepseek-reasoner");
        assert_eq!(
            cap.context_window,
            crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, Some(384_000));
        assert!(cap.thinking_supported);
        assert!(cap.cache_telemetry_supported);

        let deprecation = cap
            .alias_deprecation
            .as_ref()
            .expect("alias deprecation metadata");
        assert_eq!(deprecation.alias, "deepseek-reasoner");
        assert_eq!(deprecation.replacement, "deepseek-v4-flash");
    }
    // from provider_capability_deepseek_v4_flash_has_no_alias_deprecation
    {
        let cap = provider_capability(ApiProvider::Deepseek, "deepseek-v4-flash");
        assert!(cap.alias_deprecation.is_none());
    }
    // from provider_capability_nvidia_nim_v4_pro_maps_correctly
    {
        let cap = provider_capability(ApiProvider::NvidiaNim, DEFAULT_NVIDIA_NIM_MODEL);
        assert_eq!(
            cap.context_window,
            crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, Some(384_000));
        assert!(cap.thinking_supported);
        assert!(cap.cache_telemetry_supported);
        assert_eq!(
            cap.request_payload_mode,
            RequestPayloadMode::ChatCompletions
        );
    }
}

#[test]
fn provider_capability_scenario_2() {
    // Scenario consolidation of: provider_capability_nvidia_nim_v4_flash_maps_correctly, provider_capability_openai_codex_uses_responses_payload, provider_capability_marks_exact_inkling_route_as_reasoning, provider_capability_xiaomi_mimo_has_thinking_no_cache, provider_capability_novita_v4_pro_has_thinking_no_cache, provider_capability_fireworks_v4_pro_has_thinking_no_cache, provider_capability_siliconflow_v4_pro_has_thinking_no_cache, provider_capability_sglang_v4_pro_has_thinking_no_cache
    // from provider_capability_nvidia_nim_v4_flash_maps_correctly
    {
        let cap = provider_capability(ApiProvider::NvidiaNim, DEFAULT_NVIDIA_NIM_FLASH_MODEL);
        assert_eq!(
            cap.context_window,
            crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, Some(384_000));
        assert!(cap.thinking_supported);
        assert!(cap.cache_telemetry_supported);
    }
    // from provider_capability_openai_codex_uses_responses_payload
    {
        let cap = provider_capability(ApiProvider::OpenaiCodex, DEFAULT_OPENAI_CODEX_MODEL);
        assert_eq!(cap.provider, ApiProvider::OpenaiCodex);
        assert_eq!(cap.resolved_model, DEFAULT_OPENAI_CODEX_MODEL);
        assert_eq!(
            cap.context_window,
            OPENAI_CODEX_EFFECTIVE_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, Some(4096));
        assert!(cap.thinking_supported);
        assert!(!cap.cache_telemetry_supported);
        assert_eq!(cap.request_payload_mode, RequestPayloadMode::Responses);
    }
    // from provider_capability_marks_exact_inkling_route_as_reasoning
    {
        let cap = provider_capability(ApiProvider::Together, TOGETHER_INKLING_MODEL);
        assert!(cap.thinking_supported);
        assert_eq!(
            crate::models::context_window_for_model(TOGETHER_INKLING_MODEL),
            None
        );
        assert_eq!(
            crate::models::max_output_tokens_for_model(TOGETHER_INKLING_MODEL),
            None
        );
    }
    // from provider_capability_xiaomi_mimo_has_thinking_no_cache
    {
        let cap = provider_capability(ApiProvider::XiaomiMimo, DEFAULT_XIAOMI_MIMO_MODEL);
        assert_eq!(cap.context_window, 1_000_000);
        assert_eq!(cap.max_output, Some(131_072));
        assert!(cap.thinking_supported);
        assert!(!cap.cache_telemetry_supported);
        assert_eq!(
            cap.request_payload_mode,
            RequestPayloadMode::ChatCompletions
        );

        let omni = provider_capability(ApiProvider::XiaomiMimo, XIAOMI_MIMO_V2_5_OMNI_MODEL);
        assert_eq!(omni.context_window, 1_000_000);
        assert_eq!(omni.max_output, Some(131_072));
        assert!(omni.thinking_supported);
        assert!(!omni.cache_telemetry_supported);
    }
    // from provider_capability_novita_v4_pro_has_thinking_no_cache
    {
        let cap = provider_capability(ApiProvider::Novita, DEFAULT_NOVITA_MODEL);
        assert_eq!(
            cap.context_window,
            crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, Some(384_000));
        assert!(cap.thinking_supported);
        assert!(!cap.cache_telemetry_supported);
    }
    // from provider_capability_fireworks_v4_pro_has_thinking_no_cache
    {
        let cap = provider_capability(ApiProvider::Fireworks, DEFAULT_FIREWORKS_MODEL);
        assert_eq!(
            cap.context_window,
            crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, Some(384_000));
        assert!(cap.thinking_supported);
        assert!(!cap.cache_telemetry_supported);
    }
    // from provider_capability_siliconflow_v4_pro_has_thinking_no_cache
    {
        let cap = provider_capability(ApiProvider::Siliconflow, DEFAULT_SILICONFLOW_MODEL);
        assert_eq!(
            cap.context_window,
            crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, Some(384_000));
        assert!(cap.thinking_supported);
        assert!(!cap.cache_telemetry_supported);
        assert_eq!(
            cap.request_payload_mode,
            RequestPayloadMode::ChatCompletions
        );
    }
    // from provider_capability_sglang_v4_pro_has_thinking_no_cache
    {
        let cap = provider_capability(ApiProvider::Sglang, DEFAULT_SGLANG_MODEL);
        assert_eq!(
            cap.context_window,
            crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, Some(384_000));
        assert!(cap.thinking_supported);
        assert!(!cap.cache_telemetry_supported);
    }
}

#[test]
fn provider_capability_openrouter_v4_pro_has_thinking_no_cache() {
    let cap = provider_capability(ApiProvider::Openrouter, DEFAULT_OPENROUTER_MODEL);
    assert_eq!(
        cap.context_window,
        crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
    );
    assert_eq!(cap.max_output, Some(384_000));
    assert!(cap.thinking_supported);
    // OpenRouter does not return DeepSeek prompt-cache telemetry.
    assert!(!cap.cache_telemetry_supported);
    assert_eq!(
        cap.request_payload_mode,
        RequestPayloadMode::ChatCompletions
    );
}

#[test]
fn invalid_provider_scenario() {
    // Scenario consolidation of: invalid_provider_auth_source_is_not_explicit_configuration, invalid_provider_error_lists_huggingface
    // from invalid_provider_auth_source_is_not_explicit_configuration
    {
        let entry = ProviderConfig {
            auth: Some(codewhale_config::ProviderAuthSourceToml {
                source: codewhale_config::AuthSourceKind::Command,
                command: Vec::new(),
                timeout_ms: None,
                secret_id: None,
            }),
            ..ProviderConfig::default()
        };

        assert!(!provider_config_is_explicit(&entry));
    }
    // from invalid_provider_error_lists_huggingface
    {
        let config = Config {
            provider: Some("not-a-provider".to_string()),
            ..Default::default()
        };
        let err = config.validate().expect_err("unknown provider should fail");
        let message = err.to_string();
        assert!(message.contains("Invalid provider 'not-a-provider'"));
        assert!(message.contains("huggingface"));
    }
}

#[test]
fn provider_capability_openrouter_recent_large_models_are_reasoning_aware() {
    for (model, expected_window, expected_output) in [
        (
            OPENROUTER_ARCEE_TRINITY_LARGE_THINKING_MODEL,
            262_144,
            262_144,
        ),
        (OPENROUTER_QWEN_3_6_FLASH_MODEL, 1_000_000, 65_536),
        // Output caps vendor-verified at 65,536 (MODEL_PROVIDER_AUDIT A2/D-7).
        (OPENROUTER_QWEN_3_6_35B_A3B_MODEL, 262_144, 65_536),
        (OPENROUTER_QWEN_3_6_MAX_PREVIEW_MODEL, 262_144, 65_536),
        (OPENROUTER_QWEN_3_6_27B_MODEL, 262_144, 65_536),
        (OPENROUTER_QWEN_3_6_PLUS_MODEL, 1_000_000, 65_536),
        (OPENROUTER_XIAOMI_MIMO_V2_5_PRO_MODEL, 1_000_000, 131_072),
        (OPENROUTER_MINIMAX_M3_MODEL, 1_000_000, 524_288),
        (OPENROUTER_MINIMAX_M2_7_MODEL, 204_800, 131_072),
        (OPENROUTER_GLM_5_1_MODEL, 202_752, 131_072),
        (OPENROUTER_GLM_5_2_MODEL, 1_000_000, 131_072),
        (OPENROUTER_NEMOTRON_3_ULTRA_MODEL, 1_000_000, 16_384),
    ] {
        let cap = provider_capability(ApiProvider::Openrouter, model);

        assert_eq!(cap.context_window, expected_window);
        assert_eq!(cap.max_output, Some(expected_output));
        assert!(cap.thinking_supported);
        assert!(!cap.cache_telemetry_supported);
        assert_eq!(
            cap.request_payload_mode,
            RequestPayloadMode::ChatCompletions
        );
    }
}

#[test]
fn openrouter_nemotron_ultra_aliases_resolve_to_live_id() {
    assert_eq!(
        OPENROUTER_NEMOTRON_3_ULTRA_MODEL,
        "nvidia/nemotron-3-ultra-550b-a55b"
    );
    assert_ne!(OPENROUTER_NEMOTRON_3_ULTRA_MODEL, "nvidia/nemotron-3-ultra");

    for alias in [
        "nemotron-3-ultra",
        "nvidia/nemotron-3-ultra",
        "nvidia-nemotron-3-ultra",
    ] {
        assert_eq!(
            normalize_model_name_for_provider(ApiProvider::Openrouter, alias).as_deref(),
            Some(OPENROUTER_NEMOTRON_3_ULTRA_MODEL)
        );
    }
}

#[test]
fn provider_capability_arcee_direct_models_use_api_docs_shape() {
    let thinking_cap = provider_capability(ApiProvider::Arcee, DEFAULT_ARCEE_MODEL);
    assert_eq!(thinking_cap.context_window, 262_144);
    assert_eq!(thinking_cap.max_output, Some(262_144));
    assert!(thinking_cap.thinking_supported);
    assert!(!thinking_cap.cache_telemetry_supported);
    assert_eq!(
        thinking_cap.request_payload_mode,
        RequestPayloadMode::ChatCompletions
    );

    let preview = provider_capability(ApiProvider::Arcee, ARCEE_TRINITY_LARGE_PREVIEW_MODEL);
    assert_eq!(preview.context_window, 262_144);
    assert_eq!(preview.max_output, None);
    assert!(!preview.thinking_supported);

    let mini = provider_capability(ApiProvider::Arcee, ARCEE_TRINITY_MINI_MODEL);
    assert_eq!(mini.context_window, 128_000);
    // Trinity Mini's upstream output limit is unknown, and ProviderCapability
    // now says so instead of fabricating a 4K request fallback.
    assert_eq!(mini.max_output, None);
    assert_eq!(
        crate::models::max_output_tokens_for_model(ARCEE_TRINITY_MINI_MODEL),
        None
    );
    assert!(mini.thinking_supported);
    assert!(!mini.cache_telemetry_supported);
    assert_eq!(
        mini.request_payload_mode,
        RequestPayloadMode::ChatCompletions
    );
}

#[test]
fn provider_capability_scenario_3() {
    // Scenario consolidation of: provider_capability_openai_custom_model_is_chat_completions_without_thinking, provider_capability_atlascloud_v4_model_resolves_model_metadata, provider_capability_moonshot_default_model_resolves_kimi_metadata, provider_capability_minimax_anthropic_uses_messages_shape, provider_capability_wanjie_ark_reasoner_has_thinking_no_cache, provider_capability_mistral_matches_reasoning_model_contract, provider_capability_ollama_deepseek_tag_uses_deepseek_heuristic, provider_capability_ollama_unknown_model_falls_back_to_8192
    // from provider_capability_openai_custom_model_is_chat_completions_without_thinking
    {
        let cap = provider_capability(ApiProvider::Openai, "glm-5");
        assert_eq!(
            cap.context_window,
            crate::models::LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, None);
        assert!(!cap.thinking_supported);
        assert!(!cap.cache_telemetry_supported);
        assert_eq!(
            cap.request_payload_mode,
            RequestPayloadMode::ChatCompletions
        );
    }
    // from provider_capability_atlascloud_v4_model_resolves_model_metadata
    {
        // #3023: Atlascloud uses the generic model-based path, so its default
        // DeepSeek V4 model resolves the real V4 metadata instead of the old
        // hardcoded legacy floor.
        let cap = provider_capability(ApiProvider::Atlascloud, "deepseek-ai/deepseek-v4-flash");
        assert_eq!(
            cap.context_window,
            crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, Some(384_000));
        assert!(cap.thinking_supported);
        assert!(!cap.cache_telemetry_supported);
        assert_eq!(
            cap.request_payload_mode,
            RequestPayloadMode::ChatCompletions
        );
    }
    // from provider_capability_moonshot_default_model_resolves_kimi_metadata
    {
        let cap = provider_capability(ApiProvider::Moonshot, DEFAULT_MOONSHOT_MODEL);
        assert_eq!(cap.context_window, 262_144);
        assert_eq!(cap.max_output, Some(32_768));
        assert!(cap.thinking_supported);
        assert!(!cap.cache_telemetry_supported);
        assert_eq!(
            cap.request_payload_mode,
            RequestPayloadMode::ChatCompletions
        );
    }
    // from provider_capability_minimax_anthropic_uses_messages_shape
    {
        for model in [DEFAULT_MINIMAX_MODEL, MINIMAX_M2_7_MODEL] {
            let cap = provider_capability(ApiProvider::MinimaxAnthropic, model);
            assert!(cap.thinking_supported, "{model}");
            assert!(!cap.cache_telemetry_supported, "{model}");
            assert_eq!(
                cap.request_payload_mode,
                RequestPayloadMode::AnthropicMessages
            );
        }
    }
    // from provider_capability_wanjie_ark_reasoner_has_thinking_no_cache
    {
        let cap = provider_capability(ApiProvider::WanjieArk, DEFAULT_WANJIE_ARK_MODEL);
        assert_eq!(
            cap.context_window,
            crate::models::LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, None);
        assert!(cap.thinking_supported);
        assert!(!cap.cache_telemetry_supported);
        assert_eq!(
            cap.request_payload_mode,
            RequestPayloadMode::ChatCompletions
        );
    }
    // from provider_capability_mistral_matches_reasoning_model_contract
    {
        for model in ["mistral-medium-latest", "mistral-small-latest"] {
            let cap = provider_capability(ApiProvider::Mistral, model);
            assert_eq!(cap.context_window, 262_144, "{model}");
            assert!(cap.thinking_supported, "{model}");
            assert_eq!(
                cap.request_payload_mode,
                RequestPayloadMode::ChatCompletions
            );
        }
        for model in ["mistral-code-latest", "mistral-large-latest"] {
            let cap = provider_capability(ApiProvider::Mistral, model);
            assert!(!cap.thinking_supported, "{model}");
        }
    }
    // from provider_capability_ollama_deepseek_tag_uses_deepseek_heuristic
    {
        // #3023: known model families resolve through models.rs lookups even
        // on Ollama — a legacy DeepSeek tag gets the 128K heuristic window.
        let cap = provider_capability(ApiProvider::Ollama, "deepseek-v3.1:671b");
        assert_eq!(
            cap.context_window,
            crate::models::LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, None);
        assert!(!cap.thinking_supported);
        assert!(!cap.cache_telemetry_supported);
        assert_eq!(
            cap.request_payload_mode,
            RequestPayloadMode::ChatCompletions
        );
    }
    // from provider_capability_ollama_unknown_model_falls_back_to_8192
    {
        let cap = provider_capability(ApiProvider::Ollama, "llama3.2:3b");
        assert_eq!(cap.context_window, 8192);
        assert_eq!(cap.max_output, None);
        assert!(!cap.thinking_supported);
        assert!(!cap.cache_telemetry_supported);
        assert_eq!(
            cap.request_payload_mode,
            RequestPayloadMode::ChatCompletions
        );
    }
}

#[test]
fn provider_capability_kimi_membership_ids_report_unknown_output_ceiling() {
    // The `kimi-for-coding` family is membership-only: the membership catalog
    // owns its output limits, so the static matrix must say "unknown" rather
    // than fabricating a ceiling. A placeholder here is not cosmetic — it
    // becomes a hard request clamp in `route_budget`.
    for model in ["kimi-for-coding", "kimi-for-coding-highspeed"] {
        let cap = provider_capability(ApiProvider::Moonshot, model);
        assert_eq!(cap.context_window, 262_144, "{model}");
        assert_eq!(cap.max_output, None, "{model}");
        assert!(cap.thinking_supported, "{model}");

        // Unknown is *omitted* on the wire, never serialized as a number.
        let json = serde_json::to_value(&cap).expect("capability serializes");
        assert!(
            json.get("max_output").is_none(),
            "{model}: unknown output ceiling must not be serialized: {json}"
        );
        let round_tripped: ProviderCapability =
            serde_json::from_value(json).expect("capability round-trips with an absent max_output");
        assert_eq!(round_tripped, cap, "{model}");
    }

    // The direct-platform K2.7 Code route does publish 32K, and keeps it.
    assert_eq!(
        provider_capability(ApiProvider::Moonshot, "kimi-k2.7-code").max_output,
        Some(32_768)
    );
}

#[test]
fn provider_capability_zai_defaults_to_5_3_and_tracks_5_2_5_1_and_turbo() {
    // GLM-5.3 is now the default direct Z.AI model; its limits inherit from
    // GLM-5.2 (1M context window) until Z.ai publishes distinct 5.3 numbers.
    let default = provider_capability(ApiProvider::Zai, DEFAULT_ZAI_MODEL);
    assert_eq!(default.resolved_model, DEFAULT_ZAI_MODEL);
    assert_eq!(default.resolved_model, ZAI_GLM_5_3_MODEL);
    assert_eq!(default.context_window, 1_000_000);
    assert_eq!(default.max_output, Some(131_072));
    assert!(default.thinking_supported);
    assert!(!default.cache_telemetry_supported);

    // GLM-5.2 remains available as an explicit model with its own id.
    let v52 = provider_capability(ApiProvider::Zai, ZAI_GLM_5_2_MODEL);
    assert_eq!(v52.resolved_model, ZAI_GLM_5_2_MODEL);
    assert_eq!(v52.context_window, 1_000_000);
    assert_eq!(v52.max_output, Some(131_072));
    assert!(v52.thinking_supported);

    // GLM-5.1 remains available as an explicit model (smaller window).
    let v51 = provider_capability(ApiProvider::Zai, ZAI_GLM_5_1_MODEL);
    assert_eq!(v51.resolved_model, ZAI_GLM_5_1_MODEL);
    assert_eq!(v51.context_window, 202_752);
    assert_eq!(v51.max_output, Some(131_072));
    assert!(v51.thinking_supported);

    // GLM-5.3-Flash is the published 1M multimodal sibling.
    let flash = provider_capability(ApiProvider::Zai, ZAI_GLM_5_3_FLASH_MODEL);
    assert_eq!(flash.resolved_model, ZAI_GLM_5_3_FLASH_MODEL);
    assert_eq!(flash.context_window, 1_000_000);
    assert_eq!(flash.max_output, Some(131_072));
    assert!(flash.thinking_supported);

    // GLM-5-Turbo is the faster sub-agent sibling of GLM-5.2.
    let turbo = provider_capability(ApiProvider::Zai, ZAI_GLM_5_TURBO_MODEL);
    assert_eq!(turbo.resolved_model, ZAI_GLM_5_TURBO_MODEL);
}

#[test]
fn provider_capability_minimax_direct_models_use_api_docs_shape() {
    let m3 = provider_capability(ApiProvider::Minimax, DEFAULT_MINIMAX_MODEL);
    assert_eq!(m3.context_window, 1_000_000);
    assert_eq!(m3.max_output, Some(524_288));
    assert!(m3.thinking_supported);
    assert!(!m3.cache_telemetry_supported);
    assert_eq!(m3.request_payload_mode, RequestPayloadMode::ChatCompletions);

    for model in [
        MINIMAX_M2_7_MODEL,
        MINIMAX_M2_7_HIGHSPEED_MODEL,
        MINIMAX_M2_5_MODEL,
        MINIMAX_M2_5_HIGHSPEED_MODEL,
        MINIMAX_M2_1_MODEL,
        MINIMAX_M2_1_HIGHSPEED_MODEL,
        MINIMAX_M2_MODEL,
    ] {
        let cap = provider_capability(ApiProvider::Minimax, model);
        assert_eq!(cap.context_window, 204_800, "{model}");
        assert!(cap.thinking_supported, "{model}");
        assert!(!cap.cache_telemetry_supported, "{model}");
        assert_eq!(
            cap.request_payload_mode,
            RequestPayloadMode::ChatCompletions
        );
    }
}

#[test]
fn provider_capability_scenario_4() {
    // Scenario consolidation of: provider_capability_non_v4_model_has_smaller_window, provider_capability_roundtrip_serialization
    // from provider_capability_non_v4_model_has_smaller_window
    {
        let cap = provider_capability(ApiProvider::Deepseek, "deepseek-coder");
        assert_eq!(
            cap.context_window,
            crate::models::LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS
        );
        assert_eq!(cap.max_output, None);
        assert!(!cap.thinking_supported);
    }
    // from provider_capability_roundtrip_serialization
    {
        let cap = provider_capability(ApiProvider::Deepseek, "deepseek-v4-pro");
        let json = serde_json::to_value(&cap).unwrap();
        let deserialized: ProviderCapability = serde_json::from_value(json).unwrap();
        assert_eq!(cap, deserialized);
    }
}

#[test]
fn status_item_balance_available_for_prepaid_providers() {
    assert!(StatusItem::Balance.is_available_for(ApiProvider::Deepseek));
    assert!(StatusItem::Balance.is_available_for(ApiProvider::DeepseekCN));
    assert!(StatusItem::Balance.is_available_for(ApiProvider::Openrouter));
    assert!(StatusItem::Balance.is_available_for(ApiProvider::Siliconflow));
    assert!(StatusItem::Balance.is_available_for(ApiProvider::SiliconflowCn));
    // Invoice-only, local, and unimplemented prepaid vendors stay hidden.
    assert!(!StatusItem::Balance.is_available_for(ApiProvider::Novita));
    assert!(!StatusItem::Balance.is_available_for(ApiProvider::NvidiaNim));
    assert!(!StatusItem::Balance.is_available_for(ApiProvider::Fireworks));
    assert!(!StatusItem::Balance.is_available_for(ApiProvider::Sglang));
    assert!(!StatusItem::Balance.is_available_for(ApiProvider::Vllm));
    assert!(!StatusItem::Balance.is_available_for(ApiProvider::Ollama));
    assert!(!StatusItem::Balance.is_available_for(ApiProvider::Openai));
    assert!(!StatusItem::Balance.is_available_for(ApiProvider::Atlascloud));
    // Other StatusItem variants should be available everywhere.
    assert!(StatusItem::Mode.is_available_for(ApiProvider::Ollama));
}

#[test]
fn status_items_scenario() {
    // Scenario consolidation of: status_items_deser_ignores_unknown_variants, status_items_deser_allows_missing_field
    // from status_items_deser_ignores_unknown_variants
    {
        // Simulate a stable build reading config written by a dev build that
        // knows about items the stable build doesn't (e.g. "balance" or a
        // future "cost_saving" chip).
        let toml_str = r#"
            alternate_screen = "auto"
            status_items = ["mode", "model", "unknown_future_item", "cost", "another_unknown", "status"]
        "#;
        let tui: TuiConfig = toml::from_str(toml_str).expect("should parse without error");
        let items = tui.status_items.expect("status_items should be Some");
        assert_eq!(items.len(), 4, "unknown items should be silently dropped");
        assert_eq!(items[0], StatusItem::Mode);
        assert_eq!(items[1], StatusItem::Model);
        assert_eq!(items[2], StatusItem::Cost);
        assert_eq!(items[3], StatusItem::Status);
    }
    // from status_items_deser_allows_missing_field
    {
        let toml_str = r#"
            locale = "zh-Hans"
            mouse_capture = false
        "#;
        let tui: TuiConfig = toml::from_str(toml_str).expect("missing status_items should parse");
        assert_eq!(tui.status_items, None);
    }
}

#[test]
fn transcript_prose_scenario() -> Result<()> {
    // Scenario consolidation of: transcript_prose_measure_loads_and_resolves, transcript_prose_measure_rejects_negative_with_clear_error, transcript_prose_measure_rejects_non_integers_with_clear_error
    // from transcript_prose_measure_loads_and_resolves
    {
        // #5436: absent = full width; 0 also means full width; a positive
        // integer caps prose wrap at that many columns.
        let absent: Config = toml::from_str("provider = \"openai\"\n")?;
        absent.validate()?;
        assert_eq!(absent.prose_measure(), None);

        let zero: Config = toml::from_str(
            "
    [transcript]
    prose_measure = 0
    ",
        )?;
        zero.validate()?;
        assert_eq!(zero.prose_measure(), None, "0 must mean full width");

        let capped: Config = toml::from_str(
            "
    [transcript]
    prose_measure = 120
    ",
        )?;
        capped.validate()?;
        assert_eq!(capped.prose_measure(), Some(120));
    }
    // from transcript_prose_measure_rejects_negative_with_clear_error
    {
        let config: Config = toml::from_str(
            "
    [transcript]
    prose_measure = -5
    ",
        )
        .expect("negative integers must parse so validate can name the key");

        let error = config
            .validate()
            .expect_err("negative prose_measure should be rejected");
        let message = error.to_string();
        assert!(
            message.contains("transcript.prose_measure"),
            "error should name the key: {message}"
        );
        assert!(
            message.contains("-5"),
            "error should echo the value: {message}"
        );
        assert!(
            message.contains("positive whole number"),
            "error should say what is expected: {message}"
        );
    }
    // from transcript_prose_measure_rejects_non_integers_with_clear_error
    {
        for raw in ["\"fill\"", "12.5", "true"] {
            let config: Config = toml::from_str(&format!(
                "
    [transcript]
    prose_measure = {raw}
    "
            ))
            .unwrap_or_else(|_| panic!("{raw} must parse so validate can name the key"));

            let error = config
                .validate()
                .expect_err(&format!("{raw} should be rejected"));
            let message = error.to_string();
            assert!(
                message.contains("transcript.prose_measure"),
                "error should name the key for {raw}: {message}"
            );
            assert!(
                message.contains("positive whole number"),
                "error should say what is expected for {raw}: {message}"
            );
        }
    }
    Ok(())
}

#[test]
fn huggingface_provider_scenario() -> Result<()> {
    // Scenario consolidation of: huggingface_provider_aliases_parse, huggingface_provider_uses_direct_defaults
    // from huggingface_provider_aliases_parse
    {
        for alias in ["huggingface", "hugging-face", "hugging_face", "hf"] {
            assert_eq!(ApiProvider::parse(alias), Some(ApiProvider::Huggingface));
        }
    }
    // from huggingface_provider_uses_direct_defaults
    {
        let _lock = lock_test_env();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp_root = env::temp_dir().join(format!(
            "codewhale-tui-huggingface-defaults-test-{}-{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&temp_root)?;
        let _guard = EnvGuard::new(&temp_root);

        unsafe {
            env::set_var("CODEWHALE_PROVIDER", "huggingface");
            env::set_var("HUGGINGFACE_API_KEY", "hf-env-key");
        }

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Huggingface);
        assert_eq!(config.deepseek_api_key()?, "hf-env-key");
        assert_eq!(config.deepseek_base_url(), DEFAULT_HUGGINGFACE_BASE_URL);
        assert_eq!(config.default_model(), DEFAULT_HUGGINGFACE_MODEL);
    }
    Ok(())
}

#[test]
fn huggingface_hf_token_env_api_key_resolves() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-huggingface-hf-token-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    unsafe {
        env::set_var("CODEWHALE_PROVIDER", "huggingface");
        env::set_var("HF_TOKEN", "hf-token-value");
    }

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Huggingface);
    assert_eq!(config.deepseek_api_key()?, "hf-token-value");
    Ok(())
}

#[test]
fn huggingface_missing_key_error_mentions_env_fallbacks() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-huggingface-missing-key-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let config = Config {
        provider: Some("huggingface".to_string()),
        ..Default::default()
    };

    config.validate()?;
    let err = config.deepseek_api_key().expect_err("missing key");
    let message = err.to_string();
    assert!(message.contains("Hugging Face API key not found"));
    assert!(message.contains("https://huggingface.co/settings/tokens"));
    assert!(message.contains("HUGGINGFACE_API_KEY"));
    assert!(message.contains("HF_TOKEN"));
    Ok(())
}

#[test]
fn huggingface_custom_env_urls_do_not_inherit_ambient_keys() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-huggingface-env-test-{}-{}",
        std::process::id(),
        nanos
    ));

    {
        let long_form_root = temp_root.join("long-form");
        fs::create_dir_all(&long_form_root)?;
        let _guard = EnvGuard::new(&long_form_root);

        unsafe {
            env::set_var("CODEWHALE_PROVIDER", "huggingface");
            env::set_var("HUGGINGFACE_API_KEY", "hf-env-key");
            env::set_var("HF_TOKEN", "hf-token-fallback");
            env::set_var("HUGGINGFACE_BASE_URL", "https://custom-hf.example/v1");
            env::set_var("HF_BASE_URL", "https://fallback-hf.example/v1");
            env::set_var("HUGGINGFACE_MODEL", "meta-llama/Llama-3-70B");
            env::set_var("HF_MODEL", "fallback/model");
        }

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Huggingface);
        let error = config
            .deepseek_api_key()
            .expect_err("ambient key must not follow a custom endpoint");
        assert!(error.to_string().contains("must be bound explicitly"));
        assert!(!has_api_key(&config));
        assert_eq!(config.deepseek_base_url(), "https://custom-hf.example/v1");
        assert_eq!(config.default_model(), "meta-llama/Llama-3-70B");
    }

    {
        let short_form_root = temp_root.join("short-form");
        fs::create_dir_all(&short_form_root)?;
        let _guard = EnvGuard::new(&short_form_root);

        unsafe {
            env::set_var("CODEWHALE_PROVIDER", "huggingface");
            env::set_var("HF_TOKEN", "hf-env-key");
            env::set_var("HF_BASE_URL", "https://custom-hf.example/v1");
            env::set_var("HF_MODEL", "meta-llama/Llama-3-70B");
        }

        let config = Config::load(None, None)?;
        assert_eq!(config.api_provider(), ApiProvider::Huggingface);
        let error = config
            .deepseek_api_key()
            .expect_err("ambient key must not follow a custom endpoint");
        assert!(error.to_string().contains("must be bound explicitly"));
        assert!(!has_api_key(&config));
        assert_eq!(config.deepseek_base_url(), "https://custom-hf.example/v1");
        assert_eq!(config.default_model(), "meta-llama/Llama-3-70B");
    }
    Ok(())
}

#[test]
fn notifications_parse_scenario() {
    // Scenario consolidation of: notifications_parse_custom_completion_sound_file, notifications_parse_quiet_and_event_categories
    // from notifications_parse_custom_completion_sound_file
    {
        let config: Config = toml::from_str(
            r#"
            [notifications]
            completion_sound = "file"
            sound_file = "E:\\google\\downloads\\xm4114.wav"
            "#,
        )
        .expect("custom completion sound config should parse");

        let notifications = config.notifications_config();
        assert_eq!(notifications.completion_sound, CompletionSound::File);
        assert_eq!(
            notifications.sound_file.as_deref(),
            Some(std::path::Path::new("E:\\google\\downloads\\xm4114.wav"))
        );
    }
    // from notifications_parse_quiet_and_event_categories
    {
        let config: Config = toml::from_str(
            r#"
            [notifications]
            quiet = true

            [notifications.events]
            approval-needed = false
            model-notify = false
            "#,
        )
        .expect("quiet + events config should parse");

        let notifications = config.notifications_config();
        assert!(notifications.quiet);
        let events = notifications.events;
        assert!(!events.approval_needed);
        assert!(!events.model_notify);
        // Unlisted categories keep their enabled default.
        assert!(events.turn_complete);
        assert!(events.subagent_terminal);
        assert!(events.input_needed);
        assert!(events.elevation_needed);
    }
}

#[test]
fn notifications_parse_event_sound_table() {
    let config: Config = toml::from_str(
        r#"
        [notifications.event_sound]
        enabled = true
        events = ["turn-complete", "bogus-event", "approval-needed"]
        min_interval_ms = 500
        quiet = true
        "#,
    )
    .expect("event sound config should parse");

    let notifications = config.notifications_config();
    assert_eq!(
        notifications.event_sound,
        EventSoundConfig {
            enabled: true,
            events: vec![
                "turn-complete".to_string(),
                "bogus-event".to_string(),
                "approval-needed".to_string(),
            ],
            min_interval_ms: 500,
            quiet: true,
        }
    );
}

#[test]
fn notifications_event_sound_defaults_when_table_absent() {
    let config: Config = toml::from_str("[notifications]\nmethod = \"off\"\n")
        .expect("bare notifications table should parse");

    let event_sound = config.notifications_config().event_sound;
    assert_eq!(event_sound, EventSoundConfig::default());
    assert!(!event_sound.enabled);
    assert_eq!(
        event_sound.events,
        vec!["turn-complete".to_string(), "approval-needed".to_string()]
    );
    assert_eq!(event_sound.min_interval_ms, 2000);
    assert!(!event_sound.quiet);
}

#[test]
fn notifications_quiet_and_events_default_off_and_all_enabled() {
    let config: Config = toml::from_str("[notifications]\nmethod = \"auto\"\n")
        .expect("bare notifications table should parse");

    let notifications = config.notifications_config();
    assert!(!notifications.quiet);
    assert_eq!(notifications.events, NotificationEventsConfig::default());
    assert!(notifications.events.turn_complete);
    assert!(notifications.events.model_notify);
}

#[test]
fn huggingface_short_custom_env_url_does_not_inherit_ambient_key() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-huggingface-short-env-test-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    unsafe {
        env::set_var("CODEWHALE_PROVIDER", "hf");
        env::set_var("HF_TOKEN", "hf-token-value");
        env::set_var("HF_BASE_URL", "https://short-hf.example/v1");
        env::set_var("HF_MODEL", "org/short-model");
    }

    let config = Config::load(None, None)?;
    assert_eq!(config.api_provider(), ApiProvider::Huggingface);
    let error = config
        .deepseek_api_key()
        .expect_err("ambient key must not follow a custom endpoint");
    assert!(error.to_string().contains("must be bound explicitly"));
    assert!(!has_api_key(&config));
    assert_eq!(config.deepseek_base_url(), "https://short-hf.example/v1");
    assert_eq!(config.default_model(), "org/short-model");
    Ok(())
}

// === #1519 custom OpenAI-compatible provider slice ===

#[test]
fn custom_provider_flatten_map_parses_alongside_named_provider() {
    // A custom `[providers.my_thing]` table lands in the flatten map while a
    // built-in `[providers.openai]` table still binds its named field.
    let config: Config = toml::from_str(
        r#"
provider = "my_thing"

[providers.openai]
api_key = "openai-key"

[providers.my_thing]
kind = "openai-compatible"
base_url = "https://api.example.com/v1"
model = "custom-model-v1"
api_key_env = "EXAMPLE_API_KEY"
"#,
    )
    .expect("config with a custom provider table should parse");

    let providers = config.providers.as_ref().expect("providers table present");
    // Built-in named field still works.
    assert_eq!(providers.openai.api_key.as_deref(), Some("openai-key"));
    // The custom entry is captured by name in the flatten map.
    let custom = providers
        .custom_provider_config("my_thing")
        .expect("custom entry parsed into flatten map");
    assert_eq!(custom.kind.as_deref(), Some("openai-compatible"));
    assert_eq!(
        custom.base_url.as_deref(),
        Some("https://api.example.com/v1")
    );
    assert_eq!(custom.model.as_deref(), Some("custom-model-v1"));
    assert_eq!(custom.api_key_env.as_deref(), Some("EXAMPLE_API_KEY"));
    assert!(custom.is_openai_compatible_custom());
    // A built-in provider name never leaks into the custom map.
    assert!(providers.custom_provider_config("openai").is_none());
}

#[test]
fn api_provider_returns_custom_for_custom_name_and_deepseek_for_junk() {
    // Names a real custom table → Custom (the #1519 silent-misroute fix).
    let mut custom = HashMap::new();
    custom.insert(
        "my_thing".to_string(),
        ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("https://api.example.com/v1".to_string()),
            ..Default::default()
        },
    );
    let config = Config {
        provider: Some("my_thing".to_string()),
        providers: Some(ProvidersConfig {
            custom,
            ..Default::default()
        }),
        ..Config::default()
    };
    assert_eq!(config.api_provider(), ApiProvider::Custom);
    config
        .validate()
        .expect("named custom providers should pass config validation");

    // Genuine junk that matches no built-in provider AND no custom table →
    // falls back to DeepSeek, exactly as before this slice.
    let junk = Config {
        provider: Some("totally-not-a-provider".to_string()),
        ..Config::default()
    };
    assert_eq!(junk.api_provider(), ApiProvider::Deepseek);
    assert!(
        junk.validate().is_err(),
        "invalid provider names should still fail validation"
    );
}

#[test]
fn custom_provider_scenario() {
    // Scenario consolidation of: custom_provider_kind_only_accepts_openai_compatible, custom_provider_base_url_and_model_resolve_from_named_table
    // from custom_provider_kind_only_accepts_openai_compatible
    {
        let ok = ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            ..Default::default()
        };
        assert!(ok.is_openai_compatible_custom());

        // Underscore spelling and case are tolerated.
        let underscore = ProviderConfig {
            kind: Some("OpenAI_Compatible".to_string()),
            ..Default::default()
        };
        assert!(underscore.is_openai_compatible_custom());

        // Any other declared wire format is rejected (callers error on these).
        let other = ProviderConfig {
            kind: Some("anthropic-messages".to_string()),
            ..Default::default()
        };
        assert!(!other.is_openai_compatible_custom());

        // Built-in providers leave `kind` unset.
        assert!(!ProviderConfig::default().is_openai_compatible_custom());
    }
    // from custom_provider_base_url_and_model_resolve_from_named_table
    {
        let mut custom = HashMap::new();
        custom.insert(
            "my_thing".to_string(),
            ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some("https://api.example.com/v1".to_string()),
                model: Some("custom-model-v1".to_string()),
                ..Default::default()
            },
        );
        let config = Config {
            provider: Some("my_thing".to_string()),
            providers: Some(ProvidersConfig {
                custom,
                ..Default::default()
            }),
            ..Config::default()
        };

        // Resolution reads the named table, not a DeepSeek default.
        assert_eq!(config.api_provider(), ApiProvider::Custom);
        assert_eq!(config.deepseek_base_url(), "https://api.example.com/v1");
        assert_eq!(config.default_model(), "custom-model-v1");
    }
}

fn session_custom_provider_config(name: &str, kind: &str, base_url: &str) -> Config {
    let mut custom = HashMap::new();
    custom.insert(
        name.to_string(),
        ProviderConfig {
            kind: Some(kind.to_string()),
            base_url: Some(base_url.to_string()),
            model: Some("local-model".to_string()),
            ..Default::default()
        },
    );
    Config {
        provider: Some(name.to_string()),
        providers: Some(ProvidersConfig {
            custom,
            ..Default::default()
        }),
        ..Config::default()
    }
}

#[test]
fn session_provider_identity_preserves_exact_named_custom_key() {
    let config = session_custom_provider_config(
        "lm-studio",
        "openai-compatible",
        "http://127.0.0.1:1234/v1",
    );

    assert_eq!(
        config.provider_identity_for(ApiProvider::Custom),
        "lm-studio"
    );
    assert_eq!(
        config
            .resolve_provider_identity("lm-studio")
            .expect("exact custom identity"),
        ProviderIdentity {
            provider: ApiProvider::Custom,
            key: "lm-studio".to_string(),
            exact_id: Some("lm-studio".to_string()),
            migrated_legacy_ollama_cloud_route: false,
        }
    );
    assert_eq!(
        config
            .resolve_provider_identity("openrouter")
            .expect("built-in identity"),
        ProviderIdentity {
            provider: ApiProvider::Openrouter,
            key: "openrouter".to_string(),
            exact_id: Some("openrouter".to_string()),
            migrated_legacy_ollama_cloud_route: false,
        }
    );
    let migrated = config
        .resolve_provider_identity("custom")
        .expect("released generic custom record migrates to sole live named route");
    assert_eq!(
        migrated,
        ProviderIdentity {
            provider: ApiProvider::Custom,
            key: "lm-studio".to_string(),
            exact_id: Some("lm-studio".to_string()),
            migrated_legacy_ollama_cloud_route: false,
        }
    );
}

#[test]
fn persisted_legacy_ollama_cloud_receipts_upgrade_only_on_exact_live_route() {
    let exact = Config {
        provider: Some("ollama".to_string()),
        providers: Some(ProvidersConfig {
            ollama: ProviderConfig {
                base_url: Some(codewhale_config::provider::OLLAMA_CLOUD_BASE_URL.to_string()),
                ..ProviderConfig::default()
            },
            ..ProvidersConfig::default()
        }),
        ..Config::default()
    };
    for provider_id in [None, Some("ollama")] {
        let identity = exact
            .resolve_persisted_provider_identity(Some("ollama"), provider_id)
            .expect("exact released tuple migrates");
        assert_eq!(identity.provider, ApiProvider::OllamaCloud);
        assert_eq!(identity.key, "ollama-cloud");
        assert_eq!(identity.exact_id.as_deref(), Some("ollama"));
        assert!(identity.migrated_legacy_ollama_cloud_route);
    }

    let neighbor = Config {
        provider: Some("ollama".to_string()),
        providers: Some(ProvidersConfig {
            ollama: ProviderConfig {
                base_url: Some("https://ollama.com/v1/preview".to_string()),
                ..ProviderConfig::default()
            },
            ..ProvidersConfig::default()
        }),
        ..Config::default()
    };
    let identity = neighbor
        .resolve_persisted_provider_identity(Some("ollama"), Some("ollama"))
        .expect("neighbor remains local/custom ollama identity");
    assert_eq!(identity.provider, ApiProvider::Ollama);
    assert_eq!(identity.key, "ollama");

    let explicit = Config {
        provider: Some("ollama-cloud".to_string()),
        ..Config::default()
    };
    let identity = explicit
        .resolve_persisted_provider_identity(Some("ollama-cloud"), Some("ollama-cloud"))
        .expect("new receipt remains first-class cloud identity");
    assert_eq!(identity.provider, ApiProvider::OllamaCloud);
    assert_eq!(identity.key, "ollama-cloud");
    assert_eq!(identity.exact_id.as_deref(), Some("ollama-cloud"));
    assert!(!identity.migrated_legacy_ollama_cloud_route);

    let coexisting = Config {
        provider: Some("ollama".to_string()),
        providers: Some(ProvidersConfig {
            ollama: ProviderConfig {
                base_url: Some(codewhale_config::provider::OLLAMA_CLOUD_BASE_URL.to_string()),
                ..ProviderConfig::default()
            },
            ollama_cloud: ProviderConfig {
                base_url: Some(codewhale_config::provider::OLLAMA_CLOUD_BASE_URL.to_string()),
                ..ProviderConfig::default()
            },
            ..ProvidersConfig::default()
        }),
        ..Config::default()
    };
    let explicit = coexisting
        .resolve_persisted_provider_identity(Some("ollama-cloud"), Some("ollama-cloud"))
        .expect("explicit receipt stays on the first-class route");
    assert!(!explicit.migrated_legacy_ollama_cloud_route);
    assert_eq!(explicit.exact_id.as_deref(), Some("ollama-cloud"));

    let mut explicit_live = coexisting.clone();
    explicit_live.provider = Some("ollama-cloud".to_string());
    let migrated = explicit_live
        .resolve_persisted_provider_identity(Some("ollama-cloud"), Some("ollama"))
        .expect("legacy receipt retains its source route after a live provider switch");
    assert!(migrated.migrated_legacy_ollama_cloud_route);
    assert_eq!(migrated.exact_id.as_deref(), Some("ollama"));
}

#[test]
fn literal_custom_table_round_trips_as_exact_historical_route() {
    let config =
        session_custom_provider_config("custom", "openai-compatible", "http://127.0.0.1:1234/v1");

    assert_eq!(config.api_provider(), ApiProvider::Custom);
    assert!(!config.uses_legacy_literal_custom_route());
    let identity = config
        .resolve_provider_identity("custom")
        .expect("exact [providers.custom] identity");
    assert_eq!(identity.key, "custom");
    let route = crate::route_runtime::resolve_runtime_route(
        &config,
        identity.provider,
        Some("local-model"),
    )
    .expect("resolve exact literal table")
    .validate()
    .expect("preflight exact literal table");
    assert_eq!(route.identity.key, "custom");
    assert_eq!(route.client.base_url(), "http://127.0.0.1:1234/v1");
    assert_eq!(
        route
            .config
            .resolve_provider_identity(&route.identity.key)
            .expect("repeat exact literal table resolution"),
        identity
    );
}

#[test]
fn persisted_custom_fields_distinguish_legacy_root_from_exact_literal_table() {
    let table_only =
        session_custom_provider_config("custom", "openai-compatible", "http://127.0.0.1:1234/v1");
    let table_only_error = table_only
        .resolve_persisted_provider_identity(Some("custom"), None)
        .expect_err("id-less custom records authorize only the legacy root route");
    assert!(
        table_only_error.contains("root-level"),
        "{table_only_error}"
    );
    assert!(table_only_error.contains("fall back"), "{table_only_error}");

    let mut coexist = table_only.clone();
    coexist.base_url = Some("http://127.0.0.1:18180/v1".to_string());
    coexist.default_text_model = Some("legacy-root-model".to_string());
    let root = coexist
        .resolve_persisted_provider_identity(Some("custom"), None)
        .expect("id-less record remains bound to the root route");
    assert_eq!(root.provider, ApiProvider::Custom);
    assert_eq!(root.key, "custom");
    assert_eq!(root.exact_id, None);
    let root_route = crate::route_runtime::resolve_runtime_route_for_identity(
        &coexist,
        &root,
        Some("legacy-root-model"),
    )
    .expect("scope root identity")
    .validate()
    .expect("validate root identity");
    assert_eq!(root_route.client.base_url(), "http://127.0.0.1:18180/v1");
    assert_eq!(root_route.identity.exact_id, None);

    let exact_table = coexist
        .resolve_persisted_provider_identity(Some("custom"), Some("custom"))
        .expect("additive exact id intentionally selects the table");
    assert_eq!(exact_table.provider, ApiProvider::Custom);
    assert_eq!(exact_table.key, "custom");
    assert_eq!(exact_table.exact_id.as_deref(), Some("custom"));

    let root_only = Config {
        provider: Some("custom".to_string()),
        base_url: Some("http://127.0.0.1:18180/v1".to_string()),
        default_text_model: Some("legacy-root-model".to_string()),
        ..Config::default()
    };
    let exact_error = root_only
        .resolve_persisted_provider_identity(Some("custom"), Some("custom"))
        .expect_err("exact table record cannot fall back to a legacy root route");
    assert!(exact_error.contains("[providers.custom]"), "{exact_error}");
    assert!(exact_error.contains("will not fall back"), "{exact_error}");
    let exact_route_error = crate::route_runtime::resolve_runtime_route_for_identity(
        &root_only,
        &exact_table,
        Some("table-model"),
    )
    .expect_err("runtime route must revalidate exact table provenance");
    assert!(
        exact_route_error.contains("[providers.custom]"),
        "{exact_route_error}"
    );
}

#[test]
fn persisted_empty_custom_id_never_falls_back_to_legacy_root() {
    let mut config =
        session_custom_provider_config("custom", "openai-compatible", "http://127.0.0.1:18181/v1");
    config.base_url = Some("http://127.0.0.1:18180/v1".to_string());
    config.default_text_model = Some("legacy-root-model".to_string());

    for malformed_id in ["", "   "] {
        let error = config
            .resolve_persisted_provider_identity(Some("custom"), Some(malformed_id))
            .expect_err("an explicit empty exact id must never authorize the root route");
        assert!(error.contains("empty exact provider id"), "{error}");
        assert!(error.contains("will not guess or fall back"), "{error}");
    }

    let root = config
        .resolve_persisted_provider_identity(Some("custom"), None)
        .expect("a genuinely missing id retains legacy root compatibility");
    assert_eq!(root.exact_id, None);
    let exact = config
        .resolve_persisted_provider_identity(Some("custom"), Some("custom"))
        .expect("a non-empty exact id selects the literal table");
    assert_eq!(exact.exact_id.as_deref(), Some("custom"));
}

#[test]
fn persisted_provider_pair_never_collapses_builtin_into_same_key_custom_route() {
    let config =
        session_custom_provider_config("openai", "openai-compatible", "http://127.0.0.1:1234/v1");
    assert_eq!(
        config
            .resolve_provider_identity("openai")
            .expect("raw exact identity intentionally prefers custom"),
        ProviderIdentity {
            provider: ApiProvider::Custom,
            key: "openai".to_string(),
            exact_id: Some("openai".to_string()),
            migrated_legacy_ollama_cloud_route: false,
        }
    );

    for provider_id in [None, Some("openai")] {
        let error = config
            .resolve_persisted_provider_identity(Some("openai"), provider_id)
            .expect_err("built-in record must not be captured by the custom table");
        assert!(error.contains("requires built-in 'openai'"), "{error}");
        assert!(error.contains("shadows"), "{error}");
        assert!(error.contains("will not guess or fall back"), "{error}");
    }

    let exact_custom = config
        .resolve_persisted_provider_identity(Some("custom"), Some("openai"))
        .expect("custom kind plus exact id intentionally selects the table");
    assert_eq!(exact_custom.provider, ApiProvider::Custom);
    assert_eq!(exact_custom.key, "openai");

    let mismatch = config
        .resolve_persisted_provider_identity(Some("openrouter"), Some("openai"))
        .expect_err("mismatched built-in kind/id pair must fail closed");
    assert!(mismatch.contains("mismatched fields"), "{mismatch}");
}

#[test]
fn case_colliding_custom_table_preserves_exact_spelling_across_receipts() {
    let config =
        session_custom_provider_config("CUSTOM", "openai-compatible", "http://127.0.0.1:5678/v1");

    assert_eq!(config.api_provider(), ApiProvider::Custom);
    assert_eq!(config.provider_identity_for(ApiProvider::Custom), "CUSTOM");
    let identity = config
        .resolve_provider_identity("CUSTOM")
        .expect("exact case-colliding custom identity");
    assert_eq!(identity.key, "CUSTOM");
    let route = crate::route_runtime::resolve_runtime_route(
        &config,
        identity.provider,
        Some("local-model"),
    )
    .expect("resolve case-colliding custom table")
    .validate()
    .expect("preflight case-colliding custom table");
    assert_eq!(route.identity.key, "CUSTOM");
    assert_eq!(route.client.base_url(), "http://127.0.0.1:5678/v1");
}

#[test]
fn legacy_literal_custom_identity_requires_one_valid_root_route() {
    let _lock = lock_test_env();
    let _source = EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
    let _cli_key = EnvVarGuard::remove("CODEWHALE_CLI_API_KEY");
    let legacy = Config {
        provider: Some("custom".to_string()),
        api_key: Some("legacy-root-key".to_string()),
        base_url: Some("http://127.0.0.1:1234/v1".to_string()),
        default_text_model: Some("local-legacy-model".to_string()),
        ..Config::default()
    };

    assert_eq!(
        legacy
            .resolve_provider_identity("custom")
            .expect("unchanged legacy root route"),
        ProviderIdentity {
            provider: ApiProvider::Custom,
            key: "custom".to_string(),
            exact_id: None,
            migrated_legacy_ollama_cloud_route: false,
        }
    );
    assert_eq!(legacy.deepseek_base_url(), "http://127.0.0.1:1234/v1");
    assert_eq!(legacy.default_model(), "local-legacy-model");
    assert_eq!(legacy.deepseek_api_key().unwrap(), "legacy-root-key");

    let mut named = session_custom_provider_config(
        "lm-studio",
        "openai-compatible",
        "https://api.example.com/v1",
    );
    named.api_key = Some("must-not-leak-to-named-route".to_string());
    let named_key_error = named
        .deepseek_api_key()
        .expect_err("root legacy key must never authorize a named custom route")
        .to_string();
    assert!(named_key_error.contains("lm-studio"), "{named_key_error}");
    assert!(!named_key_error.contains("must-not-leak"));

    let mut ambiguous_named = named.clone();
    ambiguous_named
        .providers
        .as_mut()
        .expect("providers")
        .custom
        .insert(
            "vllm-local".to_string(),
            ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some("http://127.0.0.1:8000/v1".to_string()),
                model: Some("other-local-model".to_string()),
                ..ProviderConfig::default()
            },
        );
    let ambiguous_named_error = ambiguous_named
        .resolve_provider_identity("custom")
        .expect_err("generic released record cannot choose between named routes");
    assert!(
        ambiguous_named_error.contains("valid named routes: 2"),
        "{ambiguous_named_error}"
    );
    assert!(ambiguous_named_error.contains("will not guess or fall back"));

    let mut missing_model = legacy.clone();
    missing_model.default_text_model = None;
    let model_error = missing_model
        .resolve_provider_identity("custom")
        .expect_err("legacy root route needs an explicit model");
    assert!(model_error.contains("default_text_model"), "{model_error}");

    let mut auto_model = legacy.clone();
    auto_model.default_text_model = Some("auto".to_string());
    let auto_error = auto_model
        .resolve_provider_identity("custom")
        .expect_err("legacy root route cannot guess an auto model");
    assert!(auto_error.contains("not `auto`"), "{auto_error}");

    let mut invalid_url = legacy.clone();
    invalid_url.base_url = Some("not a provider URL".to_string());
    let url_error = invalid_url
        .resolve_provider_identity("custom")
        .expect_err("legacy root route needs a valid endpoint");
    assert!(url_error.contains("base_url"), "{url_error}");
    assert!(url_error.contains("will not fall back"), "{url_error}");

    let mut ambiguous = legacy.clone();
    ambiguous.providers = Some(ProvidersConfig {
        custom: HashMap::from([(
            "CUSTOM".to_string(),
            ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some("http://127.0.0.1:5678/v1".to_string()),
                model: Some("table-model".to_string()),
                ..ProviderConfig::default()
            },
        )]),
        ..ProvidersConfig::default()
    });
    let ambiguous_error = ambiguous
        .resolve_provider_identity("custom")
        .expect_err("root and table routes cannot share the generic identity");
    assert!(
        ambiguous_error.contains("[providers.custom]") && ambiguous_error.contains("ambiguous"),
        "{ambiguous_error}"
    );

    let removed_named = legacy
        .resolve_provider_identity("lm-studio")
        .expect_err("a removed named route must not fall back to legacy custom");
    assert!(removed_named.contains("[providers.lm-studio]"));
    assert!(removed_named.contains("will not fall back"));
}

#[test]
fn legacy_literal_custom_env_overrides_preserve_root_route_shape() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-legacy-custom-env-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);
    let _source = EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
    let _cli_key = EnvVarGuard::remove("CODEWHALE_CLI_API_KEY");

    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        r#"provider = "custom"
api_key = "legacy-root-key"
base_url = "http://127.0.0.1:18184/v1"
default_text_model = "legacy-model"
"#,
    )?;
    // Safety: test-only env mutation guarded by lock_test_env().
    unsafe {
        env::set_var("CODEWHALE_BASE_URL", "http://127.0.0.1:18185/v1");
        env::set_var("CODEWHALE_MODEL", "env-legacy-model");
        env::set_var("DEEPSEEK_HTTP_HEADERS", "X-Legacy-Route=kept");
    }

    let config = Config::load(None, None)?;

    assert!(config.uses_legacy_literal_custom_route());
    assert!(
        config
            .providers
            .as_ref()
            .is_none_or(|providers| !providers.custom.contains_key("custom"))
    );
    assert_eq!(config.deepseek_base_url(), "http://127.0.0.1:18185/v1");
    assert_eq!(config.default_model(), "env-legacy-model");
    assert_eq!(
        config.deepseek_api_key()?,
        "",
        "an env-selected keyless loopback route must not inherit the file-owned root key"
    );
    assert!(!active_provider_has_config_api_key(&config));
    assert_eq!(
        config
            .http_headers()
            .get("X-Legacy-Route")
            .map(String::as_str),
        Some("kept")
    );
    for _ in 0..2 {
        assert_eq!(
            config
                .resolve_provider_identity("custom")
                .expect("legacy route remains repeatedly resolvable")
                .key,
            "custom"
        );
    }
    Ok(())
}

#[test]
fn session_provider_identity_fails_closed_for_removed_or_invalid_custom_table() {
    let removed = Config::default();
    let missing = removed
        .resolve_provider_identity("lm-studio")
        .expect_err("removed provider must fail closed");
    assert!(missing.contains("[providers.lm-studio]"));
    assert!(missing.contains("will not fall back"));

    let invalid_kind = session_custom_provider_config(
        "lm-studio",
        "anthropic-messages",
        "http://127.0.0.1:1234/v1",
    );
    let kind_error = invalid_kind
        .resolve_provider_identity("lm-studio")
        .expect_err("unsupported custom wire kind must fail closed");
    assert!(kind_error.contains("kind = \"openai-compatible\""));

    let invalid_url =
        session_custom_provider_config("lm-studio", "openai-compatible", "not a provider URL");
    let url_error = invalid_url
        .resolve_provider_identity("lm-studio")
        .expect_err("invalid custom URL must fail closed");
    assert!(url_error.contains("base_url"));
    assert!(url_error.contains("will not fall back"));
}

#[test]
fn picker_consent_persists_only_confirmed_exact_scope_and_revoke_is_one_step() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    let external_path = dir.path().join("codex-auth.json");
    std::fs::write(
        &config_path,
        "# preserve operator comment\n[providers.openai_codex]\nmodel = \"gpt-5-codex\" # preserve model\n",
    )
    .expect("seed config");
    // #5772: persistence now reads and validates the exact confirmed file
    // before any consent record is written, so the fixture must hold a live
    // token (far-future JWT exp) for the grant to be saved at all.
    std::fs::write(
        &external_path,
        "{\"tokens\":{\"access_token\":\"header.eyJleHAiOjk5OTk5OTk5OTl9.sig\"}}",
    )
    .expect("seed external credential");
    // The secure adapter rejects symlinked path components; macOS `/var` is
    // one, so the confirmed path must be the canonical one.
    let external_path = external_path
        .canonicalize()
        .expect("canonical external credential path");
    let mut live = Config {
        provider: Some(ApiProvider::OpenaiCodex.as_str().to_string()),
        ..Config::default()
    };

    crate::external_credentials::reset_side_effect_trap();
    persist_external_credential_consent_for_at(
        Some(&config_path),
        &mut live,
        ApiProvider::OpenaiCodex,
        codewhale_config::ProviderKind::OpenaiCodex,
        codewhale_config::ExternalCredentialSource::CodexCli,
        &external_path,
    )
    .expect("persist confirmed consent");
    let saved = std::fs::read_to_string(&config_path).expect("saved config");
    assert!(saved.contains("# preserve operator comment"));
    assert!(saved.contains("model = \"gpt-5-codex\" # preserve model"));
    assert!(saved.contains("access = \"read_only\""));
    assert!(saved.contains("source = \"codex_cli\""));
    assert!(saved.contains(&external_path.display().to_string()));
    let consent = live
        .provider_config_for(ApiProvider::OpenaiCodex)
        .and_then(|entry| entry.external_credentials.as_ref())
        .expect("live consent");
    assert_eq!(consent.path, external_path);
    assert_eq!(
        crate::external_credentials::complete_side_effect_trap_counts(),
        (1, 1, 0, 0, 0),
        "grant persistence performs exactly the confirmed read (one secure open, one \
         bounded read) and never writes, refreshes, or reaches the network"
    );

    crate::external_credentials::reset_side_effect_trap();
    revoke_external_credential_consent_for_at(
        Some(&config_path),
        &mut live,
        ApiProvider::OpenaiCodex,
    )
    .expect("one-step revoke");
    let revoked = std::fs::read_to_string(&config_path).expect("revoked config");
    assert!(!revoked.contains("external_credentials"));
    assert!(
        live.provider_config_for(ApiProvider::OpenaiCodex)
            .and_then(|entry| entry.external_credentials.as_ref())
            .is_none()
    );
    assert_eq!(
        crate::external_credentials::complete_side_effect_trap_counts(),
        (0, 0, 0, 0, 0)
    );
}

/// Every provider must accept the model ids it advertises for itself.
///
/// Regression for #4829: `validate()` checked `default_text_model` against the
/// DeepSeek-only normalizer, so a config our own setup wizard writes
/// (`provider = "zai"`, `default_text_model = "GLM-5.2"`) was rejected on every
/// startup — the CLI could not launch at all. This asserts the equal-treatment
/// contract from CLAUDE.md: no provider's own models are second-class.
#[test]
fn validate_accepts_every_providers_own_advertised_models() {
    for &provider in ApiProvider::all() {
        for model in model_completion_names_for_provider(provider) {
            let config = Config {
                provider: Some(provider.as_str().to_string()),
                default_text_model: Some(model.to_string()),
                ..Default::default()
            };
            assert!(
                config.validate().is_ok(),
                "provider {} rejected its own advertised model {model}: {:?}",
                provider.as_str(),
                config.validate().unwrap_err().to_string(),
            );
        }
    }
}

/// The exact config that bricked the CLI in the field.
#[test]
fn validate_accepts_zai_glm_model_from_setup_wizard() {
    let config = Config {
        provider: Some("zai".to_string()),
        default_text_model: Some(DEFAULT_ZAI_MODEL.to_string()),
        ..Default::default()
    };
    config
        .validate()
        .expect("setup-wizard zai/GLM config must validate");
}

/// The official DeepSeek gate is the one legitimate per-family rejection and
/// must survive the fix — this is what keeps the validation meaningful.
#[test]
fn validate_still_rejects_unknown_model_on_official_deepseek() {
    let config = Config {
        provider: Some("deepseek".to_string()),
        default_text_model: Some("definitely-not-a-deepseek-model".to_string()),
        ..Default::default()
    };
    let err = config
        .validate()
        .expect_err("official DeepSeek must reject foreign ids")
        .to_string();
    assert!(
        err.contains("definitely-not-a-deepseek-model") && err.contains("deepseek"),
        "error should name the model and the active provider, got: {err}"
    );
}

#[test]
fn native_memory_scenario() {
    // Scenario consolidation of: native_memory_backend_owns_explicit_path, native_memory_path_honours_an_already_native_setting
    // from native_memory_backend_owns_explicit_path
    {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = tmp.path().join("legacy-memory.md");
        let config = Config {
            memory_path: Some(legacy.to_string_lossy().into_owned()),
            memory: Some(MemoryConfig {
                backend: Some(MemoryBackend::Native),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(config.memory_backend(), MemoryBackend::Native);
        assert!(config.memory_enabled());
        assert_eq!(
            config.memory_path(),
            tmp.path().join("memory/global/MEMORY.md")
        );
    }
    // from native_memory_path_honours_an_already_native_setting
    {
        // Pointing `memory_path` at a native store is the obvious reading of the
        // name; it used to nest a second store inside and write to the wrong file.
        let mut config = Config::default();
        config.memory = Some(crate::config::MemoryConfig {
            enabled: Some(true),
            ..Default::default()
        });
        config.memory_path = Some("/tmp/cw-test/memory/global/MEMORY.md".to_string());
        assert_eq!(
            config.memory_path(),
            std::path::PathBuf::from("/tmp/cw-test/memory/global/MEMORY.md")
        );

        // A legacy single-file setting still anchors the store beside it.
        config.memory_path = Some("/tmp/cw-test/memory.md".to_string());
        assert_eq!(
            config.memory_path(),
            std::path::PathBuf::from("/tmp/cw-test/memory/global/MEMORY.md")
        );
    }
}

/// Pins the v0.9.4 memory consolidation: with `[memory] enabled = true`
/// (no explicit backend), the resolved memory path is always the native
/// `memory/global/MEMORY.md` layout, so `NativeMemoryStore::from_global_path`
/// accepts it and the deleted legacy single-file branch can never be taken.
#[test]
fn enabled_memory_always_resolves_to_native_store_path() {
    let tmp = tempfile::tempdir().unwrap();
    let config: Config = toml::from_str(
        r#"
        [memory]
        enabled = true
        "#,
    )
    .expect("parse enabled memory config");
    assert_eq!(config.memory_backend(), MemoryBackend::Native);
    let path = config.memory_path();
    assert!(
        path.ends_with("memory/global/MEMORY.md"),
        "enabled memory must resolve to the native layout, got {}",
        path.display()
    );
    assert!(
        crate::native_memory::NativeMemoryStore::from_global_path(&path).is_some(),
        "native store must accept the resolved memory path"
    );

    // Even an explicitly configured single-file path is re-rooted into the
    // native layout under the native backend.
    let custom = Config {
        memory_path: Some(tmp.path().join("memory.md").to_string_lossy().into_owned()),
        memory: Some(MemoryConfig {
            enabled: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    };
    let custom_path = custom.memory_path();
    assert!(
        crate::native_memory::NativeMemoryStore::from_global_path(&custom_path).is_some(),
        "custom memory paths are re-rooted into the native layout, got {}",
        custom_path.display()
    );
}

/// v0.9.1 kimi-k3 dogfood report: a dogfood user ran `codewhale --provider moonshot --model kimi-k3`
/// and the session kept reporting `kimi-k2.7-code`. The `--model` flag reaches
/// this binary as `CODEWHALE_MODEL`, so the route it produces is asserted here
/// end to end: the effective model, the endpoint, and the id that goes on the
/// wire must all be the one the user named.
#[test]
fn cli_model_flag_selects_kimi_k3_on_the_moonshot_platform_route() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-kimi-k3-cli-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    // `EnvGuard` points DEEPSEEK_CONFIG_PATH at `<home>/.deepseek/config.toml`.
    let config_path = temp_root.join(".deepseek").join("config.toml");
    ensure_parent_dir(&config_path)?;
    fs::write(
        &config_path,
        "provider = \"moonshot\"\n\n[providers.moonshot]\napi_key = \"k\"\n",
    )?;
    // Safety: test-only env mutation guarded by lock_test_env().
    unsafe {
        env::set_var("CODEWHALE_PROVIDER", "moonshot");
        env::set_var("CODEWHALE_MODEL", MOONSHOT_KIMI_K3_MODEL);
    }

    let config = Config::load(None, None)?;

    assert_eq!(config.api_provider(), ApiProvider::Moonshot);
    assert_eq!(config.default_model(), MOONSHOT_KIMI_K3_MODEL);
    assert_eq!(config.deepseek_base_url(), DEFAULT_MOONSHOT_BASE_URL);
    assert_eq!(
        wire_model_for_provider_route(
            ApiProvider::Moonshot,
            &config.deepseek_base_url(),
            &config.default_model(),
        ),
        MOONSHOT_KIMI_K3_MODEL,
        "the id the user named must be the id on the wire"
    );
    assert_eq!(
        explicit_launch_model_override().as_deref(),
        Some(MOONSHOT_KIMI_K3_MODEL),
        "an explicit --model must remain recognizable as an explicit request"
    );
    assert_eq!(
        moonshot_k3_route_display_name(&config.deepseek_base_url(), &config.default_model()),
        Some("Moonshot direct / kimi-k3")
    );
    Ok(())
}

// === Identity-owned endpoint resolution (provider-truth regressions) ===
//
// Every test here is offline and env-locked. No credential is invented and no
// provider is contacted: the assertions are about which host string a route
// resolves to, and about the classifications derived from it.

/// A managed-config guard pointing at a path that does not exist, so an
/// operator-installed managed file on the developer's machine cannot leak into
/// these route assertions.
fn no_managed_config(root: &std::path::Path) -> EnvVarGuard {
    EnvVarGuard::set(
        "DEEPSEEK_MANAGED_CONFIG_PATH",
        root.join("absent-managed.toml"),
    )
}

fn custom_placeholder_base_url() -> String {
    normalize_base_url(
        codewhale_config::ProviderKind::Custom
            .provider()
            .default_base_url(),
    )
}

#[test]
fn env_owned_deepseek_root_base_url_does_not_reach_the_deepseek_cn_sibling() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let _managed = no_managed_config(temp_root.path());
    let config_path = temp_root.path().join("config.toml");
    fs::write(&config_path, "provider = \"deepseek\"\n")?;
    let _base = EnvVarGuard::set("CODEWHALE_BASE_URL", "https://env-gateway.example.test/v1");

    let config = Config::load(Some(config_path), None)?;

    // The env override owns the route it was addressed to.
    assert_eq!(config.api_provider(), ApiProvider::Deepseek);
    assert_eq!(
        config.deepseek_base_url(),
        "https://env-gateway.example.test/v1"
    );
    assert!(config.provider_uses_custom_endpoint(ApiProvider::Deepseek));

    // The sibling identity shares the same legacy root field but is a
    // different route: it must fall through to its own canonical endpoint.
    assert_eq!(
        config.base_url_for_route(ApiProvider::DeepseekCN),
        DEFAULT_DEEPSEEKCN_BASE_URL
    );
    assert!(!config.provider_uses_custom_endpoint(ApiProvider::DeepseekCN));
    assert!(!config.model_ids_pass_through_for_provider(ApiProvider::DeepseekCN));
    Ok(())
}

#[test]
fn env_owned_deepseek_cn_root_base_url_does_not_reach_the_deepseek_sibling() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let _managed = no_managed_config(temp_root.path());
    let config_path = temp_root.path().join("config.toml");
    fs::write(&config_path, "provider = \"deepseek-cn\"\n")?;
    let _base = EnvVarGuard::set(
        "DEEPSEEK_BASE_URL",
        "https://cn-env-gateway.example.test/v1",
    );

    let config = Config::load(Some(config_path), None)?;

    assert_eq!(config.api_provider(), ApiProvider::DeepseekCN);
    assert_eq!(
        config.deepseek_base_url(),
        "https://cn-env-gateway.example.test/v1"
    );
    assert!(config.provider_uses_custom_endpoint(ApiProvider::DeepseekCN));

    assert_eq!(
        config.base_url_for_route(ApiProvider::Deepseek),
        DEFAULT_DEEPSEEK_BASE_URL
    );
    assert!(!config.provider_uses_custom_endpoint(ApiProvider::Deepseek));
    Ok(())
}

#[test]
fn file_owned_legacy_root_base_url_stays_shared_by_both_deepseek_identities() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let _managed = no_managed_config(temp_root.path());
    let config_path = temp_root.path().join("config.toml");
    fs::write(
        &config_path,
        "provider = \"deepseek\"\nbase_url = \"https://file-gateway.example.test/v1\"\n",
    )?;

    let config = Config::load(Some(config_path), None)?;

    // No environment write, so the root field is the user's own. Both
    // identities keep reading it, exactly as they always have.
    for provider in [ApiProvider::Deepseek, ApiProvider::DeepseekCN] {
        assert_eq!(
            config.base_url_for_route(provider),
            "https://file-gateway.example.test/v1",
            "{provider:?} must keep the file-owned legacy root endpoint"
        );
        assert!(config.provider_uses_custom_endpoint(provider));
    }
    Ok(())
}

#[test]
fn managed_overlay_keeps_pinned_children_off_the_ambient_generic_host() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let config_path = temp_root.path().join("config.toml");
    let managed_path = temp_root.path().join("managed.toml");
    fs::write(&config_path, "provider = \"deepseek\"\n")?;
    fs::write(
        &managed_path,
        "provider = \"openrouter\"\n\n[providers.openrouter]\nbase_url = \"https://managed-gateway.example.test/v1\"\n",
    )?;
    let _managed = EnvVarGuard::set("DEEPSEEK_MANAGED_CONFIG_PATH", &managed_path);
    let _base = EnvVarGuard::set("CODEWHALE_BASE_URL", "https://env-gateway.example.test/v1");

    let config = Config::load(Some(config_path), None)?;

    // Managed routing is authoritative for the active route.
    assert_eq!(config.api_provider(), ApiProvider::Openrouter);
    assert_eq!(
        config.deepseek_base_url(),
        "https://managed-gateway.example.test/v1"
    );

    // The receipt must say "nobody owns the generic override" rather than
    // being cleared: a cleared receipt reads as "never met the environment
    // layer" and re-enables the generic fallback for every pinned child.
    assert_eq!(config.base_url_env_receipt, BaseUrlEnvReceipt::NoOwner);
    assert_eq!(config.root_base_url_owner, BaseUrlEnvReceipt::NoOwner);
    for provider in [
        ApiProvider::Moonshot,
        ApiProvider::Zai,
        ApiProvider::Minimax,
        ApiProvider::Deepseek,
        ApiProvider::DeepseekCN,
    ] {
        assert_eq!(
            config.base_url_for_route(provider),
            provider.default_base_url(),
            "{provider:?} must not borrow the ambient generic host under managed routing"
        );
        assert!(!config.provider_uses_custom_endpoint(provider));
    }
    Ok(())
}

#[test]
fn named_custom_children_resolve_by_identity_not_by_the_active_custom_route() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let _managed = no_managed_config(temp_root.path());
    let config_path = temp_root.path().join("config.toml");
    fs::write(
        &config_path,
        r#"provider = "acme"

[providers.acme]
base_url = "https://acme.example.test/v1"
model = "acme-1"

[providers.beta]
base_url = "https://beta.example.test/v1"
model = "beta-1"
"#,
    )?;

    let config = Config::load(Some(config_path), None)?;

    assert_eq!(config.api_provider(), ApiProvider::Custom);
    assert_eq!(config.deepseek_base_url(), "https://acme.example.test/v1");
    // A pinned child of the other named custom table resolves its own host.
    assert_eq!(
        config.base_url_for_route_identity(ApiProvider::Custom, "beta"),
        "https://beta.example.test/v1"
    );
    assert!(config.custom_identity_is_resolvable("beta"));
    Ok(())
}

#[test]
fn missing_custom_identity_fails_closed_instead_of_reading_the_active_custom() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let _managed = no_managed_config(temp_root.path());
    let config_path = temp_root.path().join("config.toml");
    fs::write(
        &config_path,
        r#"provider = "acme"

[providers.acme]
base_url = "https://acme.example.test/v1"
model = "acme-1"
"#,
    )?;

    let config = Config::load(Some(config_path), None)?;
    let placeholder = custom_placeholder_base_url();

    // A removed/renamed table, an empty identity, and the literal `custom`
    // key on a config that is not the legacy root-literal route all fail
    // closed to the descriptor placeholder — never to the active custom host.
    for identity in ["ghost", "", "   ", "custom"] {
        let resolved = config.base_url_for_route_identity(ApiProvider::Custom, identity);
        assert_eq!(
            resolved, placeholder,
            "identity {identity:?} must not resolve to the active custom endpoint"
        );
        assert_ne!(resolved, "https://acme.example.test/v1");
        assert!(!config.custom_identity_is_resolvable(identity));
    }
    Ok(())
}

#[test]
fn legacy_literal_custom_root_endpoint_belongs_only_to_the_literal_identity() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let _managed = no_managed_config(temp_root.path());
    let config_path = temp_root.path().join("config.toml");
    fs::write(
        &config_path,
        r#"provider = "custom"
base_url = "https://legacy-root.example.test/v1"
default_text_model = "legacy-1"
"#,
    )?;

    let config = Config::load(Some(config_path), None)?;

    assert!(config.uses_legacy_literal_custom_route());
    assert_eq!(
        config.base_url_for_route_identity(ApiProvider::Custom, "custom"),
        "https://legacy-root.example.test/v1"
    );
    // A differently named custom child must not inherit the legacy root.
    assert_eq!(
        config.base_url_for_route_identity(ApiProvider::Custom, "acme"),
        custom_placeholder_base_url()
    );
    Ok(())
}

/// The bare `k3` id belongs to the Kimi Code coding-plan endpoint. A config
/// that selects it there must resolve, and must be labelled as the membership
/// product rather than the direct platform one (v0.9.1 kimi-k3 dogfood report).
#[test]
fn config_selects_bare_k3_on_the_kimi_code_route() {
    let config = Config {
        provider: Some("moonshot".to_string()),
        providers: Some(ProvidersConfig {
            moonshot: ProviderConfig {
                api_key: Some("k".to_string()),
                base_url: Some(DEFAULT_KIMI_CODE_BASE_URL.to_string()),
                model: Some(KIMI_CODE_K3_MODEL.to_string()),
                ..ProviderConfig::default()
            },
            ..ProvidersConfig::default()
        }),
        ..Config::default()
    };

    assert_eq!(config.default_model(), KIMI_CODE_K3_MODEL);
    assert_eq!(
        wire_model_for_provider_route(
            ApiProvider::Moonshot,
            &config.deepseek_base_url(),
            &config.default_model(),
        ),
        KIMI_CODE_K3_MODEL
    );
    assert_eq!(
        moonshot_k3_route_display_name(&config.deepseek_base_url(), &config.default_model()),
        Some("Kimi Code membership / k3")
    );
}

/// Neither K3 id may be silently served by the other product's endpoint. The
/// two are different plans with different context windows, so an unservable
/// pairing has to fail loudly and name both routes (v0.9.1 kimi-k3 dogfood report).
#[test]
fn k3_and_kimi_k3_never_cross_products_and_fail_visibly() {
    let crossed = validate_kimi_code_api_model_id(
        ApiProvider::Moonshot,
        DEFAULT_KIMI_CODE_BASE_URL,
        MOONSHOT_KIMI_K3_MODEL,
    )
    .expect_err("kimi-k3 is not a Kimi Code model id");
    assert!(crossed.contains("api.kimi.com/coding/v1"), "{crossed}");
    assert!(crossed.contains("api.moonshot.ai/v1"), "{crossed}");
    assert!(crossed.contains(KIMI_CODE_K3_MODEL), "{crossed}");

    let reversed = validate_kimi_code_api_model_id(
        ApiProvider::Moonshot,
        DEFAULT_MOONSHOT_BASE_URL,
        KIMI_CODE_K3_MODEL,
    )
    .expect_err("bare k3 is not a direct-platform model id");
    assert!(reversed.contains("api.moonshot.ai/v1"), "{reversed}");
    assert!(reversed.contains("api.kimi.com/coding/v1"), "{reversed}");
    assert!(reversed.contains(MOONSHOT_KIMI_K3_MODEL), "{reversed}");

    // The exact-route predicates stay disjoint.
    assert!(is_exact_direct_moonshot_k3_route(
        ApiProvider::Moonshot,
        DEFAULT_MOONSHOT_BASE_URL,
        MOONSHOT_KIMI_K3_MODEL
    ));
    assert!(!is_exact_kimi_code_k3_route(
        ApiProvider::Moonshot,
        DEFAULT_MOONSHOT_BASE_URL,
        MOONSHOT_KIMI_K3_MODEL
    ));
    assert!(is_exact_kimi_code_k3_route(
        ApiProvider::Moonshot,
        DEFAULT_KIMI_CODE_BASE_URL,
        KIMI_CODE_K3_MODEL
    ));
    assert!(is_exact_kimi_code_k3_route(
        ApiProvider::Moonshot,
        DEFAULT_KIMI_CODE_BASE_URL,
        KIMI_CODE_K3_256K_MODEL
    ));
    assert!(!is_exact_kimi_code_bare_k3_route(
        ApiProvider::Moonshot,
        DEFAULT_KIMI_CODE_BASE_URL,
        KIMI_CODE_K3_256K_MODEL
    ));
    assert!(!is_exact_direct_moonshot_k3_route(
        ApiProvider::Moonshot,
        DEFAULT_KIMI_CODE_BASE_URL,
        KIMI_CODE_K3_MODEL
    ));
}

#[test]
fn unknown_models_pass_through_on_canonical_moonshot_endpoints() {
    for base_url in [DEFAULT_KIMI_CODE_BASE_URL, DEFAULT_MOONSHOT_BASE_URL] {
        validate_kimi_code_api_model_id(ApiProvider::Moonshot, base_url, "future-kimi-model")
            .expect("unknown model IDs remain provider-owned");
    }
}

#[test]
fn dispatch_endpoint_and_billing_receipts_agree_for_every_resolved_route() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let _managed = no_managed_config(temp_root.path());
    let config_path = temp_root.path().join("config.toml");
    fs::write(&config_path, "provider = \"deepseek\"\n")?;
    let _base = EnvVarGuard::set("CODEWHALE_BASE_URL", "https://env-gateway.example.test/v1");

    let config = Config::load(Some(config_path), None)?;

    // `for_route` reads the ambient config; `for_dispatched_route` reads the
    // endpoint the client is actually built from. After the resolver became
    // identity-aware these must not be able to disagree for the active route.
    let provider = config.api_provider();
    let resolved = config.deepseek_base_url();
    assert_eq!(
        crate::route_billing::for_route(&config, provider),
        crate::route_billing::for_dispatched_route(
            &config,
            crate::route_billing::DispatchedRoute {
                provider,
                base_url: &resolved,
            },
        )
    );

    // A pinned cross-provider child bills from its own resolved endpoint,
    // which is its canonical host — not the session's env-selected gateway.
    for child in [ApiProvider::DeepseekCN, ApiProvider::Moonshot] {
        let child_base = config.base_url_for_route(child);
        assert_eq!(child_base, child.default_base_url(), "{child:?}");
        assert_eq!(
            crate::route_billing::for_dispatched_route(
                &config,
                crate::route_billing::DispatchedRoute {
                    provider: child,
                    base_url: &child_base,
                },
            ),
            crate::route_billing::for_route(&config, child),
            "{child:?} ambient and dispatch billing receipts must agree"
        );
    }
    Ok(())
}

#[test]
fn readiness_and_inventory_classify_the_resolved_route_not_the_session_host() -> Result<()> {
    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let _guard = EnvGuard::new(temp_root.path());
    let _managed = no_managed_config(temp_root.path());
    let config_path = temp_root.path().join("config.toml");
    fs::write(&config_path, "provider = \"deepseek\"\n")?;
    let _base = EnvVarGuard::set("CODEWHALE_BASE_URL", "http://127.0.0.1:11434/v1");

    let config = Config::load(Some(config_path), None)?;

    // Readiness: the active route is on a local custom host and classifies as
    // keyless-local; the sibling identity is still the canonical hosted
    // endpoint and must not inherit that classification.
    assert_eq!(
        crate::provider_readiness::credential_state_for_provider(&config, ApiProvider::Deepseek),
        crate::provider_readiness::CredentialState::Local
    );
    assert_ne!(
        crate::provider_readiness::credential_state_for_provider(&config, ApiProvider::DeepseekCN),
        crate::provider_readiness::CredentialState::Local
    );

    // Inventory: the runtime route the picker/inventory reads is built by
    // re-pointing a clone of this config, so it must resolve the sibling's own
    // canonical endpoint.
    let route = crate::route_runtime::resolve_runtime_route(&config, ApiProvider::DeepseekCN, None)
        .expect("deepseek-cn runtime route");
    assert_eq!(
        route.candidate.endpoint().base_url,
        DEFAULT_DEEPSEEKCN_BASE_URL
    );
    assert_eq!(
        route.config.deepseek_base_url(),
        DEFAULT_DEEPSEEKCN_BASE_URL
    );

    // And a canonical/default endpoint is never reported as custom.
    assert!(!config.provider_uses_custom_endpoint(ApiProvider::DeepseekCN));
    assert!(config.provider_uses_custom_endpoint(ApiProvider::Deepseek));
    Ok(())
}

#[test]
fn configured_inactive_provider_reads_its_secret_store_key() -> Result<()> {
    let _lock = lock_test_env();
    let temp = tempfile::tempdir()?;
    let _home = EnvVarGuard::set("CODEWHALE_HOME", temp.path());
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
    let _moonshot = EnvVarGuard::remove("MOONSHOT_API_KEY");
    let _kimi = EnvVarGuard::remove("KIMI_API_KEY");

    // The state guided setup leaves behind: auth_mode saved to config, the
    // key saved to the secret store only — and the operator then switches
    // the active provider away (#5033).
    let providers = ProvidersConfig {
        moonshot: ProviderConfig {
            auth_mode: Some("api_key".to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    let config = Config {
        provider: Some("deepseek".to_string()),
        providers: Some(providers),
        ..Default::default()
    };

    assert!(
        !has_api_key_for(&config, ApiProvider::Moonshot),
        "no stored key yet: the configured provider must still read as unconfigured"
    );

    codewhale_secrets::Secrets::auto_detect().set("moonshot", "kimi-test-credential")?;
    assert!(
        has_api_key_for(&config, ApiProvider::Moonshot),
        "a configured-but-inactive provider with a stored key must read as configured (#5033)"
    );
    Ok(())
}

/// A self-hosted OpenAI-compatible gateway owns its model namespace, including
/// its casing. A WeChat-community user configured `DeepSeek-V4-Flash` against
/// their company's internal endpoint and reported the id coming back lowercase
/// — an id that endpoint does not serve. Every stage from the parsed config to
/// the resolved wire id must hand the string back byte-for-byte.
#[test]
fn custom_endpoint_model_id_survives_verbatim_through_the_route() {
    const MODEL: &str = "DeepSeek-V4-Flash";
    let shapes: [(&str, &str); 6] = [
        (
            "provider-scoped deepseek table with a custom base_url",
            r#"
provider = "deepseek"
[providers.deepseek]
base_url = "https://llm.corp.internal/v1"
api_key = "k"
model = "DeepSeek-V4-Flash"
"#,
        ),
        (
            "deepseek table with no explicit provider key",
            r#"
[providers.deepseek]
base_url = "https://llm.corp.internal/v1"
api_key = "k"
model = "DeepSeek-V4-Flash"
"#,
        ),
        (
            "root base_url with the root default_text_model",
            r#"
base_url = "https://llm.corp.internal/v1"
api_key = "k"
default_text_model = "DeepSeek-V4-Flash"
"#,
        ),
        (
            "anthropic dialect on a custom deepseek endpoint",
            r#"
provider = "deepseek"
[providers.deepseek]
base_url = "https://llm.corp.internal/anthropic"
api_key = "k"
model = "DeepSeek-V4-Flash"
wire = "anthropic"
"#,
        ),
        (
            "openai-compatible provider table",
            r#"
provider = "openai"
[providers.openai]
base_url = "https://llm.corp.internal/v1"
api_key = "k"
model = "DeepSeek-V4-Flash"
"#,
        ),
        (
            "literal custom provider on a root base_url",
            r#"
provider = "custom"
base_url = "https://llm.corp.internal/v1"
api_key = "k"
default_text_model = "DeepSeek-V4-Flash"
"#,
        ),
    ];

    for (label, body) in shapes {
        let mut config: Config = toml::from_str(body).expect("config parses");
        normalize_model_config(&mut config);
        let provider = config.api_provider();
        let base_url = config.base_url_for_route(provider);
        assert!(
            provider_preserves_custom_base_url_model(provider, &base_url),
            "{label}: {base_url} must classify as a custom endpoint"
        );

        let stored = config
            .provider_config_for(provider)
            .and_then(|entry| entry.model.clone())
            .or_else(|| config.default_text_model.clone())
            .unwrap_or_default();
        assert_eq!(stored, MODEL, "{label}: config load rewrote the stored id");

        let resolved = config.default_model();
        assert_eq!(resolved, MODEL, "{label}: default_model() rewrote the id");

        assert_eq!(
            wire_model_for_provider_route(provider, &base_url, &resolved),
            MODEL,
            "{label}: the wire id must match the configured id byte-for-byte"
        );

        let route = crate::route_runtime::resolve_runtime_route(&config, provider, Some(&resolved))
            .unwrap_or_else(|err| panic!("{label}: route resolution failed: {err}"));
        assert_eq!(
            route.model, MODEL,
            "{label}: the resolved route rewrote the id"
        );
    }
}

/// Preserving the user's spelling must not turn alias resolution
/// case-sensitive: normalize for comparison, never mutate what is stored or
/// sent. A first-party/catalog route still canonicalizes to its documented id.
#[test]
fn model_alias_matching_stays_case_insensitive() {
    // Mixed-case aliases still resolve to the provider's documented wire id.
    assert_eq!(
        normalize_model_name_for_provider(ApiProvider::NvidiaNim, "DeepSeek-V4-Pro").as_deref(),
        Some(DEFAULT_NVIDIA_NIM_MODEL)
    );
    assert_eq!(
        normalize_model_name_for_provider(ApiProvider::Openrouter, "DeepSeek-V4-Flash").as_deref(),
        Some(DEFAULT_OPENROUTER_FLASH_MODEL)
    );
    assert_eq!(
        canonical_model_id_for_provider(ApiProvider::Zai, "GLM-5.1").as_deref(),
        canonical_model_id_for_provider(ApiProvider::Zai, "glm-5.1").as_deref()
    );

    // A first-party DeepSeek route still migrates the retired aliases,
    // whatever case they are typed in.
    for alias in ["deepseek-chat", "DeepSeek-Chat", "DEEPSEEK-REASONER"] {
        assert_eq!(
            wire_model_for_provider_route(ApiProvider::Deepseek, "https://api.deepseek.com", alias),
            DEEPSEEK_ALIAS_REPLACEMENT,
            "first-party DeepSeek must canonicalize {alias}"
        );
    }

    // The same alias on a custom endpoint keeps the user's exact spelling:
    // that endpoint owns both the id and its meaning.
    assert_eq!(
        wire_model_for_provider_route(
            ApiProvider::Deepseek,
            "https://llm.corp.internal/v1",
            "DeepSeek-Chat"
        ),
        "DeepSeek-Chat"
    );
}

/// The remembered `/model` pick outranks `config.toml` on the next launch. It
/// must not silently restyle the configured id: two spellings of one model are
/// the config file's call, a genuinely different model stays the memory's.
#[test]
fn remembered_model_pick_defers_to_the_configured_spelling() {
    assert_eq!(
        prefer_configured_model_spelling("DeepSeek-V4-Flash", "deepseek-v4-flash".to_string()),
        "DeepSeek-V4-Flash",
        "a case-only disagreement belongs to config.toml"
    );
    assert_eq!(
        prefer_configured_model_spelling("  DeepSeek-V4-Flash  ", "DEEPSEEK-V4-FLASH".to_string()),
        "DeepSeek-V4-Flash"
    );
    assert_eq!(
        prefer_configured_model_spelling("DeepSeek-V4-Flash", "deepseek-v4-pro".to_string()),
        "deepseek-v4-pro",
        "a different model is still a real remembered selection"
    );
    assert_eq!(
        prefer_configured_model_spelling("DeepSeek-V4-Flash", "auto".to_string()),
        "auto"
    );
    assert_eq!(
        prefer_configured_model_spelling("DeepSeek-V4-Flash", "DeepSeek-V4-Flash".to_string()),
        "DeepSeek-V4-Flash"
    );
}

/// Reproduces the report that started the credential-resolution lane: a home
/// whose secret store holds a working DeepSeek key, where the provider picker
/// reads "missing key" while a real turn from the same home resolves that key.
///
/// The asymmetry is #5033's marker gate. For a provider that is not currently
/// active, `has_api_key_for` reads the durable slot only when some sibling's
/// `[providers.<name>]` table carries the api-key auth-mode marker. The request
/// path has no such gate — it reads the slot for whatever provider is active.
/// Any home where the marker is absent but the slot is populated (a config
/// written by an older CodeWhale, a workspace config loaded in place of the
/// user-global one, a key written straight into the store) makes the two
/// surfaces contradict each other, and nothing in the old picker output let a
/// user tell which one was lying.
///
/// This test does not change that policy — the gate is what keeps catalog
/// rendering from opening a write-capable keyring for 40 providers. It pins
/// the contradiction, and pins the part that is now fixed: the row says the
/// slot was skipped, and why.
#[test]
fn picker_and_request_path_disagree_when_the_secret_slot_marker_is_missing() -> Result<()> {
    use crate::credentials::CredentialSource;

    let _lock = lock_test_env();
    let temp_root = tempfile::tempdir()?;
    let temp_root = temp_root.path().canonicalize()?;
    let _guard = EnvGuard::new(&temp_root);
    let codewhale_home = temp_root.join("codewhale-home");
    let config_path = codewhale_home.join("config.toml");
    let _home = EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());
    let _path = EnvVarGuard::set("CODEWHALE_CONFIG_PATH", config_path.as_os_str());
    let _backend = EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");

    save_api_key("deepseek-working-key")?;
    assert_eq!(
        codewhale_secrets::Secrets::auto_detect().get("deepseek")?,
        Some("deepseek-working-key".to_string()),
        "precondition: the durable slot holds a usable key"
    );
    // Drop the marker the save wrote, standing in for a home whose config was
    // written by an older CodeWhale or replaced by a workspace-scoped file.
    fs::write(&config_path, "provider = \"openrouter\"\n")?;

    let active_deepseek = Config {
        provider: Some("deepseek".to_string()),
        ..Config::default()
    };
    assert_eq!(
        active_deepseek.deepseek_api_key().ok(),
        Some("deepseek-working-key".to_string()),
        "the request path still resolves the stored key"
    );

    let viewing_config = Config {
        provider: Some("openrouter".to_string()),
        ..Config::default()
    };
    assert!(
        !has_api_key_for(&viewing_config, ApiProvider::Deepseek),
        "the reported contradiction: readiness reports no key for the same home"
    );

    let resolution = resolve_credential_source(&viewing_config, ApiProvider::Deepseek);
    assert!(matches!(
        resolution.source,
        CredentialSource::Missing { .. }
    ));
    let checked = resolution.checked_places();
    assert!(
        checked
            .contains("secret store \"deepseek\" (not read: inactive provider, no api-key marker)"),
        "the row must explain the skip instead of implying an empty slot: {checked}"
    );
    assert!(
        resolution.source.probed().iter().any(|probe| probe
            .fix
            .as_deref()
            .is_some_and(|fix| fix.contains("codewhale auth set"))),
        "the row must offer the command that resolves it: {resolution:?}"
    );
    println!("CAPTURED checked-places: {checked}");
    println!("CAPTURED first-fix: {:?}", resolution.first_fix());
    Ok(())
}
