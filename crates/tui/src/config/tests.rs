use super::*;
use crate::test_support::{EnvVarGuard, lock_test_env};
use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::time::{SystemTime, UNIX_EPOCH};

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
fn provider_context_window_loads_from_provider_table() -> Result<()> {
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

    Ok(())
}

#[test]
fn provider_context_window_zero_is_invalid() {
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

#[test]
fn missing_provider_api_key_message_uses_provider_metadata() -> Result<()> {
    let message = missing_provider_api_key_message(ApiProvider::Zai)?;

    assert!(message.contains("Zhipu AI / Z.ai API key not found"));
    assert!(message.contains("https://z.ai/model-api"));
    assert!(message.contains("ZAI_API_KEY / Z_AI_API_KEY"));
    assert!(message.contains("[providers.zai] api_key"));

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
fn prompt_suggestion_defaults_to_false() {
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

#[test]
fn prompt_suggestion_enabled_when_set_true() {
    let config = Config {
        prompt_suggestion: Some(true),
        ..Default::default()
    };
    assert!(config.prompt_suggestion_enabled());
}

#[test]
fn auto_review_config_builds_runtime_policy() -> Result<()> {
    let config: Config = toml::from_str(
        r#"
[auto_review]
guidance = "Prefer review before remote side effects."

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
    assert_eq!(
        policy.natural_language_guidance.as_deref(),
        Some("Prefer review before remote side effects.")
    );

    let shell_context = crate::tui::auto_review::AutoReviewContext::from_tool_call(
        "exec_shell",
        &serde_json::json!({"command": "cargo test"}),
        crate::tui::auto_review::RunOrigin::Interactive,
        crate::tui::approval::ApprovalMode::Auto,
        Some("run tests"),
        true,
        false,
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
        Some("read the docs"),
        true,
        false,
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
fn auto_review_profile_overrides_base_policy() -> Result<()> {
    let parsed: ConfigFile = toml::from_str(
        r#"
[auto_review]
guidance = "base"

[[auto_review.block]]
action_kind = "shell"

[profiles.strict.auto_review]
guidance = "strict"

[[profiles.strict.auto_review.block]]
action_kind = "network"
"#,
    )?;

    let merged = apply_profile(parsed, Some("strict"))?;
    let policy = merged.auto_review_policy();

    assert_eq!(policy.natural_language_guidance.as_deref(), Some("strict"));
    assert_eq!(policy.block_rules.len(), 1);
    assert_eq!(
        policy.block_rules[0].action_kind,
        Some(crate::tui::auto_review::ToolActionKind::Network)
    );

    Ok(())
}

#[test]
fn auto_review_config_rejects_invalid_rule_shapes() {
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
    assert_eq!(default_config_path().as_deref(), Some(expected.as_path()));
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
fn profile_without_hotbar_keeps_base_hotbar() {
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

#[test]
fn update_config_defaults_to_enabled_without_uri() {
    let config = Config::default();
    assert_eq!(config.update, None);
    assert_eq!(config.update_config(), UpdateConfig::default());
    assert!(config.update_config().check_for_updates);
    assert_eq!(config.update_config().update_uri(), None);
}

#[test]
fn update_config_deserializes_disable_and_custom_uri() {
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

#[test]
fn network_policy_toml_maps_proxy_hosts_to_runtime_policy() {
    let policy: NetworkPolicyToml = toml::from_str(
        r#"
        default = "allow"
        proxy = ["github.com", ".githubusercontent.com"]
        "#,
    )
    .expect("network policy toml");

    let runtime = policy.into_runtime();

    assert_eq!(runtime.proxy, ["github.com", ".githubusercontent.com"]);
    assert!(runtime.trusts_proxy_fakeip_host("github.com"));
    assert!(runtime.trusts_proxy_fakeip_host("raw.githubusercontent.com"));
}

#[test]
fn verifier_config_parses_hunt_policy_and_merges_overrides() {
    let config: Config = toml::from_str(
        r#"
        [verifier]
        enabled = true
        verdict_policy = "hunt"
        "#,
    )
    .expect("parse verifier config");

    let verifier = config.verifier.expect("verifier table");
    assert!(verifier.enabled);
    assert_eq!(
        verifier.verdict_policy,
        codewhale_config::VerifierVerdictPolicy::Hunt
    );

    let merged = merge_config(
        Config {
            verifier: Some(codewhale_config::VerifierConfigToml {
                enabled: false,
                verdict_policy: codewhale_config::VerifierVerdictPolicy::Hunt,
            }),
            ..Config::default()
        },
        Config {
            verifier: Some(codewhale_config::VerifierConfigToml {
                enabled: true,
                verdict_policy: codewhale_config::VerifierVerdictPolicy::Hunt,
            }),
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
fn search_provider_defaults_to_duckduckgo() {
    assert_eq!(SearchProvider::default(), SearchProvider::DuckDuckGo);
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
fn search_provider_resolution_reports_default_source() {
    let _guard = lock_test_env();
    let prev = env::var_os("DEEPSEEK_SEARCH_PROVIDER");
    unsafe { env::remove_var("DEEPSEEK_SEARCH_PROVIDER") };

    let resolution = Config::default().search_provider_resolution();

    unsafe { EnvGuard::restore_var("DEEPSEEK_SEARCH_PROVIDER", prev) };
    assert_eq!(resolution.provider, SearchProvider::DuckDuckGo);
    assert_eq!(resolution.source, SearchProviderSource::Default);
}

#[test]
fn search_provider_resolution_reports_config_source() {
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

#[test]
fn search_provider_resolution_reports_env_override_source() {
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

#[test]
fn search_provider_env_override_accepts_baidu() {
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

#[test]
fn apply_env_overrides_sets_search_api_key() {
    let _guard = lock_test_env();
    let prev = env::var_os("DEEPSEEK_SEARCH_API_KEY");
    unsafe { env::set_var("DEEPSEEK_SEARCH_API_KEY", "search-env-key") };
    let mut config = Config::default();

    apply_env_overrides(&mut config);

    unsafe { EnvGuard::restore_var("DEEPSEEK_SEARCH_API_KEY", prev) };
    assert_eq!(
        config.search.and_then(|search| search.api_key),
        Some("search-env-key".to_string())
    );
}

#[test]
fn apply_env_overrides_sets_search_base_url() {
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

    apply_env_overrides(&mut config);

    unsafe {
        EnvGuard::restore_var("CODEWHALE_SEARCH_BASE_URL", prev_codewhale);
        EnvGuard::restore_var("DEEPSEEK_SEARCH_BASE_URL", prev_deepseek);
    }
    assert_eq!(
        config.search.and_then(|search| search.base_url),
        Some("https://search.internal.example/html/".to_string())
    );
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

    apply_env_overrides(&mut config);

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
fn search_provider_resolution_ignores_invalid_env_override() {
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

struct EnvGuard {
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
    kimi_code_oauth_host: Option<OsString>,
    kimi_oauth_host: Option<OsString>,
    sglang_api_key: Option<OsString>,
    sglang_base_url: Option<OsString>,
    sglang_model: Option<OsString>,
    vllm_api_key: Option<OsString>,
    vllm_base_url: Option<OsString>,
    vllm_model: Option<OsString>,
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
        let kimi_code_oauth_host_prev = env::var_os("KIMI_CODE_OAUTH_HOST");
        let kimi_oauth_host_prev = env::var_os("KIMI_OAUTH_HOST");
        let sglang_api_key_prev = env::var_os("SGLANG_API_KEY");
        let sglang_base_url_prev = env::var_os("SGLANG_BASE_URL");
        let sglang_model_prev = env::var_os("SGLANG_MODEL");
        let vllm_api_key_prev = env::var_os("VLLM_API_KEY");
        let vllm_base_url_prev = env::var_os("VLLM_BASE_URL");
        let vllm_model_prev = env::var_os("VLLM_MODEL");
        let ollama_api_key_prev = env::var_os("OLLAMA_API_KEY");
        let ollama_base_url_prev = env::var_os("OLLAMA_BASE_URL");
        let ollama_model_prev = env::var_os("OLLAMA_MODEL");
        let huggingface_api_key_prev = env::var_os("HUGGINGFACE_API_KEY");
        let huggingface_token_prev = env::var_os("HF_TOKEN");
        let huggingface_base_url_prev = env::var_os("HUGGINGFACE_BASE_URL");
        let hf_base_url_prev = env::var_os("HF_BASE_URL");
        let huggingface_model_prev = env::var_os("HUGGINGFACE_MODEL");
        let hf_model_prev = env::var_os("HF_MODEL");
        // Safety: test-only environment mutation guarded by a global mutex.
        unsafe {
            env::set_var("HOME", &home_str);
            env::set_var("USERPROFILE", &home_str);
            env::remove_var("CODEWHALE_HOME");
            env::remove_var("CODEWHALE_CONFIG_PATH");
            env::set_var("DEEPSEEK_CONFIG_PATH", &config_str);
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
            env::remove_var("KIMI_CODE_OAUTH_HOST");
            env::remove_var("KIMI_OAUTH_HOST");
            env::remove_var("SGLANG_API_KEY");
            env::remove_var("SGLANG_BASE_URL");
            env::remove_var("SGLANG_MODEL");
            env::remove_var("VLLM_API_KEY");
            env::remove_var("VLLM_BASE_URL");
            env::remove_var("VLLM_MODEL");
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
            kimi_code_oauth_host: kimi_code_oauth_host_prev,
            kimi_oauth_host: kimi_oauth_host_prev,
            sglang_api_key: sglang_api_key_prev,
            sglang_base_url: sglang_base_url_prev,
            sglang_model: sglang_model_prev,
            vllm_api_key: vllm_api_key_prev,
            vllm_base_url: vllm_base_url_prev,
            vllm_model: vllm_model_prev,
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
            Self::restore_var("KIMI_CODE_OAUTH_HOST", self.kimi_code_oauth_host.take());
            Self::restore_var("KIMI_OAUTH_HOST", self.kimi_oauth_host.take());
            Self::restore_var("SGLANG_API_KEY", self.sglang_api_key.take());
            Self::restore_var("SGLANG_BASE_URL", self.sglang_base_url.take());
            Self::restore_var("SGLANG_MODEL", self.sglang_model.take());
            Self::restore_var("VLLM_API_KEY", self.vllm_api_key.take());
            Self::restore_var("VLLM_BASE_URL", self.vllm_base_url.take());
            Self::restore_var("VLLM_MODEL", self.vllm_model.take());
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
fn max_subagents_defaults_to_default_limit() {
    assert_eq!(Config::default().max_subagents(), DEFAULT_MAX_SUBAGENTS);
    assert_eq!(DEFAULT_MAX_SUBAGENTS, 64);
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
fn launch_concurrency_honors_deprecated_interactive_max_launch_alias() {
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

#[test]
fn launch_concurrency_new_key_wins_over_deprecated_alias() {
    // When both keys are present the new `launch_concurrency` wins
    // deterministically, regardless of document order.
    let cfg: SubagentsConfig = toml::from_str("launch_concurrency = 3\ninteractive_max_launch = 7")
        .expect("parse both keys");
    assert_eq!(cfg.launch_concurrency, Some(3));
    assert_eq!(cfg.interactive_max_launch_legacy, Some(7));

    let config = Config {
        subagents: Some(cfg),
        ..Config::default()
    };
    assert_eq!(config.launch_concurrency(), 3);
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
    assert_eq!(
        config.subagent_heartbeat_timeout_secs_for_provider(ApiProvider::Zai),
        240
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
fn max_subagents_clamps_subagents_max_concurrent() {
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
    assert_eq!(
        Config::default().subagent_heartbeat_timeout_secs(),
        DEFAULT_SUBAGENT_HEARTBEAT_TIMEOUT_SECS
    );

    let zero = Config {
        subagents: Some(SubagentsConfig {
            heartbeat_timeout_secs: Some(0),
            ..SubagentsConfig::default()
        }),
        ..Config::default()
    };
    assert_eq!(
        zero.subagent_heartbeat_timeout_secs(),
        DEFAULT_SUBAGENT_HEARTBEAT_TIMEOUT_SECS
    );

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
        MIN_SUBAGENT_API_TIMEOUT_SECS + 30
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
fn save_api_key_onboarding_routes_openrouter_key_to_provider_table() -> Result<()> {
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
fn save_api_key_rejects_empty_input() {
    let _lock = lock_test_env();
    let err = save_api_key("   ").expect_err("empty should bail");
    assert!(
        err.to_string().contains("empty"),
        "expected error to mention empty, got: {err}"
    );
}

#[test]
fn saved_credential_describe_returns_config_file_path() {
    let cf = SavedCredential::ConfigFile(PathBuf::from("/tmp/x.toml"));
    assert_eq!(cf.describe(), "/tmp/x.toml");
}

/// #593: the dual-write outcome describes both targets so the onboarding toast
/// (`API key saved to {describe}`) tells the user the key landed in both the
/// selected durable secret store and the inspectable config file.
#[test]
fn saved_credential_describe_lists_both_targets_for_keyring_and_config() {
    let dual = SavedCredential::KeyringAndConfigFile {
        backend: "system keyring".to_string(),
        path: PathBuf::from("/tmp/x.toml"),
    };
    assert_eq!(
        dual.describe(),
        "secret store (system keyring) and /tmp/x.toml"
    );
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
fn provider_neutral_cli_key_wins_after_profile_provider_switch() -> Result<()> {
    let _lock = lock_test_env();
    let _source = EnvVarGuard::set("DEEPSEEK_API_KEY_SOURCE", "cli");
    let _cli_key = EnvVarGuard::set("CODEWHALE_CLI_API_KEY", "explicit-profile-key");
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
    Ok(())
}

#[test]
fn provider_neutral_cli_key_requires_dispatcher_source_marker() -> Result<()> {
    let _lock = lock_test_env();
    let _source = EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
    let _cli_key = EnvVarGuard::set("CODEWHALE_CLI_API_KEY", "untrusted-generic-key");
    let _anthropic_env = EnvVarGuard::remove("ANTHROPIC_API_KEY");
    let mut providers = ProvidersConfig::default();
    providers.anthropic.api_key = Some("saved-anthropic-key".to_string());
    let config = Config {
        provider: Some("anthropic".to_string()),
        providers: Some(providers),
        ..Default::default()
    };

    assert_eq!(config.deepseek_api_key()?, "saved-anthropic-key");
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
fn has_api_key_uses_active_provider_scoped_config_key() {
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

#[test]
fn has_api_key_uses_root_config_key_for_deepseek_variants() {
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

#[test]
fn save_api_key_for_openai_codex_refuses_config_storage() {
    let err = save_api_key_for(ApiProvider::OpenaiCodex, "codex-token")
        .expect_err("Codex OAuth tokens must not be persisted as provider API keys");

    let message = err.to_string();
    assert!(message.contains("OpenAI Codex uses OAuth"), "{message}");
    assert!(message.contains("codex login"), "{message}");
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
    let _source = EnvVarGuard::set("DEEPSEEK_API_KEY_SOURCE", "cli");
    let _cli_key = EnvVarGuard::set("CODEWHALE_CLI_API_KEY", "explicit-cli-key");

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

    config.provider = Some("anthropic".to_string());
    assert_eq!(
        config.deepseek_base_url(),
        "https://env-gateway.example.test/v1"
    );
    assert!(config.deepseek_api_key().is_err());

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

    Ok(())
}

#[test]
fn codewhale_config_path_env_wins_over_legacy_env() -> Result<()> {
    let _lock = lock_test_env();
    let prev_codewhale = env::var_os("CODEWHALE_CONFIG_PATH");
    let prev_deepseek = env::var_os("DEEPSEEK_CONFIG_PATH");
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

    unsafe {
        env::set_var("CODEWHALE_CONFIG_PATH", &preferred);
        env::set_var("DEEPSEEK_CONFIG_PATH", &legacy);
    }

    assert_eq!(env_config_path().unwrap(), preferred);

    unsafe {
        EnvGuard::restore_var("CODEWHALE_CONFIG_PATH", prev_codewhale);
        EnvGuard::restore_var("DEEPSEEK_CONFIG_PATH", prev_deepseek);
    }

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
fn test_load_falls_back_to_home_config_when_env_path_missing() -> Result<()> {
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
    assert_eq!(config.api_key.as_deref(), Some("home-key"));
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

    let saved = save_api_key("new-key")?;
    assert_eq!(saved, SavedCredential::ConfigFile(config_path.clone()));

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
    apply_env_overrides(&mut config);

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
    apply_env_overrides(&mut config);

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
fn normalize_model_for_provider_keeps_provider_remaps_when_case_is_preserved() {
    assert_eq!(
        normalize_model_for_provider(ApiProvider::Deepseek, "DeepSeek-V4-Pro").as_deref(),
        Some("DeepSeek-V4-Pro")
    );
    assert_eq!(
        normalize_model_for_provider(ApiProvider::NvidiaNim, "DeepSeek-V4-Pro").as_deref(),
        Some(DEFAULT_NVIDIA_NIM_MODEL)
    );
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
fn normalize_model_name_for_provider_maps_moonshot_aliases() {
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

#[test]
fn normalize_model_name_for_provider_maps_minimax_direct_aliases() {
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

#[test]
fn normalize_model_name_for_provider_maps_arcee_direct_aliases() {
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
fn model_completion_names_for_xiaomi_mimo_include_chat_models() {
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

#[test]
fn model_completion_names_for_deepseek_api_are_deduplicated_bare_ids() {
    assert_eq!(
        model_completion_names_for_provider(ApiProvider::Deepseek),
        vec!["deepseek-v4-pro", "deepseek-v4-flash"]
    );
}

#[test]
fn model_completion_names_for_together_include_provider_owned_models() {
    assert_eq!(
        model_completion_names_for_provider(ApiProvider::Together),
        vec![DEFAULT_TOGETHER_MODEL, DEFAULT_TOGETHER_FLASH_MODEL]
    );
}

#[test]
fn model_completion_names_for_wanjie_keep_legacy_default_and_v4_ids() {
    let models = model_completion_names_for_provider(ApiProvider::WanjieArk);

    assert_eq!(models.first().copied(), Some(DEFAULT_WANJIE_ARK_MODEL));
    assert!(models.contains(&"deepseek-v4-pro"));
    assert!(models.contains(&"deepseek-v4-flash"));
}

#[test]
fn model_completion_names_for_ollama_do_not_promote_static_remote_models() {
    let models = model_completion_names_for_provider(ApiProvider::Ollama);

    assert!(models.is_empty());
}

#[test]
fn model_completion_names_for_openrouter_include_recent_large_models() {
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

#[test]
fn model_completion_names_for_moonshot_uses_latest_platform_model() {
    assert_eq!(
        model_completion_names_for_provider(ApiProvider::Moonshot),
        vec![DEFAULT_MOONSHOT_MODEL]
    );
}

#[test]
fn model_completion_names_for_zai_lists_default_5_1_and_turbo() {
    let models = model_completion_names_for_provider(ApiProvider::Zai);

    // GLM-5.2 is the default and must be first; GLM-5.1 stays available,
    // and GLM-5-Turbo is the faster sub-agent sibling.
    assert_eq!(models.first().copied(), Some(DEFAULT_ZAI_MODEL));
    assert_eq!(DEFAULT_ZAI_MODEL, ZAI_GLM_5_2_MODEL);
    assert!(models.contains(&ZAI_GLM_5_1_MODEL));
    assert!(models.contains(&ZAI_GLM_5_TURBO_MODEL));
    // No accidental duplicate entries.
    let mut sorted = models.to_vec();
    sorted.sort_unstable();
    let mut deduped = sorted.clone();
    deduped.dedup();
    assert_eq!(sorted, deduped);
}

#[test]
fn normalize_model_name_for_zai_canonicalizes_current_glm_models() {
    for (alias, expected) in [
        ("glm-5.1", ZAI_GLM_5_1_MODEL),
        ("glm-5-1", ZAI_GLM_5_1_MODEL),
        ("glm-5.2", DEFAULT_ZAI_MODEL),
        ("zai-glm-5-2", DEFAULT_ZAI_MODEL),
        ("glm-5-turbo", ZAI_GLM_5_TURBO_MODEL),
        ("zai-glm-5-turbo", ZAI_GLM_5_TURBO_MODEL),
    ] {
        assert_eq!(
            normalize_model_name_for_provider(ApiProvider::Zai, alias).as_deref(),
            Some(expected)
        );
    }
    assert_eq!(
        normalize_model_name_for_provider(ApiProvider::Zai, "glm-next-preview").as_deref(),
        Some("glm-next-preview")
    );
}

#[test]
fn model_completion_names_for_minimax_include_direct_chat_models() {
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

#[test]
fn model_completion_names_for_minimax_anthropic_include_target_models() {
    let models = model_completion_names_for_provider(ApiProvider::MinimaxAnthropic);

    assert!(models.contains(&DEFAULT_MINIMAX_MODEL));
    assert!(models.contains(&MINIMAX_M2_7_MODEL));
}

#[test]
fn model_completion_names_for_sakana_include_fugu_models() {
    assert_eq!(
        model_completion_names_for_provider(ApiProvider::Sakana),
        vec![DEFAULT_SAKANA_MODEL, SAKANA_FUGU_ULTRA_MODEL]
    );
}

#[test]
fn normalize_model_name_rejects_invalid_or_non_deepseek_ids() {
    assert!(normalize_model_name("qwen3-coder").is_none());
    assert!(normalize_model_name("codewhale v4").is_none());
    assert!(normalize_model_name("").is_none());
}

#[test]
fn normalize_model_name_accepts_provider_prefixed_deepseek_ids() {
    assert_eq!(
        normalize_model_name("accounts/fireworks/models/deepseek-v4-flash").as_deref(),
        Some("accounts/fireworks/models/deepseek-v4-flash")
    );
    assert_eq!(
        normalize_model_name("provider/deepseek-ai/deepseek-v4-pro").as_deref(),
        Some("provider/deepseek-ai/deepseek-v4-pro")
    );
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
fn profile_without_context_does_not_disable_base_context() {
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
fn project_context_pack_defaults_on_and_can_be_disabled() {
    let mut config = Config::default();
    assert!(config.project_context_pack_enabled());

    config.context.project_pack = Some(false);
    assert!(!config.project_context_pack_enabled());
}

#[test]
fn validate_accepts_future_deepseek_model_id() -> Result<()> {
    let config = Config {
        default_text_model: Some("deepseek-v4".to_string()),
        ..Default::default()
    };
    config.validate()?;
    Ok(())
}

#[test]
fn validate_accepts_auto_default_text_model() -> Result<()> {
    let config = Config {
        default_text_model: Some("auto".to_string()),
        ..Default::default()
    };
    config.validate()?;
    assert_eq!(config.default_model(), "auto");
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
fn nvidia_nim_provider_uses_nim_defaults() -> Result<()> {
    let config = Config {
        provider: Some("nvidia-nim".to_string()),
        ..Default::default()
    };

    config.validate()?;
    assert_eq!(config.api_provider(), ApiProvider::NvidiaNim);
    assert_eq!(config.default_model(), DEFAULT_NVIDIA_NIM_MODEL);
    assert_eq!(config.deepseek_base_url(), DEFAULT_NVIDIA_NIM_BASE_URL);
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
fn nvidia_nim_provider_normalizes_deepseek_v4_flash_alias() -> Result<()> {
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
    Ok(())
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
fn nvidia_nim_env_accepts_short_nim_base_url_alias() -> Result<()> {
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
    assert_eq!(config.default_model(), DEFAULT_OPENAI_MODEL);
    assert_eq!(config.deepseek_base_url(), DEFAULT_OPENAI_BASE_URL);
    Ok(())
}

#[test]
fn openai_codex_default_model_falls_back_to_codex_model() {
    // The Codex Responses backend only accepts its own model family, and a
    // global `default_text_model` is validated to DeepSeek IDs (or "auto"),
    // so with the Codex provider it must resolve to the Codex default
    // instead of leaking a DeepSeek id the backend rejects.
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
fn insecure_skip_tls_verify_is_scoped_to_active_provider() {
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

#[test]
fn insecure_skip_tls_verify_reads_active_provider_table() {
    let mut providers = ProvidersConfig::default();
    providers.openai.insecure_skip_tls_verify = Some(true);
    let config = Config {
        provider: Some("openai".to_string()),
        providers: Some(providers),
        ..Default::default()
    };

    assert!(config.insecure_skip_tls_verify());
}

#[test]
fn xiaomi_mimo_provider_uses_documented_defaults() -> Result<()> {
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
    Ok(())
}

#[test]
fn xiaomi_mimo_provider_ignores_non_mimo_root_default_model() -> Result<()> {
    let config = Config {
        provider: Some("xiaomi-mimo".to_string()),
        default_text_model: Some(DEFAULT_OPENROUTER_MODEL.to_string()),
        ..Default::default()
    };

    config.validate()?;
    assert_eq!(config.api_provider(), ApiProvider::XiaomiMimo);
    assert_eq!(config.default_model(), DEFAULT_XIAOMI_MIMO_MODEL);
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
fn xiaomi_mimo_token_plan_mode_accepts_region_aliases() -> Result<()> {
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
    Ok(())
}

#[test]
fn xiaomi_mimo_unknown_mode_stays_on_token_plan_endpoint() -> Result<()> {
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
fn moonshot_kimi_oauth_reads_kimi_code_home_credential() -> Result<()> {
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

    let expires_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
        + 3600.0;
    let credential = json!({
        "access_token": "fresh-kimi-code-oauth-token",
        "refresh_token": "refresh-token",
        "expires_at": expires_at,
        "scope": "openid profile email",
        "token_type": "Bearer",
    });
    fs::write(
        credential_dir.join(KIMI_CODE_CREDENTIAL_FILE),
        serde_json::to_string(&credential)?,
    )?;

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
    assert_eq!(config.deepseek_api_key()?, "fresh-kimi-code-oauth-token");
    assert!(has_api_key_for(&config, ApiProvider::Moonshot));
    Ok(())
}

#[test]
fn moonshot_kimi_oauth_falls_back_to_legacy_share_dir_credential() -> Result<()> {
    let _lock = lock_test_env();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "codewhale-tui-kimi-oauth-key-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&temp_root)?;
    let _guard = EnvGuard::new(&temp_root);

    let kimi_share_dir = temp_root.join(".kimi");
    let credential_dir = kimi_share_dir.join("credentials");
    fs::create_dir_all(&credential_dir)?;
    unsafe { env::set_var("KIMI_SHARE_DIR", &kimi_share_dir) };

    let expires_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
        + 3600.0;
    let credential = json!({
        "access_token": "fresh-oauth-token",
        "refresh_token": "refresh-token",
        "expires_at": expires_at,
        "scope": "openid profile email",
        "token_type": "Bearer",
    });
    fs::write(
        credential_dir.join(KIMI_CODE_CREDENTIAL_FILE),
        serde_json::to_string(&credential)?,
    )?;

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
    assert_eq!(config.deepseek_api_key()?, "fresh-oauth-token");
    assert!(has_api_key_for(&config, ApiProvider::Moonshot));
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
fn has_api_key_for_uses_deepseek_cn_provider_table() -> Result<()> {
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
fn has_api_key_for_accepts_xai_oauth_without_masking_api_keys() -> Result<()> {
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

    let mut providers = ProvidersConfig::default();
    providers.xai.api_key = Some("xai-api-key".to_string());
    let api_key_config = Config {
        providers: Some(providers),
        ..Config::default()
    };
    assert!(has_api_key_for(&api_key_config, ApiProvider::Xai));

    fs::write(&auth_path, "{}")?;
    assert!(has_api_key_for(&Config::default(), ApiProvider::Xai));
    fs::remove_dir_all(temp_root)?;
    Ok(())
}

#[test]
fn has_api_key_for_uses_root_config_key_for_deepseek_variants() {
    let config = Config {
        api_key: Some("root-config-key".to_string()),
        ..Config::default()
    };

    assert!(has_api_key_for(&config, ApiProvider::Deepseek));
    assert!(has_api_key_for(&config, ApiProvider::DeepseekCN));
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

    let path = save_api_key_for(ApiProvider::Openrouter, "or-saved-key")?;
    assert_eq!(path, config_path);
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
    assert_eq!(novita_path, path);
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
        assert_eq!(save_api_key_for(provider, key)?, path);
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
fn provider_capability_deepseek_v4_pro_has_1m_window_and_thinking() {
    let cap = provider_capability(ApiProvider::Deepseek, "deepseek-v4-pro");
    assert_eq!(
        cap.context_window,
        crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
    );
    assert_eq!(cap.max_output, 384_000);
    assert!(cap.thinking_supported);
    assert!(cap.cache_telemetry_supported);
    assert_eq!(
        cap.request_payload_mode,
        RequestPayloadMode::ChatCompletions
    );
}

#[test]
fn provider_capability_deepseek_anthropic_uses_messages_payload() {
    let cap = provider_capability(
        ApiProvider::DeepseekAnthropic,
        DEFAULT_DEEPSEEK_ANTHROPIC_MODEL,
    );
    assert_eq!(
        cap.context_window,
        crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
    );
    assert_eq!(cap.max_output, 384_000);
    assert!(cap.thinking_supported);
    assert!(!cap.cache_telemetry_supported);
    assert_eq!(
        cap.request_payload_mode,
        RequestPayloadMode::AnthropicMessages
    );
    assert!(cap.alias_deprecation.is_none());
}

#[test]
fn provider_capability_openmodel_uses_messages_payload() {
    let cap = provider_capability(ApiProvider::Openmodel, DEFAULT_OPENMODEL_MODEL);
    assert_eq!(cap.resolved_model, DEFAULT_OPENMODEL_MODEL);
    assert_eq!(
        cap.context_window,
        crate::models::context_window_for_model(DEFAULT_OPENMODEL_MODEL).unwrap_or(200_000)
    );
    assert_eq!(
        cap.max_output,
        crate::models::max_output_tokens_for_model(DEFAULT_OPENMODEL_MODEL).unwrap_or(64_000)
    );
    assert!(!cap.cache_telemetry_supported);
    assert_eq!(
        cap.request_payload_mode,
        RequestPayloadMode::AnthropicMessages
    );
    assert!(provider_passes_model_through(ApiProvider::Openmodel));
}

#[test]
fn provider_capability_deepseek_v4_flash_has_1m_window_and_thinking() {
    let cap = provider_capability(ApiProvider::Deepseek, "deepseek-v4-flash");
    assert_eq!(
        cap.context_window,
        crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
    );
    assert_eq!(cap.max_output, 384_000);
    assert!(cap.thinking_supported);
    assert!(cap.cache_telemetry_supported);
}

#[test]
fn provider_capability_deepseek_chat_alias_has_v4_flash_caps_and_metadata() {
    let cap = provider_capability(ApiProvider::Deepseek, "deepseek-chat");
    assert_eq!(
        cap.context_window,
        crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
    );
    assert_eq!(cap.max_output, 384_000);
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

#[test]
fn provider_capability_deepseek_reasoner_alias_has_v4_flash_caps_and_metadata() {
    let cap = provider_capability(ApiProvider::Deepseek, "deepseek-reasoner");
    assert_eq!(
        cap.context_window,
        crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
    );
    assert_eq!(cap.max_output, 384_000);
    assert!(cap.thinking_supported);
    assert!(cap.cache_telemetry_supported);

    let deprecation = cap
        .alias_deprecation
        .as_ref()
        .expect("alias deprecation metadata");
    assert_eq!(deprecation.alias, "deepseek-reasoner");
    assert_eq!(deprecation.replacement, "deepseek-v4-flash");
}

#[test]
fn provider_capability_deepseek_v4_flash_has_no_alias_deprecation() {
    let cap = provider_capability(ApiProvider::Deepseek, "deepseek-v4-flash");
    assert!(cap.alias_deprecation.is_none());
}

#[test]
fn provider_capability_nvidia_nim_v4_pro_maps_correctly() {
    let cap = provider_capability(ApiProvider::NvidiaNim, DEFAULT_NVIDIA_NIM_MODEL);
    assert_eq!(
        cap.context_window,
        crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
    );
    assert_eq!(cap.max_output, 384_000);
    assert!(cap.thinking_supported);
    assert!(cap.cache_telemetry_supported);
    assert_eq!(
        cap.request_payload_mode,
        RequestPayloadMode::ChatCompletions
    );
}

#[test]
fn provider_capability_nvidia_nim_v4_flash_maps_correctly() {
    let cap = provider_capability(ApiProvider::NvidiaNim, DEFAULT_NVIDIA_NIM_FLASH_MODEL);
    assert_eq!(
        cap.context_window,
        crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
    );
    assert_eq!(cap.max_output, 384_000);
    assert!(cap.thinking_supported);
    assert!(cap.cache_telemetry_supported);
}

#[test]
fn provider_capability_openrouter_v4_pro_has_thinking_no_cache() {
    let cap = provider_capability(ApiProvider::Openrouter, DEFAULT_OPENROUTER_MODEL);
    assert_eq!(
        cap.context_window,
        crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
    );
    assert_eq!(cap.max_output, 384_000);
    assert!(cap.thinking_supported);
    // OpenRouter does not return DeepSeek prompt-cache telemetry.
    assert!(!cap.cache_telemetry_supported);
    assert_eq!(
        cap.request_payload_mode,
        RequestPayloadMode::ChatCompletions
    );
}

#[test]
fn provider_capability_openai_codex_uses_responses_payload() {
    let cap = provider_capability(ApiProvider::OpenaiCodex, DEFAULT_OPENAI_CODEX_MODEL);
    assert_eq!(cap.provider, ApiProvider::OpenaiCodex);
    assert_eq!(cap.resolved_model, DEFAULT_OPENAI_CODEX_MODEL);
    assert_eq!(
        cap.context_window,
        OPENAI_CODEX_EFFECTIVE_CONTEXT_WINDOW_TOKENS
    );
    assert_eq!(cap.max_output, 4096);
    assert!(cap.thinking_supported);
    assert!(!cap.cache_telemetry_supported);
    assert_eq!(cap.request_payload_mode, RequestPayloadMode::Responses);
}

#[test]
fn invalid_provider_auth_source_is_not_explicit_configuration() {
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
        assert_eq!(cap.max_output, expected_output);
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
    assert_eq!(thinking_cap.max_output, 262_144);
    assert!(thinking_cap.thinking_supported);
    assert!(!thinking_cap.cache_telemetry_supported);
    assert_eq!(
        thinking_cap.request_payload_mode,
        RequestPayloadMode::ChatCompletions
    );

    let preview = provider_capability(ApiProvider::Arcee, ARCEE_TRINITY_LARGE_PREVIEW_MODEL);
    assert_eq!(preview.context_window, 262_144);
    assert_eq!(preview.max_output, 4096);
    assert!(!preview.thinking_supported);

    let mini = provider_capability(ApiProvider::Arcee, ARCEE_TRINITY_MINI_MODEL);
    assert_eq!(mini.context_window, 128_000);
    // ProviderCapability carries a mandatory request fallback; model metadata
    // itself deliberately keeps Trinity Mini's upstream output limit unknown.
    assert_eq!(mini.max_output, 4096);
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
fn provider_capability_marks_exact_inkling_route_as_reasoning() {
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

#[test]
fn provider_capability_xiaomi_mimo_has_thinking_no_cache() {
    let cap = provider_capability(ApiProvider::XiaomiMimo, DEFAULT_XIAOMI_MIMO_MODEL);
    assert_eq!(cap.context_window, 1_000_000);
    assert_eq!(cap.max_output, 131_072);
    assert!(cap.thinking_supported);
    assert!(!cap.cache_telemetry_supported);
    assert_eq!(
        cap.request_payload_mode,
        RequestPayloadMode::ChatCompletions
    );

    let omni = provider_capability(ApiProvider::XiaomiMimo, XIAOMI_MIMO_V2_5_OMNI_MODEL);
    assert_eq!(omni.context_window, 1_000_000);
    assert_eq!(omni.max_output, 131_072);
    assert!(omni.thinking_supported);
    assert!(!omni.cache_telemetry_supported);
}

#[test]
fn provider_capability_novita_v4_pro_has_thinking_no_cache() {
    let cap = provider_capability(ApiProvider::Novita, DEFAULT_NOVITA_MODEL);
    assert_eq!(
        cap.context_window,
        crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
    );
    assert_eq!(cap.max_output, 384_000);
    assert!(cap.thinking_supported);
    assert!(!cap.cache_telemetry_supported);
}

#[test]
fn provider_capability_fireworks_v4_pro_has_thinking_no_cache() {
    let cap = provider_capability(ApiProvider::Fireworks, DEFAULT_FIREWORKS_MODEL);
    assert_eq!(
        cap.context_window,
        crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
    );
    assert_eq!(cap.max_output, 384_000);
    assert!(cap.thinking_supported);
    assert!(!cap.cache_telemetry_supported);
}

#[test]
fn provider_capability_siliconflow_v4_pro_has_thinking_no_cache() {
    let cap = provider_capability(ApiProvider::Siliconflow, DEFAULT_SILICONFLOW_MODEL);
    assert_eq!(
        cap.context_window,
        crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
    );
    assert_eq!(cap.max_output, 384_000);
    assert!(cap.thinking_supported);
    assert!(!cap.cache_telemetry_supported);
    assert_eq!(
        cap.request_payload_mode,
        RequestPayloadMode::ChatCompletions
    );
}

#[test]
fn provider_capability_sglang_v4_pro_has_thinking_no_cache() {
    let cap = provider_capability(ApiProvider::Sglang, DEFAULT_SGLANG_MODEL);
    assert_eq!(
        cap.context_window,
        crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
    );
    assert_eq!(cap.max_output, 384_000);
    assert!(cap.thinking_supported);
    assert!(!cap.cache_telemetry_supported);
}

#[test]
fn provider_capability_openai_custom_model_is_chat_completions_without_thinking() {
    let cap = provider_capability(ApiProvider::Openai, "glm-5");
    assert_eq!(
        cap.context_window,
        crate::models::LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS
    );
    assert_eq!(cap.max_output, 4096);
    assert!(!cap.thinking_supported);
    assert!(!cap.cache_telemetry_supported);
    assert_eq!(
        cap.request_payload_mode,
        RequestPayloadMode::ChatCompletions
    );
}

#[test]
fn provider_capability_atlascloud_v4_model_resolves_model_metadata() {
    // #3023: Atlascloud uses the generic model-based path, so its default
    // DeepSeek V4 model resolves the real V4 metadata instead of the old
    // hardcoded legacy floor.
    let cap = provider_capability(ApiProvider::Atlascloud, "deepseek-ai/deepseek-v4-flash");
    assert_eq!(
        cap.context_window,
        crate::models::DEEPSEEK_V4_CONTEXT_WINDOW_TOKENS
    );
    assert_eq!(cap.max_output, 384_000);
    assert!(cap.thinking_supported);
    assert!(!cap.cache_telemetry_supported);
    assert_eq!(
        cap.request_payload_mode,
        RequestPayloadMode::ChatCompletions
    );
}

#[test]
fn provider_capability_moonshot_default_model_resolves_kimi_metadata() {
    let cap = provider_capability(ApiProvider::Moonshot, DEFAULT_MOONSHOT_MODEL);
    assert_eq!(cap.context_window, 262_144);
    assert_eq!(cap.max_output, 32_768);
    assert!(cap.thinking_supported);
    assert!(!cap.cache_telemetry_supported);
    assert_eq!(
        cap.request_payload_mode,
        RequestPayloadMode::ChatCompletions
    );
}

#[test]
fn provider_capability_zai_defaults_to_5_2_and_tracks_5_1_and_turbo() {
    // GLM-5.2 is now the default direct Z.AI model (1M context window).
    let default = provider_capability(ApiProvider::Zai, DEFAULT_ZAI_MODEL);
    assert_eq!(default.resolved_model, DEFAULT_ZAI_MODEL);
    assert_eq!(default.resolved_model, ZAI_GLM_5_2_MODEL);
    assert_eq!(default.context_window, 1_000_000);
    assert_eq!(default.max_output, 131_072);
    assert!(default.thinking_supported);
    assert!(!default.cache_telemetry_supported);

    // GLM-5.1 remains available as an explicit model (smaller window).
    let v51 = provider_capability(ApiProvider::Zai, ZAI_GLM_5_1_MODEL);
    assert_eq!(v51.resolved_model, ZAI_GLM_5_1_MODEL);
    assert_eq!(v51.context_window, 202_752);
    assert_eq!(v51.max_output, 131_072);
    assert!(v51.thinking_supported);

    // GLM-5-Turbo is the faster sub-agent sibling.
    let turbo = provider_capability(ApiProvider::Zai, ZAI_GLM_5_TURBO_MODEL);
    assert_eq!(turbo.resolved_model, ZAI_GLM_5_TURBO_MODEL);
}

#[test]
fn provider_capability_minimax_direct_models_use_api_docs_shape() {
    let m3 = provider_capability(ApiProvider::Minimax, DEFAULT_MINIMAX_MODEL);
    assert_eq!(m3.context_window, 1_000_000);
    assert_eq!(m3.max_output, 524_288);
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
fn provider_capability_minimax_anthropic_uses_messages_shape() {
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

#[test]
fn provider_capability_wanjie_ark_reasoner_has_thinking_no_cache() {
    let cap = provider_capability(ApiProvider::WanjieArk, DEFAULT_WANJIE_ARK_MODEL);
    assert_eq!(
        cap.context_window,
        crate::models::LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS
    );
    assert_eq!(cap.max_output, 4096);
    assert!(cap.thinking_supported);
    assert!(!cap.cache_telemetry_supported);
    assert_eq!(
        cap.request_payload_mode,
        RequestPayloadMode::ChatCompletions
    );
}

#[test]
fn provider_capability_ollama_deepseek_tag_uses_deepseek_heuristic() {
    // #3023: known model families resolve through models.rs lookups even
    // on Ollama — a legacy DeepSeek tag gets the 128K heuristic window.
    let cap = provider_capability(ApiProvider::Ollama, "deepseek-v3.1:671b");
    assert_eq!(
        cap.context_window,
        crate::models::LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS
    );
    assert_eq!(cap.max_output, 4096);
    assert!(!cap.thinking_supported);
    assert!(!cap.cache_telemetry_supported);
    assert_eq!(
        cap.request_payload_mode,
        RequestPayloadMode::ChatCompletions
    );
}

#[test]
fn provider_capability_ollama_unknown_model_falls_back_to_8192() {
    let cap = provider_capability(ApiProvider::Ollama, "llama3.2:3b");
    assert_eq!(cap.context_window, 8192);
    assert_eq!(cap.max_output, 4096);
    assert!(!cap.thinking_supported);
    assert!(!cap.cache_telemetry_supported);
    assert_eq!(
        cap.request_payload_mode,
        RequestPayloadMode::ChatCompletions
    );
}

#[test]
fn provider_capability_non_v4_model_has_smaller_window() {
    let cap = provider_capability(ApiProvider::Deepseek, "deepseek-coder");
    assert_eq!(
        cap.context_window,
        crate::models::LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS
    );
    assert_eq!(cap.max_output, 4096);
    assert!(!cap.thinking_supported);
}

#[test]
fn provider_capability_roundtrip_serialization() {
    let cap = provider_capability(ApiProvider::Deepseek, "deepseek-v4-pro");
    let json = serde_json::to_value(&cap).unwrap();
    let deserialized: ProviderCapability = serde_json::from_value(json).unwrap();
    assert_eq!(cap, deserialized);
}

#[test]
fn status_item_balance_available_only_for_deepseek_providers() {
    // Balance item should only be offered for DeepSeek / DeepSeekCN.
    assert!(StatusItem::Balance.is_available_for(ApiProvider::Deepseek));
    assert!(StatusItem::Balance.is_available_for(ApiProvider::DeepseekCN));
    // Sanity: all other known providers should hide the Balance toggle.
    assert!(!StatusItem::Balance.is_available_for(ApiProvider::Openrouter));
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
fn status_items_deser_ignores_unknown_variants() {
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

#[test]
fn status_items_deser_allows_missing_field() {
    let toml_str = r#"
        locale = "zh-Hans"
        mouse_capture = false
    "#;
    let tui: TuiConfig = toml::from_str(toml_str).expect("missing status_items should parse");
    assert_eq!(tui.status_items, None);
}

#[test]
fn huggingface_provider_aliases_parse() {
    for alias in ["huggingface", "hugging-face", "hugging_face", "hf"] {
        assert_eq!(ApiProvider::parse(alias), Some(ApiProvider::Huggingface));
    }
}

#[test]
fn invalid_provider_error_lists_huggingface() {
    let config = Config {
        provider: Some("not-a-provider".to_string()),
        ..Default::default()
    };
    let err = config.validate().expect_err("unknown provider should fail");
    let message = err.to_string();
    assert!(message.contains("Invalid provider 'not-a-provider'"));
    assert!(message.contains("huggingface"));
}

#[test]
fn huggingface_provider_uses_direct_defaults() -> Result<()> {
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
fn notifications_parse_custom_completion_sound_file() {
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
fn custom_provider_kind_only_accepts_openai_compatible() {
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

#[test]
fn custom_provider_base_url_and_model_resolve_from_named_table() {
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
        }
    );
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
fn provider_auth_mode_save_uses_requested_path_and_preserves_comments() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "# keep this operator note\n[providers.xai]\nmodel = \"grok-code-fast-1\" # keep model note\n",
    )
    .expect("seed config");

    let saved = save_provider_auth_mode_for_at(ApiProvider::Xai, "oauth", Some(&path))
        .expect("save auth mode");

    assert_eq!(saved, path);
    let contents = std::fs::read_to_string(&saved).expect("read config");
    assert!(contents.contains("# keep this operator note"));
    assert!(contents.contains("model = \"grok-code-fast-1\" # keep model note"));
    assert!(contents.contains("auth_mode = \"oauth\""));
}
