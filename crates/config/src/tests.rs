use super::*;
use std::env;
use std::ffi::OsString;
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[test]
fn network_policy_toml_deserializes_proxy_hosts() {
    let policy: NetworkPolicyToml = toml::from_str(
        r#"
        default = "allow"
        proxy = ["github.com", ".githubusercontent.com"]
        "#,
    )
    .expect("network policy toml");

    assert_eq!(policy.default, "allow");
    assert_eq!(policy.proxy, ["github.com", ".githubusercontent.com"]);
    assert!(policy.audit);
}

#[test]
fn verifier_config_defaults_to_hunt_verdict_policy() {
    let config: ConfigToml = toml::from_str(
        r#"
        [verifier]
        enabled = true
        "#,
    )
    .expect("verifier config toml");

    let verifier = config.verifier.expect("verifier table");
    assert!(verifier.enabled);
    assert_eq!(verifier.verdict_policy, VerifierVerdictPolicy::Hunt);
}

#[test]
fn verifier_config_rejects_unknown_verdict_policy() {
    let err = toml::from_str::<ConfigToml>(
        r#"
        [verifier]
        verdict_policy = "strict"
        "#,
    )
    .expect_err("only the shipped hunt policy should parse");

    assert!(
        err.message().contains("unknown variant"),
        "unexpected error: {err}"
    );
}

#[test]
fn permissions_toml_deserializes_typed_ask_rules() {
    let permissions: PermissionsToml = toml::from_str(
        r#"
        [[rules]]
        tool = "exec_shell"
        command = "cargo test"

        [[rules]]
        tool = "read_file"
        path = "secrets/api_key.txt"
        "#,
    )
    .expect("permissions toml");

    assert_eq!(
        permissions.rules,
        vec![
            ToolAskRule::exec_shell("cargo test"),
            ToolAskRule::file_path("read_file", "secrets/api_key.txt"),
        ]
    );
}

#[test]
fn permissions_toml_rejects_unknown_decision_field() {
    // `decision` is NOT a valid field — `deny_unknown_fields` still active.
    let err = toml::from_str::<PermissionsToml>(
        r#"
        [[rules]]
        tool = "exec_shell"
        decision = "allow"
        command = "cargo test"
        "#,
    )
    .expect_err("permissions.toml should reject unknown 'decision' field");

    assert!(err.message().contains("unknown field"));
}

#[test]
fn permissions_toml_deserializes_action_deny_and_allow() {
    let permissions: PermissionsToml = toml::from_str(
        r#"
        [[rules]]
        tool = "exec_shell"
        command = "sed"
        action = "deny"

        [[rules]]
        tool = "exec_shell"
        command = "git status"
        action = "allow"

        [[rules]]
        tool = "exec_shell"
        command = "cargo test"
        "#,
    )
    .expect("permissions toml with actions");

    assert_eq!(permissions.rules.len(), 3);
    assert_eq!(
        permissions.rules[0].action,
        codewhale_execpolicy::PermissionAction::Deny
    );
    assert_eq!(
        permissions.rules[1].action,
        codewhale_execpolicy::PermissionAction::Allow
    );
    assert_eq!(
        permissions.rules[2].action,
        codewhale_execpolicy::PermissionAction::Ask
    ); // default
}

#[test]
fn permissions_ruleset_populates_denied_and_trusted_prefixes() {
    let permissions: PermissionsToml = toml::from_str(
        r#"
        [[rules]]
        tool = "exec_shell"
        command = "sed"
        action = "deny"

        [[rules]]
        tool = "exec_shell"
        command = "awk"
        action = "deny"

        [[rules]]
        tool = "exec_shell"
        command = "git status"
        action = "allow"

        [[rules]]
        tool = "exec_shell"
        command = "cargo test"
        action = "ask"
        "#,
    )
    .unwrap();

    let ruleset = permissions.ruleset();

    // All four rules kept as ask_rules for path-based / tool-only matching
    assert_eq!(ruleset.ask_rules.len(), 4);
    // deny rules promoted to denied_prefixes
    assert!(ruleset.denied_prefixes.contains(&"sed".to_string()));
    assert!(ruleset.denied_prefixes.contains(&"awk".to_string()));
    // allow rule promoted to trusted_prefixes
    assert!(ruleset.trusted_prefixes.contains(&"git status".to_string()));
    // ask rule NOT in trusted/denied prefixes
    assert!(!ruleset.trusted_prefixes.contains(&"cargo test".to_string()));
    assert!(!ruleset.denied_prefixes.contains(&"cargo test".to_string()));
}

#[test]
fn permissions_ruleset_deny_without_command_stays_in_ask_rules() {
    // Tool-only deny (no command) can't be promoted to denied_prefixes.
    let permissions: PermissionsToml = toml::from_str(
        r#"
        [[rules]]
        tool = "exec_shell"
        action = "deny"
        "#,
    )
    .unwrap();

    let ruleset = permissions.ruleset();
    assert_eq!(ruleset.ask_rules.len(), 1);
    assert_eq!(
        ruleset.ask_rules[0].action,
        codewhale_execpolicy::PermissionAction::Deny
    );
    // No command → nothing to promote to denied_prefixes
    assert!(ruleset.denied_prefixes.is_empty());
}

#[test]
fn permissions_ruleset_empty_rules_produces_empty_ruleset() {
    let permissions = PermissionsToml::default();
    let ruleset = permissions.ruleset();
    assert!(ruleset.trusted_prefixes.is_empty());
    assert!(ruleset.denied_prefixes.is_empty());
    assert!(ruleset.ask_rules.is_empty());
}

#[test]
fn permissions_ruleset_mixed_actions_all_coexist() {
    let permissions: PermissionsToml = toml::from_str(
        r#"
        [[rules]]
        tool = "exec_shell"
        command = "rm -rf"
        action = "deny"

        [[rules]]
        tool = "exec_shell"
        command = "git status"
        action = "allow"

        [[rules]]
        tool = "exec_shell"
        command = "npm test"
        action = "ask"

        [[rules]]
        tool = "read_file"
        path = "Cargo.toml"
        action = "allow"

        [[rules]]
        tool = "write_file"
        path = "src/secrets.rs"
        action = "deny"
        "#,
    )
    .unwrap();

    let ruleset = permissions.ruleset();

    // All 5 rules in ask_rules
    assert_eq!(ruleset.ask_rules.len(), 5);

    // Command-based deny → denied_prefixes
    assert!(ruleset.denied_prefixes.contains(&"rm -rf".to_string()));
    assert_eq!(ruleset.denied_prefixes.len(), 1); // only rm -rf has a command

    // Command-based allow → trusted_prefixes
    assert!(ruleset.trusted_prefixes.contains(&"git status".to_string()));
    assert_eq!(ruleset.trusted_prefixes.len(), 1); // only git status has a command

    // Path-based rules stay in ask_rules but not in prefixes
    let path_deny = ruleset
        .ask_rules
        .iter()
        .find(|r| r.path.as_deref() == Some("src/secrets.rs"))
        .unwrap();
    assert_eq!(
        path_deny.action,
        codewhale_execpolicy::PermissionAction::Deny
    );

    let path_allow = ruleset
        .ask_rules
        .iter()
        .find(|r| r.path.as_deref() == Some("Cargo.toml"))
        .unwrap();
    assert_eq!(
        path_allow.action,
        codewhale_execpolicy::PermissionAction::Allow
    );
}

#[test]
fn provider_command_auth_source_deserializes() {
    let config: ConfigToml = toml::from_str(
        r#"
        [providers.deepseek.auth]
        source = "command"
        command = ["keepassxc-cli", "show", "CodeWhale/DeepSeek", "--attribute", "password"]
        timeout_ms = 2000
        "#,
    )
    .expect("config toml");

    let auth = config
        .providers
        .deepseek
        .auth
        .expect("provider auth source");
    assert_eq!(auth.source, AuthSourceKind::Command);
    assert_eq!(auth.source_class(), "command");
    assert_eq!(auth.command[0], "keepassxc-cli");
    assert_eq!(auth.timeout_ms, Some(2000));
    auth.validate().expect("valid command auth source");
}

#[test]
fn provider_secret_auth_source_deserializes() {
    let config: ConfigToml = toml::from_str(
        r#"
        [providers.openai.auth]
        source = "secret"
        secret_id = "codewhale/openai"
        "#,
    )
    .expect("config toml");

    let auth = config.providers.openai.auth.expect("provider auth source");
    assert_eq!(auth.source, AuthSourceKind::Secret);
    assert_eq!(auth.source_class(), "secret");
    assert_eq!(auth.secret_id.as_deref(), Some("codewhale/openai"));
    auth.validate().expect("valid secret auth source");
}

#[test]
fn provider_auth_source_rejects_empty_command() {
    let config: ConfigToml = toml::from_str(
        r#"
        [providers.deepseek.auth]
        source = "command"
        command = []
        "#,
    )
    .expect("config toml");

    let auth = config
        .providers
        .deepseek
        .auth
        .expect("provider auth source");
    let err = auth.validate().expect_err("empty command must be invalid");
    assert!(err.to_string().contains("command must include"));
}

#[test]
fn hotbar_hidden_when_config_is_absent() {
    // #3807: an absent `hotbar` key resolves to no bindings, so the Hotbar is
    // hidden until the user opts in. The default slots are still available
    // explicitly via `default_hotbar_bindings_toml()` (what `/hotbar on` writes).
    let config = ConfigToml::default();

    let resolved = config.resolve_hotbar_bindings(&DEFAULT_HOTBAR_ACTIONS);

    assert_eq!(resolved.warnings, Vec::new());
    assert!(
        resolved.bindings.is_empty(),
        "fresh config must resolve to no hotbar bindings: {:?}",
        resolved.bindings
    );

    // The explicit default set still expands to the eight recommended slots.
    let explicit = ConfigToml {
        hotbar: Some(default_hotbar_bindings_toml()),
        ..ConfigToml::default()
    };
    assert_eq!(
        explicit
            .resolve_hotbar_bindings(&DEFAULT_HOTBAR_ACTIONS)
            .bindings,
        default_hotbar_bindings(),
        "an explicit default-bindings config still shows all eight slots"
    );
}

#[test]
fn hotbar_empty_array_disables_default_slots() {
    let config: ConfigToml = toml::from_str("hotbar = []\n").expect("parse empty hotbar array");

    let resolved = config.resolve_hotbar_bindings(&DEFAULT_HOTBAR_ACTIONS);

    assert_eq!(resolved.warnings, Vec::new());
    assert_eq!(resolved.bindings, Vec::new());

    let serialized = toml::to_string_pretty(&config).expect("serialize config");
    let round_tripped: ConfigToml =
        toml::from_str(&serialized).expect("deserialize serialized config");
    assert_eq!(round_tripped.hotbar, Some(Vec::new()));
}

#[test]
fn hotbar_tables_parse_and_round_trip() {
    let config: ConfigToml = toml::from_str(
        r#"
[[hotbar]]
slot = 1
label = "Plan"
action = "mode.plan"

[[hotbar]]
slot = 2
action = "session.compact"
"#,
    )
    .expect("parse hotbar tables");

    let resolved = config.resolve_hotbar_bindings(&["mode.plan", "session.compact"]);

    assert_eq!(
        resolved.bindings,
        vec![
            HotbarBinding {
                slot: 1,
                action: "mode.plan".to_string(),
                label: Some("Plan".to_string()),
            },
            HotbarBinding {
                slot: 2,
                action: "session.compact".to_string(),
                label: None,
            },
        ]
    );
    assert_eq!(resolved.warnings, Vec::new());

    let serialized = toml::to_string_pretty(&config).expect("serialize config");
    let round_tripped: ConfigToml =
        toml::from_str(&serialized).expect("deserialize serialized config");
    assert_eq!(round_tripped.hotbar, config.hotbar);
}

#[test]
fn hotbar_validation_warns_without_dropping_unknown_actions() {
    let config: ConfigToml = toml::from_str(
        r#"
[[hotbar]]
slot = 0
action = "mode.plan"

[[hotbar]]
slot = 2
action = "mode.plan"

[[hotbar]]
slot = 2
action = "custom.action"

[[hotbar]]
slot = 9
action = "mode.agent"
"#,
    )
    .expect("parse hotbar tables");

    let resolved = config.resolve_hotbar_bindings(&["mode.plan", "mode.agent"]);

    assert_eq!(
        resolved.bindings,
        vec![HotbarBinding {
            slot: 2,
            action: "custom.action".to_string(),
            label: None,
        }]
    );
    assert_eq!(
        resolved.warnings,
        vec![
            HotbarConfigWarning::SlotOutOfRange {
                slot: 0,
                action: "mode.plan".to_string(),
            },
            HotbarConfigWarning::UnknownAction {
                slot: 2,
                action: "custom.action".to_string(),
            },
            HotbarConfigWarning::DuplicateSlot {
                slot: 2,
                previous_action: "mode.plan".to_string(),
                replacement_action: "custom.action".to_string(),
            },
            HotbarConfigWarning::SlotOutOfRange {
                slot: 9,
                action: "mode.agent".to_string(),
            },
        ]
    );
    assert!(resolved.warnings[1].to_string().contains("keeping binding"));
}

#[test]
fn config_store_loads_sibling_permissions_toml() {
    use std::time::{SystemTime, UNIX_EPOCH};

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "codewhale-permissions-schema-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("mkdir");
    let config_path = dir.join(CONFIG_FILE_NAME);
    let permissions_path = dir.join(PERMISSIONS_FILE_NAME);
    fs::write(&config_path, "model = \"deepseek-v4-flash\"\n").expect("write config");
    fs::write(
        &permissions_path,
        r#"
        [[rules]]
        tool = "exec_shell"
        command = "cargo test"

        [[rules]]
        tool = "read_file"
        path = "secrets/api_key.txt"
        "#,
    )
    .expect("write permissions");

    let store = ConfigStore::load(Some(config_path.clone())).expect("load config store");

    assert_eq!(store.config.model.as_deref(), Some("deepseek-v4-flash"));
    assert_eq!(
        store.permissions().rules.as_slice(),
        &[
            ToolAskRule::exec_shell("cargo test"),
            ToolAskRule::file_path("read_file", "secrets/api_key.txt"),
        ]
    );
    assert_eq!(
        store
            .permissions_path()
            .canonicalize()
            .expect("store perms"),
        permissions_path.canonicalize().expect("expected perms")
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn config_store_loads_permissions_even_when_config_is_absent() {
    use std::time::{SystemTime, UNIX_EPOCH};

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "codewhale-permissions-only-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("mkdir");
    let config_path = dir.join(CONFIG_FILE_NAME);
    fs::write(
        dir.join(PERMISSIONS_FILE_NAME),
        r#"
        [[rules]]
        tool = "exec_shell"
        command = "cargo check"
        "#,
    )
    .expect("write permissions");

    let store = ConfigStore::load(Some(config_path)).expect("load config store");

    assert!(store.config.model.is_none());
    assert_eq!(
        store.permissions().rules.as_slice(),
        &[ToolAskRule::exec_shell("cargo check")]
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn config_store_exec_policy_engine_uses_sibling_permissions() {
    use std::time::{SystemTime, UNIX_EPOCH};

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "codewhale-permissions-engine-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("mkdir");
    let config_path = dir.join(CONFIG_FILE_NAME);
    fs::write(&config_path, "model = \"deepseek-v4-flash\"\n").expect("write config");
    fs::write(
        dir.join(PERMISSIONS_FILE_NAME),
        r#"
        [[rules]]
        tool = "exec_shell"
        command = "cargo test"
        "#,
    )
    .expect("write permissions");

    let store = ConfigStore::load(Some(config_path)).expect("load config store");
    let decision = store
        .exec_policy_engine()
        .check(codewhale_execpolicy::ExecPolicyContext {
            command: "cargo test --workspace",
            cwd: "/workspace",
            tool: Some("exec_shell"),
            path: None,
            ask_for_approval: codewhale_execpolicy::AskForApproval::UnlessTrusted,
            sandbox_mode: Some("workspace-write"),
        })
        .expect("policy check");

    assert!(decision.allow);
    assert!(decision.requires_approval);
    assert_eq!(
        decision.matched_rule.as_deref(),
        Some("tool=exec_shell command=cargo test")
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn config_store_appends_ask_rules_without_losing_comments_or_duplicates() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join(CONFIG_FILE_NAME);
    let permissions_path = dir.path().join(PERMISSIONS_FILE_NAME);
    fs::write(&config_path, "model = \"deepseek-v4-flash\"\n").expect("write config");
    fs::write(
        &permissions_path,
        r#"# keep this permission note
[[rules]]
tool = "exec_shell"
command = "cargo check"
"#,
    )
    .expect("write permissions");

    let mut store = ConfigStore::load(Some(config_path)).expect("load config store");
    let existing = ToolAskRule::exec_shell("cargo check");
    let added_rule = ToolAskRule::file_path("read_file", "docs/README.md");
    let added = store
        .append_ask_rules(&[existing, added_rule.clone(), added_rule.clone()])
        .expect("append ask rules");

    assert_eq!(added, 1);
    assert_eq!(
        store.permissions().rules,
        vec![ToolAskRule::exec_shell("cargo check"), added_rule.clone(),]
    );
    let body = fs::read_to_string(&permissions_path).expect("read permissions");
    assert!(body.contains("# keep this permission note"));
    assert_eq!(body.matches("docs/README.md").count(), 1);
    assert!(!body.contains("decision"));

    let before_duplicate_append = body;
    assert_eq!(
        store
            .append_ask_rules(&[added_rule])
            .expect("dedupe ask rule"),
        0
    );
    assert_eq!(
        fs::read_to_string(&permissions_path).expect("read unchanged permissions"),
        before_duplicate_append
    );

    let reloaded =
        ConfigStore::load(Some(dir.path().join(CONFIG_FILE_NAME))).expect("reload config store");
    assert_eq!(reloaded.permissions(), store.permissions());
}

#[test]
fn config_store_appends_ask_rule_to_inline_rules_array() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join(CONFIG_FILE_NAME);
    let permissions_path = dir.path().join(PERMISSIONS_FILE_NAME);
    fs::write(
        &permissions_path,
        "# inline rules stay valid\nrules = [{ tool = \"exec_shell\", command = \"cargo check\" }]\n",
    )
    .expect("write permissions");

    let mut store = ConfigStore::load(Some(config_path)).expect("load config store");
    assert_eq!(
        store
            .append_ask_rules(&[ToolAskRule::file_path("read_file", "README.md")])
            .expect("append inline ask rule"),
        1
    );

    let body = fs::read_to_string(&permissions_path).expect("read permissions");
    assert!(body.contains("# inline rules stay valid"));
    let parsed: PermissionsToml = toml::from_str(&body).expect("parse persisted permissions");
    assert_eq!(
        parsed.rules,
        vec![
            ToolAskRule::exec_shell("cargo check"),
            ToolAskRule::file_path("read_file", "README.md"),
        ]
    );
}

#[test]
fn config_store_does_not_overwrite_invalid_permissions_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join(CONFIG_FILE_NAME);
    let permissions_path = dir.path().join(PERMISSIONS_FILE_NAME);
    let mut store = ConfigStore::load(Some(config_path)).expect("load config store");
    let invalid = "rules = \"not-an-array\"\n";
    fs::write(&permissions_path, invalid).expect("write invalid permissions");

    let error = store
        .append_ask_rules(&[ToolAskRule::exec_shell("cargo test")])
        .expect_err("invalid permissions should fail");

    assert!(error.to_string().contains("failed to parse permissions"));
    assert_eq!(
        fs::read_to_string(&permissions_path).expect("read invalid permissions"),
        invalid
    );
    assert!(store.permissions().is_empty());
}

#[test]
fn duplicate_append_refreshes_permissions_changed_on_disk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join(CONFIG_FILE_NAME);
    let permissions_path = dir.path().join(PERMISSIONS_FILE_NAME);
    let mut store = ConfigStore::load(Some(config_path)).expect("load config store");
    fs::write(
        permissions_path,
        "[[rules]]\ntool = \"exec_shell\"\ncommand = \"cargo check\"\n",
    )
    .expect("write external permissions update");

    assert_eq!(
        store
            .append_ask_rules(&[ToolAskRule::exec_shell("cargo check")])
            .expect("dedupe external ask rule"),
        0
    );
    assert_eq!(
        store.permissions().rules,
        vec![ToolAskRule::exec_shell("cargo check")]
    );
}

#[cfg(unix)]
#[test]
fn config_store_secures_persisted_permissions_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join(CONFIG_FILE_NAME);
    let permissions_path = dir.path().join(PERMISSIONS_FILE_NAME);
    let mut store = ConfigStore::load(Some(config_path)).expect("load config store");

    store
        .append_ask_rules(&[ToolAskRule::exec_shell("cargo test")])
        .expect("append ask rule");

    let mode = fs::metadata(permissions_path)
        .expect("permissions metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}

struct EnvGuard {
    deepseek_api_key: Option<OsString>,
    deepseek_base_url: Option<OsString>,
    deepseek_anthropic_base_url: Option<OsString>,
    deepseek_claude_base_url: Option<OsString>,
    deepseek_http_headers: Option<OsString>,
    deepseek_model: Option<OsString>,
    deepseek_default_text_model: Option<OsString>,
    deepseek_provider: Option<OsString>,
    deepseek_auth_mode: Option<OsString>,
    nvidia_api_key: Option<OsString>,
    nvidia_nim_api_key: Option<OsString>,
    nim_base_url: Option<OsString>,
    nvidia_base_url: Option<OsString>,
    nvidia_nim_base_url: Option<OsString>,
    openrouter_api_key: Option<OsString>,
    openrouter_base_url: Option<OsString>,
    openrouter_model: Option<OsString>,
    openmodel_api_key: Option<OsString>,
    openmodel_base_url: Option<OsString>,
    openmodel_model: Option<OsString>,
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
    wanjie_ark_api_key: Option<OsString>,
    volcengine_api_key: Option<OsString>,
    volcengine_ark_api_key: Option<OsString>,
    ark_api_key: Option<OsString>,
    volcengine_base_url: Option<OsString>,
    volcengine_ark_base_url: Option<OsString>,
    ark_base_url: Option<OsString>,
    wanjie_ark_base_url: Option<OsString>,
    wanjie_base_url: Option<OsString>,
    wanjie_maas_base_url: Option<OsString>,
    volcengine_model: Option<OsString>,
    volcengine_ark_model: Option<OsString>,
    wanjie_ark_model: Option<OsString>,
    wanjie_model: Option<OsString>,
    wanjie_maas_model: Option<OsString>,
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
    zai_api_key: Option<OsString>,
    z_ai_api_key: Option<OsString>,
    zhipu_api_key: Option<OsString>,
    glm_api_key: Option<OsString>,
    zai_base_url: Option<OsString>,
    z_ai_base_url: Option<OsString>,
    zhipu_base_url: Option<OsString>,
    zhipuai_base_url: Option<OsString>,
    bigmodel_base_url: Option<OsString>,
    zai_model: Option<OsString>,
    z_ai_model: Option<OsString>,
    zhipu_model: Option<OsString>,
    zhipuai_model: Option<OsString>,
    bigmodel_model: Option<OsString>,
    glm_model: Option<OsString>,
    stepfun_api_key: Option<OsString>,
    step_api_key: Option<OsString>,
    stepfun_base_url: Option<OsString>,
    stepfun_model: Option<OsString>,
    minimax_api_key: Option<OsString>,
    minimax_base_url: Option<OsString>,
    minimax_anthropic_base_url: Option<OsString>,
    minimax_model: Option<OsString>,
    sakana_api_key: Option<OsString>,
    fugu_api_key: Option<OsString>,
    sakana_base_url: Option<OsString>,
    sakana_model: Option<OsString>,
    sglang_api_key: Option<OsString>,
    sglang_base_url: Option<OsString>,
    vllm_api_key: Option<OsString>,
    vllm_base_url: Option<OsString>,
    ollama_api_key: Option<OsString>,
    ollama_base_url: Option<OsString>,
    huggingface_api_key: Option<OsString>,
    huggingface_token: Option<OsString>,
    huggingface_base_url: Option<OsString>,
    hf_base_url: Option<OsString>,
    huggingface_model: Option<OsString>,
    hf_model: Option<OsString>,
    codewhale_provider: Option<OsString>,
    codewhale_model: Option<OsString>,
    codewhale_base_url: Option<OsString>,
    xai_api_key: Option<OsString>,
    xai_base_url: Option<OsString>,
    xai_model: Option<OsString>,
    opencode_go_api_key: Option<OsString>,
    opencode_go_base_url: Option<OsString>,
    opencode_go_model: Option<OsString>,
    opencode_zen_api_key: Option<OsString>,
    opencode_api_key: Option<OsString>,
    opencode_zen_base_url: Option<OsString>,
    opencode_zen_model: Option<OsString>,
    meta_model_api_key: Option<OsString>,
    model_api_key: Option<OsString>,
    meta_model_api_base_url: Option<OsString>,
    model_api_base_url: Option<OsString>,
    meta_model_api_model: Option<OsString>,
    model_api_model: Option<OsString>,
}

impl EnvGuard {
    fn without_deepseek_runtime_overrides() -> Self {
        let guard = Self {
            deepseek_api_key: env::var_os("DEEPSEEK_API_KEY"),
            deepseek_base_url: env::var_os("DEEPSEEK_BASE_URL"),
            deepseek_anthropic_base_url: env::var_os("DEEPSEEK_ANTHROPIC_BASE_URL"),
            deepseek_claude_base_url: env::var_os("DEEPSEEK_CLAUDE_BASE_URL"),
            deepseek_http_headers: env::var_os("DEEPSEEK_HTTP_HEADERS"),
            deepseek_model: env::var_os("DEEPSEEK_MODEL"),
            deepseek_default_text_model: env::var_os("DEEPSEEK_DEFAULT_TEXT_MODEL"),
            deepseek_provider: env::var_os("DEEPSEEK_PROVIDER"),
            deepseek_auth_mode: env::var_os("DEEPSEEK_AUTH_MODE"),
            codewhale_provider: env::var_os("CODEWHALE_PROVIDER"),
            codewhale_model: env::var_os("CODEWHALE_MODEL"),
            codewhale_base_url: env::var_os("CODEWHALE_BASE_URL"),
            xai_api_key: env::var_os("XAI_API_KEY"),
            xai_base_url: env::var_os("XAI_BASE_URL"),
            xai_model: env::var_os("XAI_MODEL"),
            opencode_go_api_key: env::var_os("OPENCODE_GO_API_KEY"),
            opencode_go_base_url: env::var_os("OPENCODE_GO_BASE_URL"),
            opencode_go_model: env::var_os("OPENCODE_GO_MODEL"),
            opencode_zen_api_key: env::var_os("OPENCODE_ZEN_API_KEY"),
            opencode_api_key: env::var_os("OPENCODE_API_KEY"),
            opencode_zen_base_url: env::var_os("OPENCODE_ZEN_BASE_URL"),
            opencode_zen_model: env::var_os("OPENCODE_ZEN_MODEL"),
            meta_model_api_key: env::var_os("META_MODEL_API_KEY"),
            model_api_key: env::var_os("MODEL_API_KEY"),
            meta_model_api_base_url: env::var_os("META_MODEL_API_BASE_URL"),
            model_api_base_url: env::var_os("MODEL_API_BASE_URL"),
            meta_model_api_model: env::var_os("META_MODEL_API_MODEL"),
            model_api_model: env::var_os("MODEL_API_MODEL"),
            nvidia_api_key: env::var_os("NVIDIA_API_KEY"),
            nvidia_nim_api_key: env::var_os("NVIDIA_NIM_API_KEY"),
            nim_base_url: env::var_os("NIM_BASE_URL"),
            nvidia_base_url: env::var_os("NVIDIA_BASE_URL"),
            nvidia_nim_base_url: env::var_os("NVIDIA_NIM_BASE_URL"),
            openrouter_api_key: env::var_os("OPENROUTER_API_KEY"),
            openrouter_base_url: env::var_os("OPENROUTER_BASE_URL"),
            openrouter_model: env::var_os("OPENROUTER_MODEL"),
            openmodel_api_key: env::var_os("OPENMODEL_API_KEY"),
            openmodel_base_url: env::var_os("OPENMODEL_BASE_URL"),
            openmodel_model: env::var_os("OPENMODEL_MODEL"),
            xiaomi_mimo_token_plan_api_key: env::var_os("XIAOMI_MIMO_TOKEN_PLAN_API_KEY"),
            mimo_token_plan_api_key: env::var_os("MIMO_TOKEN_PLAN_API_KEY"),
            xiaomi_mimo_api_key: env::var_os("XIAOMI_MIMO_API_KEY"),
            xiaomi_api_key: env::var_os("XIAOMI_API_KEY"),
            mimo_api_key: env::var_os("MIMO_API_KEY"),
            xiaomi_mimo_base_url: env::var_os("XIAOMI_MIMO_BASE_URL"),
            mimo_base_url: env::var_os("MIMO_BASE_URL"),
            xiaomi_mimo_model: env::var_os("XIAOMI_MIMO_MODEL"),
            mimo_model: env::var_os("MIMO_MODEL"),
            xiaomi_mimo_mode: env::var_os("XIAOMI_MIMO_MODE"),
            mimo_mode: env::var_os("MIMO_MODE"),
            wanjie_ark_api_key: env::var_os("WANJIE_ARK_API_KEY"),
            volcengine_api_key: env::var_os("VOLCENGINE_API_KEY"),
            volcengine_ark_api_key: env::var_os("VOLCENGINE_ARK_API_KEY"),
            ark_api_key: env::var_os("ARK_API_KEY"),
            volcengine_base_url: env::var_os("VOLCENGINE_BASE_URL"),
            volcengine_ark_base_url: env::var_os("VOLCENGINE_ARK_BASE_URL"),
            ark_base_url: env::var_os("ARK_BASE_URL"),
            wanjie_ark_base_url: env::var_os("WANJIE_ARK_BASE_URL"),
            wanjie_base_url: env::var_os("WANJIE_BASE_URL"),
            wanjie_maas_base_url: env::var_os("WANJIE_MAAS_BASE_URL"),
            volcengine_model: env::var_os("VOLCENGINE_MODEL"),
            volcengine_ark_model: env::var_os("VOLCENGINE_ARK_MODEL"),
            wanjie_ark_model: env::var_os("WANJIE_ARK_MODEL"),
            wanjie_model: env::var_os("WANJIE_MODEL"),
            wanjie_maas_model: env::var_os("WANJIE_MAAS_MODEL"),
            novita_api_key: env::var_os("NOVITA_API_KEY"),
            novita_base_url: env::var_os("NOVITA_BASE_URL"),
            novita_model: env::var_os("NOVITA_MODEL"),
            fireworks_api_key: env::var_os("FIREWORKS_API_KEY"),
            fireworks_base_url: env::var_os("FIREWORKS_BASE_URL"),
            fireworks_model: env::var_os("FIREWORKS_MODEL"),
            siliconflow_api_key: env::var_os("SILICONFLOW_API_KEY"),
            siliconflow_base_url: env::var_os("SILICONFLOW_BASE_URL"),
            siliconflow_model: env::var_os("SILICONFLOW_MODEL"),
            arcee_api_key: env::var_os("ARCEE_API_KEY"),
            arcee_base_url: env::var_os("ARCEE_BASE_URL"),
            arcee_model: env::var_os("ARCEE_MODEL"),
            moonshot_api_key: env::var_os("MOONSHOT_API_KEY"),
            moonshot_base_url: env::var_os("MOONSHOT_BASE_URL"),
            moonshot_model: env::var_os("MOONSHOT_MODEL"),
            kimi_api_key: env::var_os("KIMI_API_KEY"),
            kimi_base_url: env::var_os("KIMI_BASE_URL"),
            kimi_model: env::var_os("KIMI_MODEL"),
            kimi_model_name: env::var_os("KIMI_MODEL_NAME"),
            zai_api_key: env::var_os("ZAI_API_KEY"),
            z_ai_api_key: env::var_os("Z_AI_API_KEY"),
            zhipu_api_key: env::var_os("ZHIPU_API_KEY"),
            glm_api_key: env::var_os("GLM_API_KEY"),
            zai_base_url: env::var_os("ZAI_BASE_URL"),
            z_ai_base_url: env::var_os("Z_AI_BASE_URL"),
            zhipu_base_url: env::var_os("ZHIPU_BASE_URL"),
            zhipuai_base_url: env::var_os("ZHIPUAI_BASE_URL"),
            bigmodel_base_url: env::var_os("BIGMODEL_BASE_URL"),
            zai_model: env::var_os("ZAI_MODEL"),
            z_ai_model: env::var_os("Z_AI_MODEL"),
            zhipu_model: env::var_os("ZHIPU_MODEL"),
            zhipuai_model: env::var_os("ZHIPUAI_MODEL"),
            bigmodel_model: env::var_os("BIGMODEL_MODEL"),
            glm_model: env::var_os("GLM_MODEL"),
            stepfun_api_key: env::var_os("STEPFUN_API_KEY"),
            step_api_key: env::var_os("STEP_API_KEY"),
            stepfun_base_url: env::var_os("STEPFUN_BASE_URL"),
            stepfun_model: env::var_os("STEPFUN_MODEL"),
            minimax_api_key: env::var_os("MINIMAX_API_KEY"),
            minimax_base_url: env::var_os("MINIMAX_BASE_URL"),
            minimax_anthropic_base_url: env::var_os("MINIMAX_ANTHROPIC_BASE_URL"),
            minimax_model: env::var_os("MINIMAX_MODEL"),
            sakana_api_key: env::var_os("SAKANA_API_KEY"),
            fugu_api_key: env::var_os("FUGU_API_KEY"),
            sakana_base_url: env::var_os("SAKANA_BASE_URL"),
            sakana_model: env::var_os("SAKANA_MODEL"),
            sglang_api_key: env::var_os("SGLANG_API_KEY"),
            sglang_base_url: env::var_os("SGLANG_BASE_URL"),
            vllm_api_key: env::var_os("VLLM_API_KEY"),
            vllm_base_url: env::var_os("VLLM_BASE_URL"),
            ollama_api_key: env::var_os("OLLAMA_API_KEY"),
            ollama_base_url: env::var_os("OLLAMA_BASE_URL"),
            huggingface_api_key: env::var_os("HUGGINGFACE_API_KEY"),
            huggingface_token: env::var_os("HF_TOKEN"),
            huggingface_base_url: env::var_os("HUGGINGFACE_BASE_URL"),
            hf_base_url: env::var_os("HF_BASE_URL"),
            huggingface_model: env::var_os("HUGGINGFACE_MODEL"),
            hf_model: env::var_os("HF_MODEL"),
        };
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            env::remove_var("DEEPSEEK_API_KEY");
            env::remove_var("DEEPSEEK_BASE_URL");
            env::remove_var("DEEPSEEK_ANTHROPIC_BASE_URL");
            env::remove_var("DEEPSEEK_CLAUDE_BASE_URL");
            env::remove_var("DEEPSEEK_HTTP_HEADERS");
            env::remove_var("DEEPSEEK_MODEL");
            env::remove_var("DEEPSEEK_DEFAULT_TEXT_MODEL");
            env::remove_var("DEEPSEEK_PROVIDER");
            env::remove_var("DEEPSEEK_AUTH_MODE");
            env::remove_var("CODEWHALE_PROVIDER");
            env::remove_var("CODEWHALE_MODEL");
            env::remove_var("CODEWHALE_BASE_URL");
            env::remove_var("XAI_API_KEY");
            env::remove_var("XAI_BASE_URL");
            env::remove_var("XAI_MODEL");
            env::remove_var("OPENCODE_GO_API_KEY");
            env::remove_var("OPENCODE_GO_BASE_URL");
            env::remove_var("OPENCODE_GO_MODEL");
            env::remove_var("OPENCODE_ZEN_API_KEY");
            env::remove_var("OPENCODE_API_KEY");
            env::remove_var("OPENCODE_ZEN_BASE_URL");
            env::remove_var("OPENCODE_ZEN_MODEL");
            env::remove_var("META_MODEL_API_KEY");
            env::remove_var("MODEL_API_KEY");
            env::remove_var("META_MODEL_API_BASE_URL");
            env::remove_var("MODEL_API_BASE_URL");
            env::remove_var("META_MODEL_API_MODEL");
            env::remove_var("MODEL_API_MODEL");
            env::remove_var("NVIDIA_API_KEY");
            env::remove_var("NVIDIA_NIM_API_KEY");
            env::remove_var("NIM_BASE_URL");
            env::remove_var("NVIDIA_BASE_URL");
            env::remove_var("NVIDIA_NIM_BASE_URL");
            env::remove_var("OPENROUTER_API_KEY");
            env::remove_var("OPENROUTER_BASE_URL");
            env::remove_var("OPENROUTER_MODEL");
            env::remove_var("OPENMODEL_API_KEY");
            env::remove_var("OPENMODEL_BASE_URL");
            env::remove_var("OPENMODEL_MODEL");
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
            env::remove_var("WANJIE_ARK_API_KEY");
            env::remove_var("VOLCENGINE_API_KEY");
            env::remove_var("VOLCENGINE_ARK_API_KEY");
            env::remove_var("ARK_API_KEY");
            env::remove_var("VOLCENGINE_BASE_URL");
            env::remove_var("VOLCENGINE_ARK_BASE_URL");
            env::remove_var("ARK_BASE_URL");
            env::remove_var("WANJIE_ARK_BASE_URL");
            env::remove_var("WANJIE_BASE_URL");
            env::remove_var("WANJIE_MAAS_BASE_URL");
            env::remove_var("VOLCENGINE_MODEL");
            env::remove_var("VOLCENGINE_ARK_MODEL");
            env::remove_var("WANJIE_ARK_MODEL");
            env::remove_var("WANJIE_MODEL");
            env::remove_var("WANJIE_MAAS_MODEL");
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
            env::remove_var("ZAI_API_KEY");
            env::remove_var("Z_AI_API_KEY");
            env::remove_var("ZHIPU_API_KEY");
            env::remove_var("GLM_API_KEY");
            env::remove_var("ZAI_BASE_URL");
            env::remove_var("Z_AI_BASE_URL");
            env::remove_var("ZHIPU_BASE_URL");
            env::remove_var("ZHIPUAI_BASE_URL");
            env::remove_var("BIGMODEL_BASE_URL");
            env::remove_var("ZAI_MODEL");
            env::remove_var("Z_AI_MODEL");
            env::remove_var("ZHIPU_MODEL");
            env::remove_var("ZHIPUAI_MODEL");
            env::remove_var("BIGMODEL_MODEL");
            env::remove_var("GLM_MODEL");
            env::remove_var("STEPFUN_API_KEY");
            env::remove_var("STEP_API_KEY");
            env::remove_var("STEPFUN_BASE_URL");
            env::remove_var("STEPFUN_MODEL");
            env::remove_var("MINIMAX_API_KEY");
            env::remove_var("MINIMAX_BASE_URL");
            env::remove_var("MINIMAX_ANTHROPIC_BASE_URL");
            env::remove_var("MINIMAX_MODEL");
            env::remove_var("SAKANA_API_KEY");
            env::remove_var("FUGU_API_KEY");
            env::remove_var("SAKANA_BASE_URL");
            env::remove_var("SAKANA_MODEL");
            env::remove_var("SGLANG_API_KEY");
            env::remove_var("SGLANG_BASE_URL");
            env::remove_var("VLLM_API_KEY");
            env::remove_var("VLLM_BASE_URL");
            env::remove_var("OLLAMA_API_KEY");
            env::remove_var("OLLAMA_BASE_URL");
            env::remove_var("HUGGINGFACE_API_KEY");
            env::remove_var("HF_TOKEN");
            env::remove_var("HUGGINGFACE_BASE_URL");
            env::remove_var("HF_BASE_URL");
            env::remove_var("HUGGINGFACE_MODEL");
            env::remove_var("HF_MODEL");
        }
        guard
    }

    unsafe fn restore_var(key: &str, value: Option<OsString>) {
        if let Some(value) = value {
            unsafe { env::set_var(key, value) };
        } else {
            unsafe { env::remove_var(key) };
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            Self::restore_var("DEEPSEEK_API_KEY", self.deepseek_api_key.take());
            Self::restore_var("DEEPSEEK_BASE_URL", self.deepseek_base_url.take());
            Self::restore_var(
                "DEEPSEEK_ANTHROPIC_BASE_URL",
                self.deepseek_anthropic_base_url.take(),
            );
            Self::restore_var(
                "DEEPSEEK_CLAUDE_BASE_URL",
                self.deepseek_claude_base_url.take(),
            );
            Self::restore_var("DEEPSEEK_HTTP_HEADERS", self.deepseek_http_headers.take());
            Self::restore_var("DEEPSEEK_MODEL", self.deepseek_model.take());
            Self::restore_var(
                "DEEPSEEK_DEFAULT_TEXT_MODEL",
                self.deepseek_default_text_model.take(),
            );
            Self::restore_var("DEEPSEEK_PROVIDER", self.deepseek_provider.take());
            Self::restore_var("DEEPSEEK_AUTH_MODE", self.deepseek_auth_mode.take());
            Self::restore_var("CODEWHALE_PROVIDER", self.codewhale_provider.take());
            Self::restore_var("CODEWHALE_MODEL", self.codewhale_model.take());
            Self::restore_var("CODEWHALE_BASE_URL", self.codewhale_base_url.take());
            Self::restore_var("XAI_API_KEY", self.xai_api_key.take());
            Self::restore_var("XAI_BASE_URL", self.xai_base_url.take());
            Self::restore_var("XAI_MODEL", self.xai_model.take());
            Self::restore_var("OPENCODE_GO_API_KEY", self.opencode_go_api_key.take());
            Self::restore_var("OPENCODE_GO_BASE_URL", self.opencode_go_base_url.take());
            Self::restore_var("OPENCODE_GO_MODEL", self.opencode_go_model.take());
            Self::restore_var("OPENCODE_ZEN_API_KEY", self.opencode_zen_api_key.take());
            Self::restore_var("OPENCODE_API_KEY", self.opencode_api_key.take());
            Self::restore_var("OPENCODE_ZEN_BASE_URL", self.opencode_zen_base_url.take());
            Self::restore_var("OPENCODE_ZEN_MODEL", self.opencode_zen_model.take());
            Self::restore_var("META_MODEL_API_KEY", self.meta_model_api_key.take());
            Self::restore_var("MODEL_API_KEY", self.model_api_key.take());
            Self::restore_var(
                "META_MODEL_API_BASE_URL",
                self.meta_model_api_base_url.take(),
            );
            Self::restore_var("MODEL_API_BASE_URL", self.model_api_base_url.take());
            Self::restore_var("META_MODEL_API_MODEL", self.meta_model_api_model.take());
            Self::restore_var("MODEL_API_MODEL", self.model_api_model.take());
            Self::restore_var("NVIDIA_API_KEY", self.nvidia_api_key.take());
            Self::restore_var("NVIDIA_NIM_API_KEY", self.nvidia_nim_api_key.take());
            Self::restore_var("NIM_BASE_URL", self.nim_base_url.take());
            Self::restore_var("NVIDIA_BASE_URL", self.nvidia_base_url.take());
            Self::restore_var("NVIDIA_NIM_BASE_URL", self.nvidia_nim_base_url.take());
            Self::restore_var("OPENROUTER_API_KEY", self.openrouter_api_key.take());
            Self::restore_var("OPENROUTER_BASE_URL", self.openrouter_base_url.take());
            Self::restore_var("OPENROUTER_MODEL", self.openrouter_model.take());
            Self::restore_var("OPENMODEL_API_KEY", self.openmodel_api_key.take());
            Self::restore_var("OPENMODEL_BASE_URL", self.openmodel_base_url.take());
            Self::restore_var("OPENMODEL_MODEL", self.openmodel_model.take());
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
            Self::restore_var("WANJIE_ARK_API_KEY", self.wanjie_ark_api_key.take());
            Self::restore_var("VOLCENGINE_API_KEY", self.volcengine_api_key.take());
            Self::restore_var("VOLCENGINE_ARK_API_KEY", self.volcengine_ark_api_key.take());
            Self::restore_var("ARK_API_KEY", self.ark_api_key.take());
            Self::restore_var("VOLCENGINE_BASE_URL", self.volcengine_base_url.take());
            Self::restore_var(
                "VOLCENGINE_ARK_BASE_URL",
                self.volcengine_ark_base_url.take(),
            );
            Self::restore_var("ARK_BASE_URL", self.ark_base_url.take());
            Self::restore_var("WANJIE_ARK_BASE_URL", self.wanjie_ark_base_url.take());
            Self::restore_var("WANJIE_BASE_URL", self.wanjie_base_url.take());
            Self::restore_var("WANJIE_MAAS_BASE_URL", self.wanjie_maas_base_url.take());
            Self::restore_var("VOLCENGINE_MODEL", self.volcengine_model.take());
            Self::restore_var("VOLCENGINE_ARK_MODEL", self.volcengine_ark_model.take());
            Self::restore_var("WANJIE_ARK_MODEL", self.wanjie_ark_model.take());
            Self::restore_var("WANJIE_MODEL", self.wanjie_model.take());
            Self::restore_var("WANJIE_MAAS_MODEL", self.wanjie_maas_model.take());
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
            Self::restore_var("ZAI_API_KEY", self.zai_api_key.take());
            Self::restore_var("Z_AI_API_KEY", self.z_ai_api_key.take());
            Self::restore_var("ZHIPU_API_KEY", self.zhipu_api_key.take());
            Self::restore_var("GLM_API_KEY", self.glm_api_key.take());
            Self::restore_var("ZAI_BASE_URL", self.zai_base_url.take());
            Self::restore_var("Z_AI_BASE_URL", self.z_ai_base_url.take());
            Self::restore_var("ZHIPU_BASE_URL", self.zhipu_base_url.take());
            Self::restore_var("ZHIPUAI_BASE_URL", self.zhipuai_base_url.take());
            Self::restore_var("BIGMODEL_BASE_URL", self.bigmodel_base_url.take());
            Self::restore_var("ZAI_MODEL", self.zai_model.take());
            Self::restore_var("Z_AI_MODEL", self.z_ai_model.take());
            Self::restore_var("ZHIPU_MODEL", self.zhipu_model.take());
            Self::restore_var("ZHIPUAI_MODEL", self.zhipuai_model.take());
            Self::restore_var("BIGMODEL_MODEL", self.bigmodel_model.take());
            Self::restore_var("GLM_MODEL", self.glm_model.take());
            Self::restore_var("STEPFUN_API_KEY", self.stepfun_api_key.take());
            Self::restore_var("STEP_API_KEY", self.step_api_key.take());
            Self::restore_var("STEPFUN_BASE_URL", self.stepfun_base_url.take());
            Self::restore_var("STEPFUN_MODEL", self.stepfun_model.take());
            Self::restore_var("MINIMAX_API_KEY", self.minimax_api_key.take());
            Self::restore_var("MINIMAX_BASE_URL", self.minimax_base_url.take());
            Self::restore_var(
                "MINIMAX_ANTHROPIC_BASE_URL",
                self.minimax_anthropic_base_url.take(),
            );
            Self::restore_var("MINIMAX_MODEL", self.minimax_model.take());
            Self::restore_var("SAKANA_API_KEY", self.sakana_api_key.take());
            Self::restore_var("FUGU_API_KEY", self.fugu_api_key.take());
            Self::restore_var("SAKANA_BASE_URL", self.sakana_base_url.take());
            Self::restore_var("SAKANA_MODEL", self.sakana_model.take());
            Self::restore_var("SGLANG_API_KEY", self.sglang_api_key.take());
            Self::restore_var("SGLANG_BASE_URL", self.sglang_base_url.take());
            Self::restore_var("VLLM_API_KEY", self.vllm_api_key.take());
            Self::restore_var("VLLM_BASE_URL", self.vllm_base_url.take());
            Self::restore_var("OLLAMA_API_KEY", self.ollama_api_key.take());
            Self::restore_var("OLLAMA_BASE_URL", self.ollama_base_url.take());
            Self::restore_var("HUGGINGFACE_API_KEY", self.huggingface_api_key.take());
            Self::restore_var("HF_TOKEN", self.huggingface_token.take());
            Self::restore_var("HUGGINGFACE_BASE_URL", self.huggingface_base_url.take());
            Self::restore_var("HF_BASE_URL", self.hf_base_url.take());
            Self::restore_var("HUGGINGFACE_MODEL", self.huggingface_model.take());
            Self::restore_var("HF_MODEL", self.hf_model.take());
        }
    }
}

struct RecordingSecretsStore {
    gets: Mutex<Vec<String>>,
    value: Option<String>,
}

impl RecordingSecretsStore {
    fn with_value(value: &str) -> Self {
        Self {
            gets: Mutex::new(Vec::new()),
            value: Some(value.to_string()),
        }
    }
}

impl codewhale_secrets::KeyringStore for RecordingSecretsStore {
    fn get(&self, key: &str) -> Result<Option<String>, codewhale_secrets::SecretsError> {
        self.gets.lock().unwrap().push(key.to_string());
        Ok(self.value.clone())
    }

    fn set(&self, _key: &str, _value: &str) -> Result<(), codewhale_secrets::SecretsError> {
        Ok(())
    }

    fn delete(&self, _key: &str) -> Result<(), codewhale_secrets::SecretsError> {
        Ok(())
    }

    fn backend_name(&self) -> &'static str {
        "recording"
    }
}

#[test]
fn root_deepseek_fields_are_runtime_fallbacks() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let config = ConfigToml {
        api_key: Some("root-key".to_string()),
        base_url: Some("https://api.deepseek.com".to_string()),
        default_text_model: Some("deepseek-v4-pro".to_string()),
        ..ConfigToml::default()
    };

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Deepseek);
    assert_eq!(resolved.api_key.as_deref(), Some("root-key"));
    assert_eq!(resolved.base_url, "https://api.deepseek.com");
    assert_eq!(resolved.model, "deepseek-v4-pro");
}

#[test]
fn deepseek_runtime_defaults_to_beta_endpoint() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let config = ConfigToml::default();

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Deepseek);
    assert_eq!(resolved.base_url, DEFAULT_DEEPSEEK_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_DEEPSEEK_MODEL);
}

#[test]
fn provider_specific_deepseek_fields_override_tui_compat_fields() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let mut config = ConfigToml {
        api_key: Some("root-key".to_string()),
        base_url: Some("https://api.deepseek.com".to_string()),
        default_text_model: Some("deepseek-v4-pro".to_string()),
        ..ConfigToml::default()
    };
    config.providers.deepseek.api_key = Some("provider-key".to_string());
    config.providers.deepseek.base_url = Some("https://gateway.example/v1".to_string());
    config.providers.deepseek.model = Some("deepseek-v4-flash".to_string());

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.api_key.as_deref(), Some("provider-key"));
    assert_eq!(resolved.base_url, "https://gateway.example/v1");
    assert_eq!(resolved.model, "deepseek-v4-flash");
}

#[test]
fn provider_http_headers_override_root_headers() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let mut config = ConfigToml {
        api_key: Some("root-key".to_string()),
        base_url: Some("https://api.deepseek.com".to_string()),
        default_text_model: Some("deepseek-v4-pro".to_string()),
        ..ConfigToml::default()
    };
    config.providers.deepseek.api_key = Some("provider-key".to_string());
    config.providers.deepseek.base_url = Some("https://gateway.example/v1".to_string());
    config.providers.deepseek.model = Some("deepseek-v4-flash".to_string());
    config
        .http_headers
        .insert("X-Shared".to_string(), "root".to_string());
    config
        .providers
        .deepseek
        .http_headers
        .insert("X-Model-Provider-Id".to_string(), "tongyi".to_string());
    config
        .providers
        .deepseek
        .http_headers
        .insert("X-Shared".to_string(), "provider".to_string());

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.api_key.as_deref(), Some("provider-key"));
    assert_eq!(resolved.base_url, "https://gateway.example/v1");
    assert_eq!(resolved.model, "deepseek-v4-flash");
    assert_eq!(
        resolved
            .http_headers
            .get("X-Model-Provider-Id")
            .map(String::as_str),
        Some("tongyi")
    );
    assert_eq!(
        resolved.http_headers.get("X-Shared").map(String::as_str),
        Some("provider")
    );
}

#[test]
fn insecure_skip_tls_verify_resolves_only_for_active_provider() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let mut config = ConfigToml {
        provider: ProviderKind::Openai,
        ..ConfigToml::default()
    };
    config.providers.deepseek.insecure_skip_tls_verify = Some(true);

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Openai);
    assert!(!resolved.insecure_skip_tls_verify);

    config.providers.openai.insecure_skip_tls_verify = Some(true);
    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Openai);
    assert!(resolved.insecure_skip_tls_verify);
}

#[test]
fn openai_provider_accepts_dashscope_bailian_base_url_and_model() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let mut config = ConfigToml {
        provider: ProviderKind::Openai,
        ..ConfigToml::default()
    };
    config.providers.openai.api_key = Some("dashscope-table-key".to_string());
    config.providers.openai.base_url =
        Some("https://dashscope-intl.aliyuncs.com/compatible-mode/v1".to_string());
    config.providers.openai.model = Some("qwen-plus".to_string());

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Openai);
    assert_eq!(resolved.api_key.as_deref(), Some("dashscope-table-key"));
    assert_eq!(
        resolved.base_url,
        "https://dashscope-intl.aliyuncs.com/compatible-mode/v1"
    );
    assert_eq!(resolved.model, "qwen-plus");
}

#[test]
fn http_headers_env_overrides_config() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let mut config = ConfigToml::default();
    config
        .http_headers
        .insert("X-Model-Provider-Id".to_string(), "from-file".to_string());
    // Safety: test-only environment mutation guarded by a module mutex.
    unsafe {
        env::set_var("DEEPSEEK_HTTP_HEADERS", "X-Model-Provider-Id=from-env");
    }

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(
        resolved
            .http_headers
            .get("X-Model-Provider-Id")
            .map(String::as_str),
        Some("from-env")
    );
}

#[test]
fn nvidia_nim_provider_defaults_to_catalog_endpoint_and_model() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let config = ConfigToml {
        provider: ProviderKind::NvidiaNim,
        ..ConfigToml::default()
    };

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::NvidiaNim);
    assert_eq!(resolved.base_url, DEFAULT_NVIDIA_NIM_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_NVIDIA_NIM_MODEL);
}

#[test]
fn nvidia_nim_provider_uses_provider_specific_credentials() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let mut config = ConfigToml {
        provider: ProviderKind::NvidiaNim,
        ..ConfigToml::default()
    };
    config.providers.nvidia_nim.api_key = Some("nim-key".to_string());
    config.providers.nvidia_nim.base_url = Some("https://nim.example/v1".to_string());
    config.providers.nvidia_nim.model = Some("deepseek-ai/deepseek-v4-pro".to_string());

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::NvidiaNim);
    assert_eq!(resolved.api_key.as_deref(), Some("nim-key"));
    assert_eq!(resolved.base_url, "https://nim.example/v1");
    assert_eq!(resolved.model, "deepseek-ai/deepseek-v4-pro");
}

#[test]
fn nvidia_nim_provider_normalizes_flash_aliases() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let cli = CliRuntimeOverrides {
        provider: Some(ProviderKind::NvidiaNim),
        model: Some("deepseek-v4-flash".to_string()),
        ..CliRuntimeOverrides::default()
    };

    let resolved = ConfigToml::default().resolve_runtime_options(&cli);

    assert_eq!(resolved.provider, ProviderKind::NvidiaNim);
    assert_eq!(resolved.model, DEFAULT_NVIDIA_NIM_FLASH_MODEL);
}

#[test]
fn nvidia_nim_custom_env_url_does_not_inherit_ambient_credentials() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only environment mutation guarded by a module mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "nvidia-nim");
        env::set_var("NVIDIA_API_KEY", "nim-env-key");
        env::set_var("NVIDIA_NIM_BASE_URL", "https://nim-env.example/v1");
    }

    let config = ConfigToml::default();
    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::NvidiaNim);
    assert_eq!(resolved.api_key, None);
    assert_eq!(resolved.api_key_source, None);
    assert_eq!(resolved.base_url, "https://nim-env.example/v1");
    assert_eq!(resolved.model, DEFAULT_NVIDIA_NIM_MODEL);
}

#[test]
fn nvidia_nim_provider_accepts_short_nim_base_url_alias() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only environment mutation guarded by a module mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "nvidia-nim");
        env::set_var("NVIDIA_API_KEY", "nim-env-key");
        env::set_var("NIM_BASE_URL", "https://short-nim.example/v1");
    }

    let config = ConfigToml::default();
    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::NvidiaNim);
    assert_eq!(resolved.base_url, "https://short-nim.example/v1");
}

#[test]
fn nvidia_nim_provider_can_fallback_to_deepseek_api_key_env() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only environment mutation guarded by a module mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "nvidia-nim");
        env::set_var("DEEPSEEK_API_KEY", "deepseek-compat-key");
    }

    let config = ConfigToml::default();
    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::NvidiaNim);
    assert_eq!(resolved.api_key.as_deref(), Some("deepseek-compat-key"));
}

#[test]
fn list_values_redacts_root_api_key() {
    let config = ConfigToml {
        api_key: Some("sk-deepseek-secret".to_string()),
        ..ConfigToml::default()
    };

    let values = config.list_values();

    assert_eq!(
        values.get("api_key").map(String::as_str),
        Some("sk-d***cret")
    );
}

#[test]
fn list_values_fully_redacts_short_api_key() {
    let config = ConfigToml {
        api_key: Some("short-key".to_string()),
        ..ConfigToml::default()
    };

    let values = config.list_values();

    assert_eq!(values.get("api_key").map(String::as_str), Some("********"));
}

#[test]
fn get_display_value_redacts_sensitive_keys() {
    let mut config = ConfigToml {
        api_key: Some("sk-deepseek-secret".to_string()),
        ..ConfigToml::default()
    };
    config.providers.openrouter.api_key = Some("openrouter-secret-value".to_string());
    config.model = Some("deepseek-v4-pro".to_string());

    assert_eq!(
        config.get_display_value("api_key").as_deref(),
        Some("sk-d***cret")
    );
    assert_eq!(
        config
            .get_display_value("providers.openrouter.api_key")
            .as_deref(),
        Some("open***alue")
    );
    assert_eq!(
        config.get_display_value("model").as_deref(),
        Some("deepseek-v4-pro")
    );
}

#[test]
fn stream_chunk_timeout_display_defaults_to_900_for_flat_key() {
    let config = ConfigToml::default();

    assert_eq!(
        config
            .get_display_value("stream_chunk_timeout_secs")
            .as_deref(),
        Some("900")
    );
}

#[test]
fn stream_chunk_timeout_display_reads_tui_table_for_flat_key() {
    let config: ConfigToml = toml::from_str(
        r#"
        [tui]
        stream_chunk_timeout_secs = 1200
        "#,
    )
    .expect("config toml");

    assert_eq!(
        config
            .get_display_value("stream_chunk_timeout_secs")
            .as_deref(),
        Some("1200")
    );
}

#[test]
fn stream_chunk_timeout_display_supports_dotted_tui_key() {
    let config: ConfigToml = toml::from_str(
        r#"
        [tui]
        stream_chunk_timeout_secs = 1200
        "#,
    )
    .expect("config toml");

    assert_eq!(
        config
            .get_display_value("tui.stream_chunk_timeout_secs")
            .as_deref(),
        Some("1200")
    );
}

#[test]
fn stream_chunk_timeout_display_zero_maps_to_default_and_clamps() {
    let zero: ConfigToml = toml::from_str(
        r#"
        [tui]
        stream_chunk_timeout_secs = 0
        "#,
    )
    .expect("zero config toml");
    assert_eq!(
        zero.get_display_value("stream_chunk_timeout_secs")
            .as_deref(),
        Some("900")
    );

    let high: ConfigToml = toml::from_str(
        r#"
        [tui]
        stream_chunk_timeout_secs = 9999
        "#,
    )
    .expect("high config toml");
    assert_eq!(
        high.get_display_value("stream_chunk_timeout_secs")
            .as_deref(),
        Some("3600")
    );
}

#[test]
fn config_display_redacts_nested_extra_secrets() {
    let mut config = ConfigToml::default();
    let mut profile = toml::map::Map::new();
    profile.insert(
        "chatgpt_access_token".to_string(),
        toml::Value::String("raw-chatgpt-access-token-value".to_string()),
    );
    profile.insert(
        "safe_label".to_string(),
        toml::Value::String("visible".to_string()),
    );

    let mut nested = toml::map::Map::new();
    nested.insert(
        "refresh_token".to_string(),
        toml::Value::String("raw-refresh-token-value".to_string()),
    );
    nested.insert("expires_at".to_string(), toml::Value::Integer(1234));
    profile.insert("session".to_string(), toml::Value::Table(nested));

    config
        .extras
        .insert("extras".to_string(), toml::Value::Table(profile));

    let listed = config.list_values();
    let rendered = listed.get("extras").expect("extras are listed");

    assert!(rendered.contains("chatgpt_access_token"));
    assert!(rendered.contains("refresh_token"));
    assert!(rendered.contains("safe_label = \"visible\""));
    assert!(!rendered.contains("raw-chatgpt-access-token-value"));
    assert!(!rendered.contains("raw-refresh-token-value"));

    let display = config
        .get_display_value("extras")
        .expect("extras display value");
    assert!(!display.contains("raw-chatgpt-access-token-value"));
    assert!(!display.contains("raw-refresh-token-value"));
}

#[test]
fn config_display_redacts_sensitive_extra_leaf_keys_and_headers() {
    let mut config = ConfigToml::default();
    config.extras.insert(
        "chatgpt_access_token".to_string(),
        toml::Value::String("raw-chatgpt-token-value".to_string()),
    );
    config.http_headers.insert(
        "Authorization".to_string(),
        "Bearer raw-header-token".to_string(),
    );
    config
        .http_headers
        .insert("X-Test".to_string(), "ok".to_string());

    assert_eq!(
        config.get_display_value("chatgpt_access_token").as_deref(),
        Some("\"raw-***alue\"")
    );

    let headers = config
        .list_values()
        .get("http_headers")
        .expect("headers are listed")
        .clone();
    assert!(headers.contains("Authorization=Bear***oken"));
    assert!(headers.contains("X-Test=ok"));
    assert!(!headers.contains("raw-header-token"));
}

#[test]
fn hook_sinks_config_uses_separate_table_from_lifecycle_hooks() -> Result<()> {
    let raw = r#"
[hooks]
enabled = true
default_timeout_secs = 20

[[hooks.hooks]]
event = "message_submit"
command = "echo ok"

[hook_sinks]
unix_socket_path = "/tmp/cw-hooks.sock"
"#;

    let config: ConfigToml = toml::from_str(raw)?;

    assert_eq!(
        config.get_value("hook_sinks.unix_socket_path").as_deref(),
        Some("/tmp/cw-hooks.sock")
    );
    assert!(
        config.extras.contains_key("hooks"),
        "legacy lifecycle hooks table must remain an opaque extra"
    );

    let serialized = toml::to_string_pretty(&config)?;
    let round_tripped: ConfigToml = toml::from_str(&serialized)?;
    let hooks = round_tripped
        .extras
        .get("hooks")
        .and_then(toml::Value::as_table)
        .expect("hooks table preserved");

    assert_eq!(
        hooks.get("enabled").and_then(toml::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        hooks
            .get("default_timeout_secs")
            .and_then(toml::Value::as_integer),
        Some(20)
    );
    assert!(
        hooks.get("hooks").and_then(toml::Value::as_array).is_some(),
        "nested lifecycle hooks array must survive config rewrites"
    );
    assert_eq!(
        round_tripped
            .get_value("hook_sinks.unix_socket_path")
            .as_deref(),
        Some("/tmp/cw-hooks.sock")
    );

    Ok(())
}

#[test]
fn hook_sinks_unix_socket_path_round_trips_through_key_value_api() -> Result<()> {
    let mut config = ConfigToml::default();

    config.set_value("hook_sinks.unix_socket_path", "/tmp/cw-events.sock")?;

    assert_eq!(
        config.get_value("hook_sinks.unix_socket_path").as_deref(),
        Some("/tmp/cw-events.sock")
    );
    assert_eq!(
        config
            .list_values()
            .get("hook_sinks.unix_socket_path")
            .map(String::as_str),
        Some("/tmp/cw-events.sock")
    );

    config.unset_value("hook_sinks.unix_socket_path")?;
    assert_eq!(config.get_value("hook_sinks.unix_socket_path"), None);

    Ok(())
}

/// End-to-end smoke for the preferred Kimi Code setup path:
///   1. Start from a fresh root config that uses DeepSeek defaults.
///   2. Mutate it through the same key-value setters the
///      `codewhale config set providers.moonshot.*` CLI invokes.
///   3. Switch the active provider through `CODEWHALE_PROVIDER` —
///      the public env alias — without ever touching the legacy
///      `DEEPSEEK_PROVIDER` name.
///   4. Resolve the runtime and confirm the doctor/runtime values.
///
/// No real API key is required; the `api_key` here is just a
/// non-empty placeholder.
#[test]
fn moonshot_kimi_code_smoke_config_set_then_resolve() -> Result<()> {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();

    let mut config = ConfigToml {
        provider: ProviderKind::Deepseek,
        default_text_model: Some("deepseek-v4-pro".to_string()),
        ..ConfigToml::default()
    };

    // Same key paths a user would run via `codewhale config set`.
    config.set_value("providers.moonshot.api_key", "kimi-code-key-placeholder")?;
    config.set_value("providers.moonshot.auth_mode", "api_key")?;
    config.set_value("providers.moonshot.base_url", DEFAULT_KIMI_CODE_BASE_URL)?;
    config.set_value("providers.moonshot.model", DEFAULT_KIMI_CODE_MODEL)?;

    // Public env alias for the active-provider switch.
    // Safety: test-only env mutation guarded by env_lock().
    unsafe { env::set_var("CODEWHALE_PROVIDER", "moonshot") };

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Moonshot);
    assert_eq!(resolved.base_url, DEFAULT_KIMI_CODE_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_KIMI_CODE_MODEL);
    assert_eq!(resolved.auth_mode.as_deref(), Some("api_key"));
    assert_eq!(
        resolved.api_key.as_deref(),
        Some("kimi-code-key-placeholder")
    );
    assert_eq!(
        resolved.api_key_source,
        Some(RuntimeApiKeySource::ConfigFile)
    );
    Ok(())
}

#[test]
fn moonshot_provider_config_values_round_trip() -> Result<()> {
    let mut config = ConfigToml::default();

    config.set_value("providers.moonshot.api_key", "moonshot-secret-value")?;
    config.set_value("providers.moonshot.base_url", DEFAULT_KIMI_CODE_BASE_URL)?;
    config.set_value("providers.moonshot.model", DEFAULT_KIMI_CODE_MODEL)?;
    config.set_value("providers.moonshot.auth_mode", "api_key")?;
    config.set_value("providers.moonshot.http_headers", "X-Test=ok")?;

    assert_eq!(
        config
            .get_display_value("providers.moonshot.api_key")
            .as_deref(),
        Some("moon***alue")
    );
    assert_eq!(
        config.get_value("providers.moonshot.base_url").as_deref(),
        Some(DEFAULT_KIMI_CODE_BASE_URL)
    );
    assert_eq!(
        config.get_value("providers.moonshot.model").as_deref(),
        Some(DEFAULT_KIMI_CODE_MODEL)
    );
    assert_eq!(
        config.get_value("providers.moonshot.auth_mode").as_deref(),
        Some("api_key")
    );
    assert_eq!(
        config
            .list_values()
            .get("providers.moonshot.api_key")
            .map(String::as_str),
        Some("moon***alue")
    );

    config.unset_value("providers.moonshot.auth_mode")?;
    config.unset_value("providers.moonshot.base_url")?;
    config.unset_value("providers.moonshot.model")?;

    assert_eq!(config.get_value("providers.moonshot.auth_mode"), None);
    assert_eq!(config.get_value("providers.moonshot.base_url"), None);
    assert_eq!(config.get_value("providers.moonshot.model"), None);
    Ok(())
}

#[test]
fn siliconflow_cn_provider_config_values_round_trip() -> Result<()> {
    let mut config = ConfigToml::default();

    config.set_value("providers.siliconflow_cn.api_key", "sf-cn-secret-value")?;
    config.set_value(
        "providers.siliconflow_cn.base_url",
        DEFAULT_SILICONFLOW_CN_BASE_URL,
    )?;
    config.set_value("providers.siliconflow_cn.model", DEFAULT_SILICONFLOW_MODEL)?;
    config.set_value("providers.siliconflow_cn.http_headers", "X-Test=ok")?;

    assert_eq!(
        config
            .get_display_value("providers.siliconflow_cn.api_key")
            .as_deref(),
        Some("sf-c***alue")
    );
    assert_eq!(
        config
            .get_value("providers.siliconflow_cn.base_url")
            .as_deref(),
        Some(DEFAULT_SILICONFLOW_CN_BASE_URL)
    );
    assert_eq!(
        config
            .get_value("providers.siliconflow_cn.model")
            .as_deref(),
        Some(DEFAULT_SILICONFLOW_MODEL)
    );
    assert_eq!(
        config
            .list_values()
            .get("providers.siliconflow_cn.api_key")
            .map(String::as_str),
        Some("sf-c***alue")
    );

    config.unset_value("providers.siliconflow_cn.api_key")?;
    config.unset_value("providers.siliconflow_cn.base_url")?;
    config.unset_value("providers.siliconflow_cn.model")?;
    config.unset_value("providers.siliconflow_cn.http_headers")?;

    assert_eq!(config.get_value("providers.siliconflow_cn.api_key"), None);
    assert_eq!(config.get_value("providers.siliconflow_cn.base_url"), None);
    assert_eq!(config.get_value("providers.siliconflow_cn.model"), None);
    assert_eq!(
        config.get_value("providers.siliconflow_cn.http_headers"),
        None
    );
    Ok(())
}

#[test]
fn volcengine_provider_config_values_round_trip() -> Result<()> {
    let mut config = ConfigToml::default();

    config.set_value("providers.volcengine.api_key", "volcengine-secret-value")?;
    config.set_value("providers.volcengine.base_url", DEFAULT_VOLCENGINE_BASE_URL)?;
    config.set_value("providers.volcengine.model", DEFAULT_VOLCENGINE_MODEL)?;
    config.set_value("providers.volcengine.http_headers", "X-Test=ok")?;

    assert_eq!(
        config
            .get_display_value("providers.volcengine.api_key")
            .as_deref(),
        Some("volc***alue")
    );
    assert_eq!(
        config.get_value("providers.volcengine.base_url").as_deref(),
        Some(DEFAULT_VOLCENGINE_BASE_URL)
    );
    assert_eq!(
        config.get_value("providers.volcengine.model").as_deref(),
        Some(DEFAULT_VOLCENGINE_MODEL)
    );
    assert_eq!(
        config
            .get_value("providers.volcengine.http_headers")
            .as_deref(),
        Some("X-Test=ok")
    );
    assert_eq!(
        config
            .list_values()
            .get("providers.volcengine.http_headers")
            .map(String::as_str),
        Some("X-Test=ok")
    );

    config.unset_value("providers.volcengine.http_headers")?;
    assert_eq!(config.get_value("providers.volcengine.http_headers"), None);
    Ok(())
}

#[test]
fn provider_key_value_api_covers_all_provider_metadata_entries() -> Result<()> {
    for provider in ProviderKind::ALL {
        let table = provider.provider().provider_config_key();
        let mut config = ConfigToml::default();
        let api_key = format!("secret-value-for-{table}-123456");
        let api_key_path = format!("providers.{table}.api_key");
        let base_url_path = format!("providers.{table}.base_url");
        let model_path = format!("providers.{table}.model");
        let context_window_path = format!("providers.{table}.context_window");
        let headers_path = format!("providers.{table}.http_headers");
        let mode_path = format!("providers.{table}.mode");
        let auth_mode_path = format!("providers.{table}.auth_mode");
        let insecure_path = format!("providers.{table}.insecure_skip_tls_verify");
        let path_suffix_path = format!("providers.{table}.path_suffix");

        config.set_value(&api_key_path, &api_key)?;
        config.set_value(&base_url_path, "https://gateway.example/v1")?;
        config.set_value(&model_path, "provider-test-model")?;
        config.set_value(&context_window_path, "1000000")?;
        config.set_value(&headers_path, "X-Test=ok")?;
        config.set_value(&mode_path, "concise")?;
        config.set_value(&auth_mode_path, "api_key")?;
        config.set_value(&insecure_path, "true")?;
        config.set_value(&path_suffix_path, "/chat/completions")?;

        assert_eq!(
            config.get_value(&api_key_path).as_deref(),
            Some(api_key.as_str())
        );
        assert_eq!(
            config.get_value(&base_url_path).as_deref(),
            Some("https://gateway.example/v1")
        );
        assert_eq!(
            config.get_value(&model_path).as_deref(),
            Some("provider-test-model")
        );
        assert_eq!(
            config.get_value(&context_window_path).as_deref(),
            Some("1000000")
        );
        assert_eq!(
            config.get_value(&headers_path).as_deref(),
            Some("X-Test=ok")
        );
        assert_eq!(config.get_value(&mode_path).as_deref(), Some("concise"));
        assert_eq!(
            config.get_value(&auth_mode_path).as_deref(),
            Some("api_key")
        );
        assert_eq!(config.get_value(&insecure_path).as_deref(), Some("true"));
        assert_eq!(
            config.get_value(&path_suffix_path).as_deref(),
            Some("/chat/completions")
        );

        let listed = config.list_values();
        let listed_api_key = listed
            .get(&api_key_path)
            .expect("provider API key is listed");
        assert!(listed_api_key.contains("***"));
        assert_ne!(listed_api_key, &api_key);
        assert_eq!(
            listed.get(&headers_path).map(String::as_str),
            Some("X-Test=ok")
        );
        assert_eq!(
            listed.get(&context_window_path).map(String::as_str),
            Some("1000000")
        );
        assert_eq!(listed.get(&insecure_path).map(String::as_str), Some("true"));

        config.unset_value(&api_key_path)?;
        config.unset_value(&base_url_path)?;
        config.unset_value(&model_path)?;
        config.unset_value(&context_window_path)?;
        config.unset_value(&headers_path)?;
        config.unset_value(&mode_path)?;
        config.unset_value(&auth_mode_path)?;
        config.unset_value(&insecure_path)?;
        config.unset_value(&path_suffix_path)?;

        assert_eq!(config.get_value(&api_key_path), None);
        assert_eq!(config.get_value(&base_url_path), None);
        assert_eq!(config.get_value(&model_path), None);
        assert_eq!(config.get_value(&context_window_path), None);
        assert_eq!(config.get_value(&headers_path), None);
        assert_eq!(config.get_value(&mode_path), None);
        assert_eq!(config.get_value(&auth_mode_path), None);
        assert_eq!(config.get_value(&insecure_path), None);
        assert_eq!(config.get_value(&path_suffix_path), None);

        if provider == ProviderKind::Deepseek {
            assert_eq!(config.api_key, None);
            assert_eq!(config.base_url, None);
            assert_eq!(config.default_text_model, None);
            assert!(config.http_headers.is_empty());
        }
    }

    Ok(())
}

#[test]
fn provider_context_window_rejects_zero() {
    let mut config = ConfigToml::default();
    let err = config
        .set_value("providers.openai.context_window", "0")
        .expect_err("zero context window should be rejected");

    assert!(err.to_string().contains("greater than 0"));
}

#[test]
fn project_merge_denies_credentials_endpoints_and_provider_selection() {
    let mut base = ConfigToml {
        provider: ProviderKind::Deepseek,
        api_key: Some("user-key".to_string()),
        base_url: Some("https://api.deepseek.com".to_string()),
        default_text_model: Some("deepseek-v4-flash".to_string()),
        ..ConfigToml::default()
    };
    base.providers.openrouter.api_key = Some("user-openrouter-key".to_string());
    base.providers.openrouter.path_suffix = Some("/chat/completions".to_string());

    let mut project = ConfigToml {
        provider: ProviderKind::Openrouter,
        api_key: Some("attacker-key".to_string()),
        base_url: Some("https://evil.example/v1".to_string()),
        default_text_model: Some("deepseek-v4-pro".to_string()),
        auth_mode: Some("oauth".to_string()),
        telemetry: Some(true),
        ..ConfigToml::default()
    };
    project.providers.openrouter.api_key = Some("attacker-openrouter-key".to_string());
    project.providers.openrouter.base_url = Some("https://evil.example/openrouter".to_string());
    project.providers.openrouter.insecure_skip_tls_verify = Some(true);
    project.providers.openrouter.path_suffix = Some("/attacker/chat".to_string());
    project.providers.openrouter.model = Some("deepseek/deepseek-v4-pro".to_string());
    project.providers.volcengine.model = Some("DeepSeek-V4-Pro".to_string());
    project.providers.moonshot.model = Some("kimi-k2.6".to_string());

    base.merge_project_overrides(project);

    assert_eq!(base.provider, ProviderKind::Deepseek);
    assert_eq!(base.api_key.as_deref(), Some("user-key"));
    assert_eq!(base.base_url.as_deref(), Some("https://api.deepseek.com"));
    assert_eq!(base.auth_mode, None);
    assert_eq!(base.telemetry, None);
    assert_eq!(
        base.providers.openrouter.api_key.as_deref(),
        Some("user-openrouter-key")
    );
    assert_eq!(base.providers.openrouter.base_url, None);
    assert_eq!(base.providers.openrouter.insecure_skip_tls_verify, None);
    assert_eq!(
        base.providers.openrouter.path_suffix.as_deref(),
        Some("/chat/completions")
    );
    assert_eq!(base.default_text_model.as_deref(), Some("deepseek-v4-pro"));
    assert_eq!(
        base.providers.openrouter.model.as_deref(),
        Some("deepseek/deepseek-v4-pro")
    );
    assert_eq!(
        base.providers.volcengine.model.as_deref(),
        Some("DeepSeek-V4-Pro")
    );
    assert_eq!(base.providers.moonshot.model.as_deref(), Some("kimi-k2.6"));
}

#[test]
fn project_merge_forwards_all_provider_model_overrides() {
    let mut project_toml = String::new();
    for provider in ProviderKind::ALL {
        let key = provider.provider().provider_config_key();
        project_toml.push_str(&format!(
            "[providers.{key}]\nmodel = \"project-{key}-model\"\n\n"
        ));
    }

    let project: ConfigToml =
        toml::from_str(&project_toml).expect("project provider overrides parse");
    let mut base = ConfigToml::default();

    base.merge_project_overrides(project);

    for provider in ProviderKind::ALL {
        let key = provider.provider().provider_config_key();
        let expected = format!("project-{key}-model");
        assert_eq!(
            base.providers.for_provider(provider).model.as_deref(),
            Some(expected.as_str()),
            "provider {key} should merge repo-local model override"
        );
    }
}

#[test]
fn project_merge_does_not_replace_user_hotbar_bindings() {
    let mut base = ConfigToml {
        hotbar: Some(vec![HotbarBindingToml {
            slot: 1,
            action: "mode.plan".to_string(),
            label: Some("Plan".to_string()),
        }]),
        ..ConfigToml::default()
    };
    let project = ConfigToml {
        hotbar: Some(vec![HotbarBindingToml {
            slot: 1,
            action: "mode.yolo".to_string(),
            label: Some("Yolo".to_string()),
        }]),
        ..ConfigToml::default()
    };

    base.merge_project_overrides(project);

    assert_eq!(
        base.hotbar,
        Some(vec![HotbarBindingToml {
            slot: 1,
            action: "mode.plan".to_string(),
            label: Some("Plan".to_string()),
        }])
    );
}

#[test]
fn project_merge_only_tightens_approval_and_sandbox_policy() {
    let mut strict = ConfigToml {
        approval_policy: Some("never".to_string()),
        sandbox_mode: Some("read-only".to_string()),
        ..ConfigToml::default()
    };
    strict.merge_project_overrides(ConfigToml {
        approval_policy: Some("on-request".to_string()),
        sandbox_mode: Some("workspace-write".to_string()),
        ..ConfigToml::default()
    });
    assert_eq!(strict.approval_policy.as_deref(), Some("never"));
    assert_eq!(strict.sandbox_mode.as_deref(), Some("read-only"));

    let mut permissive = ConfigToml {
        approval_policy: Some("auto".to_string()),
        sandbox_mode: Some("workspace-write".to_string()),
        ..ConfigToml::default()
    };
    permissive.merge_project_overrides(ConfigToml {
        approval_policy: Some("never".to_string()),
        sandbox_mode: Some("read-only".to_string()),
        ..ConfigToml::default()
    });
    assert_eq!(permissive.approval_policy.as_deref(), Some("never"));
    assert_eq!(permissive.sandbox_mode.as_deref(), Some("read-only"));

    let mut unset = ConfigToml::default();
    unset.merge_project_overrides(ConfigToml {
        approval_policy: Some("on-request".to_string()),
        sandbox_mode: Some("workspace-write".to_string()),
        ..ConfigToml::default()
    });
    assert_eq!(unset.approval_policy, None);
    assert_eq!(unset.sandbox_mode, None);
}

#[test]
fn list_values_redacts_unicode_api_key_without_byte_slicing() {
    let config = ConfigToml {
        api_key: Some("密钥密钥密钥密钥123456789".to_string()),
        ..ConfigToml::default()
    };

    let values = config.list_values();

    assert_eq!(
        values.get("api_key").map(String::as_str),
        Some("密钥密钥***6789")
    );
}

#[test]
fn app_homes_prefer_home_env_before_platform_home_fallback() {
    let _lock = env_lock();
    let home =
        std::env::temp_dir().join(format!("codewhale-config-home-env-{}", std::process::id()));
    let userprofile = std::env::temp_dir().join(format!(
        "codewhale-config-userprofile-{}",
        std::process::id()
    ));
    let _env = StateEnvRestore {
        home: env::var_os("HOME"),
        userprofile: env::var_os("USERPROFILE"),
        codewhale_home: env::var_os("CODEWHALE_HOME"),
    };
    // Safety: test-only environment mutation is serialized by env_lock().
    unsafe {
        env::set_var("HOME", &home);
        env::set_var("USERPROFILE", &userprofile);
        env::remove_var("CODEWHALE_HOME");
    }

    assert_eq!(
        codewhale_home().expect("codewhale home"),
        home.join(CODEWHALE_APP_DIR)
    );
    assert_eq!(
        legacy_deepseek_home().expect("legacy home"),
        home.join(LEGACY_APP_DIR)
    );

    let explicit = std::env::temp_dir().join(format!(
        "codewhale-config-explicit-home-{}",
        std::process::id()
    ));
    // Safety: test-only environment mutation is serialized by env_lock().
    unsafe {
        env::set_var("CODEWHALE_HOME", &explicit);
    }
    assert_eq!(codewhale_home().expect("explicit home"), explicit);
}

#[test]
fn migrate_config_reports_copied_legacy_path() {
    let _lock = env_lock();
    struct LegacyConfigGuard {
        path: PathBuf,
        original: Option<Vec<u8>>,
    }

    impl LegacyConfigGuard {
        fn install(path: PathBuf, contents: &[u8]) -> Self {
            let original = fs::read(&path).ok();
            fs::create_dir_all(path.parent().expect("legacy config parent")).expect("legacy dir");
            fs::write(&path, contents).expect("legacy config");
            Self { path, original }
        }
    }

    impl Drop for LegacyConfigGuard {
        fn drop(&mut self) {
            if let Some(original) = self.original.take() {
                let _ = fs::write(&self.path, original);
            } else {
                let _ = fs::remove_file(&self.path);
                if let Some(parent) = self.path.parent() {
                    let _ = fs::remove_dir(parent);
                }
            }
        }
    }

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let home = std::env::temp_dir().join(format!(
        "codewhale-config-migration-{}-{unique}",
        std::process::id()
    ));
    let legacy_dir = home.join(LEGACY_APP_DIR);
    let primary_dir = home.join(CODEWHALE_APP_DIR);
    let legacy_config = legacy_dir.join(CONFIG_FILE_NAME);
    let _legacy = LegacyConfigGuard::install(legacy_config.clone(), b"provider = \"deepseek\"\n");

    let _env = StateEnvRestore {
        home: env::var_os("HOME"),
        userprofile: env::var_os("USERPROFILE"),
        codewhale_home: env::var_os("CODEWHALE_HOME"),
    };
    // Safety: test-only environment mutation is serialized by env_lock().
    unsafe {
        env::set_var("HOME", &home);
        env::set_var("USERPROFILE", &home);
        env::remove_var("CODEWHALE_HOME");
    }

    let migration = migrate_config_if_needed()
        .expect("migration")
        .expect("legacy config should be copied");

    assert_eq!(migration.legacy_path, legacy_config);
    assert_eq!(migration.primary_path, primary_dir.join(CONFIG_FILE_NAME));
    let notice = migration.user_notice();
    assert!(notice.contains(&legacy_dir.join(CONFIG_FILE_NAME).display().to_string()));
    assert!(notice.contains(&primary_dir.join(CONFIG_FILE_NAME).display().to_string()));
    assert!(notice.contains(".codewhale path for future edits"));
    assert!(notice.contains(".deepseek file remains only as a compatibility fallback"));
    assert_eq!(
        fs::read_to_string(primary_dir.join(CONFIG_FILE_NAME)).expect("primary config"),
        "provider = \"deepseek\"\n"
    );

    let _ = fs::remove_dir_all(home);
}

#[test]
fn explicit_codewhale_home_bypasses_legacy_config_fallback_and_migration() {
    let _lock = env_lock();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let home = std::env::temp_dir().join(format!(
        "codewhale-config-explicit-isolation-{}-{unique}",
        std::process::id()
    ));
    let legacy_config = home.join(LEGACY_APP_DIR).join(CONFIG_FILE_NAME);
    fs::create_dir_all(legacy_config.parent().expect("legacy config parent")).expect("legacy dir");
    fs::write(&legacy_config, b"provider = \"deepseek\"\n").expect("legacy config");

    let explicit_home = home.join("isolated-codewhale");
    let _env = StateEnvRestore {
        home: env::var_os("HOME"),
        userprofile: env::var_os("USERPROFILE"),
        codewhale_home: env::var_os("CODEWHALE_HOME"),
    };
    // Safety: test-only environment mutation is serialized by env_lock().
    unsafe {
        env::set_var("HOME", &home);
        env::set_var("USERPROFILE", &home);
        env::set_var("CODEWHALE_HOME", &explicit_home);
    }

    assert_eq!(
        default_config_path().expect("default config path"),
        explicit_home.join(CONFIG_FILE_NAME),
        "explicit CODEWHALE_HOME must not read ambient legacy config"
    );
    assert!(
        migrate_config_if_needed()
            .expect("migration check")
            .is_none(),
        "explicit CODEWHALE_HOME must not migrate ambient legacy config"
    );
    assert!(
        !explicit_home.join(CONFIG_FILE_NAME).exists(),
        "legacy config must not be copied into explicit CODEWHALE_HOME"
    );

    let _ = fs::remove_dir_all(home);
}

// ── ensure_state_dir legacy migration (#3240) ───────────────────────

/// Saves and restores the env vars that the state-resolvers read.
struct StateEnvRestore {
    home: Option<OsString>,
    userprofile: Option<OsString>,
    codewhale_home: Option<OsString>,
}

impl Drop for StateEnvRestore {
    fn drop(&mut self) {
        // Safety: test-only environment mutation is serialized by env_lock().
        unsafe {
            match self.home.take() {
                Some(value) => env::set_var("HOME", value),
                None => env::remove_var("HOME"),
            }
            match self.userprofile.take() {
                Some(value) => env::set_var("USERPROFILE", value),
                None => env::remove_var("USERPROFILE"),
            }
            match self.codewhale_home.take() {
                Some(value) => env::set_var("CODEWHALE_HOME", value),
                None => env::remove_var("CODEWHALE_HOME"),
            }
        }
    }
}

/// Points `HOME`/`USERPROFILE` at a fresh temp tree and clears
/// `CODEWHALE_HOME` so `codewhale_home()` -> `<home>/.codewhale` and
/// `legacy_deepseek_home()` -> `<home>/.deepseek`. Env is restored on drop.
struct StateDirEnv {
    home: PathBuf,
    _restore: StateEnvRestore,
}

impl StateDirEnv {
    fn install(unique: u128) -> Self {
        let home = std::env::temp_dir().join(format!(
            "codewhale-state-migration-{}-{unique}",
            std::process::id()
        ));
        let restore = StateEnvRestore {
            home: env::var_os("HOME"),
            userprofile: env::var_os("USERPROFILE"),
            codewhale_home: env::var_os("CODEWHALE_HOME"),
        };
        // Safety: test-only environment mutation is serialized by env_lock().
        unsafe {
            env::set_var("HOME", &home);
            env::set_var("USERPROFILE", &home);
            env::remove_var("CODEWHALE_HOME");
        }
        Self {
            home,
            _restore: restore,
        }
    }
    fn legacy(&self, sub: &str) -> PathBuf {
        self.home.join(LEGACY_APP_DIR).join(sub)
    }
    fn primary(&self, sub: &str) -> PathBuf {
        self.home.join(CODEWHALE_APP_DIR).join(sub)
    }
}

#[test]
fn ensure_state_dir_relocates_legacy_subdir_on_first_write() {
    let _lock = env_lock();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let state_env = StateDirEnv::install(unique);
    // Seed a legacy subdir; primary must not exist yet.
    fs::create_dir_all(state_env.legacy("slop_ledger")).expect("legacy dir");
    fs::write(
        state_env.legacy("slop_ledger").join("slop_ledger.json"),
        b"legacy",
    )
    .expect("legacy file");
    assert!(!state_env.primary("slop_ledger").exists());

    let (dir, migration) =
        ensure_state_dir_with_migration("slop_ledger").expect("ensure_state_dir");
    assert_eq!(dir, state_env.primary("slop_ledger"));
    let migration = migration.expect("legacy migration should be reported");
    assert_eq!(migration.kind, StateMigrationKind::Relocated);
    assert_eq!(migration.subdir, "slop_ledger");
    assert_eq!(migration.legacy_path, state_env.legacy("slop_ledger"));
    assert_eq!(migration.primary_path, state_env.primary("slop_ledger"));
    // Legacy contents relocated into primary.
    assert_eq!(
        fs::read_to_string(state_env.primary("slop_ledger").join("slop_ledger.json"))
            .expect("migrated file"),
        "legacy"
    );
    // The legacy subdir was relocated (moved), so .deepseek stops growing.
    assert!(
        !state_env.legacy("slop_ledger").exists(),
        "legacy subdir should be removed after relocation"
    );
    // Idempotent: a second call is a no-op now that primary exists.
    let (_, repeated_migration) =
        ensure_state_dir_with_migration("slop_ledger").expect("idempotent ensure");
    assert!(repeated_migration.is_none());
    let _ = fs::remove_dir_all(&state_env.home);
}

#[test]
fn state_migration_notice_explains_preserved_data_and_canonical_root() {
    let migration = StateMigration {
        subdir: "sessions".to_string(),
        legacy_path: PathBuf::from("/home/alice/.deepseek/sessions"),
        primary_path: PathBuf::from("/home/alice/.codewhale/sessions"),
        kind: StateMigrationKind::Relocated,
    };

    let notice = migration.user_notice();

    assert!(notice.contains("Codewhale migrated legacy state"));
    assert!(notice.contains("/home/alice/.deepseek/sessions"));
    assert!(notice.contains("/home/alice/.codewhale/sessions"));
    assert!(notice.contains("Your data was preserved"));
    assert!(notice.contains("Use .codewhale as the canonical state location"));
    assert!(notice.contains("remove the legacy .deepseek tree"));
}

#[test]
fn copied_state_migration_notice_says_legacy_copy_remains() {
    let migration = StateMigration {
        subdir: "catalog".to_string(),
        legacy_path: PathBuf::from("/home/alice/.deepseek/catalog"),
        primary_path: PathBuf::from("/home/alice/.codewhale/catalog"),
        kind: StateMigrationKind::Copied,
    };

    let notice = migration.user_notice();

    assert!(notice.contains("copied"));
    assert!(notice.contains("legacy .deepseek copy was left in place"));
}

#[test]
fn ensure_state_dir_writes_to_primary_when_both_exist() {
    let _lock = env_lock();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let state_env = StateDirEnv::install(unique);
    // Migrated user: primary already exists; a legacy orphan also remains.
    fs::create_dir_all(state_env.primary("sessions")).expect("primary dir");
    fs::write(state_env.primary("sessions").join("a.json"), b"primary").expect("primary file");
    fs::create_dir_all(state_env.legacy("sessions")).expect("legacy dir");
    fs::write(state_env.legacy("sessions").join("old.json"), b"legacy").expect("legacy file");

    let (dir, migration) = ensure_state_dir_with_migration("sessions").expect("ensure_state_dir");
    assert_eq!(dir, state_env.primary("sessions"));
    assert!(
        migration.is_none(),
        "existing primary must not emit a migration event"
    );
    // Primary untouched; legacy orphan left as-is (not migrated, not deleted).
    assert_eq!(
        fs::read_to_string(state_env.primary("sessions").join("a.json")).expect("primary"),
        "primary"
    );
    assert!(
        state_env.legacy("sessions").exists(),
        "existing legacy orphan must not be deleted when primary exists"
    );
    let _ = fs::remove_dir_all(&state_env.home);
}

#[test]
fn resolve_state_dir_still_finds_legacy_for_backfill() {
    let _lock = env_lock();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let state_env = StateDirEnv::install(unique);
    // Only legacy exists -> read resolver returns legacy (backfill).
    fs::create_dir_all(state_env.legacy("catalog")).expect("legacy dir");
    assert_eq!(
        resolve_state_dir("catalog").expect("resolve"),
        state_env.legacy("catalog")
    );
    // After the primary is created (e.g. via a write), the read resolver
    // returns primary — legacy is reachable only while primary is absent.
    ensure_state_dir("catalog").expect("ensure");
    assert_eq!(
        resolve_state_dir("catalog").expect("resolve after migrate"),
        state_env.primary("catalog")
    );
    let _ = fs::remove_dir_all(&state_env.home);
}

#[test]
fn explicit_codewhale_home_bypasses_legacy_state_fallback_and_migration() {
    let _lock = env_lock();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let state_env = StateDirEnv::install(unique);
    let explicit_home = state_env.home.join("isolated-codewhale");
    // Safety: test-only environment mutation is serialized by env_lock().
    unsafe {
        env::set_var("CODEWHALE_HOME", &explicit_home);
    }
    fs::create_dir_all(state_env.legacy("catalog")).expect("legacy dir");
    fs::write(state_env.legacy("catalog").join("legacy.json"), b"legacy").expect("legacy file");

    let primary = explicit_home.join("catalog");
    assert_eq!(
        resolve_state_dir("catalog").expect("resolve"),
        primary,
        "explicit CODEWHALE_HOME must not read ambient legacy state"
    );

    let ensured = ensure_state_dir("catalog").expect("ensure");
    assert_eq!(ensured, primary);
    assert!(
        state_env.legacy("catalog").join("legacy.json").exists(),
        "explicit CODEWHALE_HOME must not migrate ambient legacy state"
    );
    assert!(
        !primary.join("legacy.json").exists(),
        "legacy contents must not be copied into an explicit CODEWHALE_HOME"
    );
    let _ = fs::remove_dir_all(&state_env.home);
}

#[test]
fn state_resolvers_reject_path_traversal_subdirs() {
    // Defense against path injection (#3240 hardening): the public state
    // resolvers must refuse subdirs that could escape the state root.
    for bad in ["..", "../secret", "/etc", "a/../../b"] {
        let err = ensure_state_dir(bad)
            .err()
            .unwrap_or_else(|| panic!("expected {bad:?} to be rejected"));
        assert!(
            format!("{err:#}").contains("state subdir"),
            "expected rejection of {bad:?}, got {err:#}"
        );
        assert!(
            resolve_state_dir(bad).is_err(),
            "read resolver must also reject {bad:?}"
        );
    }
    // Safe values are accepted (including the root sentinel ".").
    assert!(ensure_safe_state_subdir(".").is_ok());
    assert!(ensure_safe_state_subdir("sessions").is_ok());
    assert!(ensure_safe_state_subdir("a/b").is_ok());
    assert!(ensure_safe_state_subdir("").is_err());
}

#[test]
fn project_state_resolvers_reject_path_traversal_subdirs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = dir.path().join("workspace");
    fs::create_dir_all(&workspace).expect("workspace");

    for bad in ["..", "../secret", "/etc", "a/../../b"] {
        let err = resolve_project_state_dir(&workspace, bad)
            .err()
            .unwrap_or_else(|| panic!("expected {bad:?} to be rejected"));
        assert!(
            format!("{err:#}").contains("state subdir"),
            "expected rejection of {bad:?}, got {err:#}"
        );
        assert!(
            ensure_project_state_dir(&workspace, bad).is_err(),
            "write resolver must also reject {bad:?}"
        );
    }

    let canonical_workspace = workspace.canonicalize().expect("canonical workspace");
    let safe = resolve_project_state_dir(&workspace, "notes.md")
        .expect("safe project state subdir should resolve")
        .1;
    assert_eq!(
        safe,
        canonical_workspace.join(LEGACY_APP_DIR).join("notes.md")
    );
    let created =
        ensure_project_state_dir(&workspace, "a/b").expect("safe nested project state dir");
    assert_eq!(
        created,
        canonical_workspace.join(CODEWHALE_APP_DIR).join("a/b")
    );
}

#[test]
fn project_state_resolvers_reject_workspace_traversal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = dir.path().join("workspace");
    fs::create_dir_all(&workspace).expect("workspace");
    let bad_workspace = workspace.join("..").join("outside");

    let err = resolve_project_state_dir(&bad_workspace, "notes.md")
        .expect_err("workspace traversal should fail");
    assert!(format!("{err:#}").contains("project workspace path"));
    assert!(ensure_project_state_dir(&bad_workspace, "state").is_err());
}

#[test]
fn normalize_config_file_path_rejects_traversal() {
    let err = normalize_config_file_path(PathBuf::from("../config.toml"))
        .expect_err("traversal path should fail");
    assert!(format!("{err:#}").contains("cannot contain '..'"));
}

#[test]
fn config_store_save_revalidates_path_before_parent_creation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let outside_dir = dir.path().join("outside");
    let traversal_path = dir
        .path()
        .join("allowed")
        .join("..")
        .join("outside")
        .join(CONFIG_FILE_NAME);
    let store = ConfigStore {
        path: traversal_path,
        config: ConfigToml::default(),
        permissions: PermissionsToml::default(),
        original_raw: None,
    };

    let err = store
        .save()
        .expect_err("save should reject traversal before creating parents");

    assert!(format!("{err:#}").contains("cannot contain '..'"));
    assert!(
        !outside_dir.exists(),
        "save must not create directories from an unvalidated path"
    );
}

#[test]
fn resolve_config_path_rejects_env_traversal() {
    let _lock = env_lock();
    struct ConfigPathEnvGuard {
        codewhale: Option<OsString>,
        deepseek: Option<OsString>,
    }
    impl Drop for ConfigPathEnvGuard {
        fn drop(&mut self) {
            // Safety: test-only environment mutation is serialized by env_lock().
            unsafe {
                match self.codewhale.as_ref() {
                    Some(value) => env::set_var("CODEWHALE_CONFIG_PATH", value),
                    None => env::remove_var("CODEWHALE_CONFIG_PATH"),
                }
                match self.deepseek.as_ref() {
                    Some(value) => env::set_var("DEEPSEEK_CONFIG_PATH", value),
                    None => env::remove_var("DEEPSEEK_CONFIG_PATH"),
                }
            }
        }
    }
    let _guard = ConfigPathEnvGuard {
        codewhale: env::var_os("CODEWHALE_CONFIG_PATH"),
        deepseek: env::var_os("DEEPSEEK_CONFIG_PATH"),
    };

    // Safety: test-only environment mutation is serialized by env_lock().
    unsafe {
        env::set_var("CODEWHALE_CONFIG_PATH", "../config.toml");
        env::remove_var("DEEPSEEK_CONFIG_PATH");
    }

    let err = resolve_config_path(None).expect_err("env traversal should fail");
    assert!(format!("{err:#}").contains("cannot contain '..'"));
}

#[cfg(unix)]
#[test]
fn normalize_config_file_path_rejects_symlink_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("target.toml");
    let link = dir.path().join(CONFIG_FILE_NAME);
    fs::write(&target, "model = \"deepseek-v4-flash\"\n").expect("write target");
    std::os::unix::fs::symlink(&target, &link).expect("symlink config");

    let err = normalize_config_file_path(link).expect_err("symlink config should fail");
    assert!(format!("{err:#}").contains("must not be a symlink"));
}

#[cfg(unix)]
#[test]
fn load_project_config_rejects_symlinked_primary_config() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    let primary_dir = workspace.path().join(CODEWHALE_APP_DIR);
    let legacy_dir = workspace.path().join(LEGACY_APP_DIR);
    fs::create_dir_all(&primary_dir).expect("mkdir primary");
    fs::create_dir_all(&legacy_dir).expect("mkdir legacy");
    let outside_config = outside.path().join(CONFIG_FILE_NAME);
    fs::write(&outside_config, "model = \"outside-model\"\n").expect("write outside config");
    fs::write(
        legacy_dir.join(CONFIG_FILE_NAME),
        "model = \"legacy-model\"\n",
    )
    .expect("write legacy config");
    std::os::unix::fs::symlink(&outside_config, primary_dir.join(CONFIG_FILE_NAME))
        .expect("symlink project config");

    let loaded = load_project_config(workspace.path());

    assert!(
        loaded.is_none(),
        "symlinked primary project config should stop the project overlay"
    );
}

#[cfg(unix)]
#[test]
fn load_sibling_permissions_rejects_symlink_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join(CONFIG_FILE_NAME);
    let outside = dir.path().join("outside-permissions.toml");
    let permissions_link = dir.path().join(PERMISSIONS_FILE_NAME);
    fs::write(&config_path, "model = \"deepseek-v4-flash\"\n").expect("write config");
    fs::write(&outside, "").expect("write outside permissions");
    std::os::unix::fs::symlink(&outside, &permissions_link).expect("symlink permissions");

    let err = load_sibling_permissions(&config_path).expect_err("symlink permissions should fail");
    assert!(format!("{err:#}").contains("must not be a symlink"));
}

#[cfg(unix)]
#[test]
fn append_ask_rules_rejects_symlinked_permissions_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join(CONFIG_FILE_NAME);
    let outside = dir.path().join("outside-permissions.toml");
    let permissions_link = dir.path().join(PERMISSIONS_FILE_NAME);
    fs::write(&config_path, "model = \"deepseek-v4-flash\"\n").expect("write config");
    fs::write(&outside, "").expect("write outside permissions");
    let mut store = ConfigStore::load(Some(config_path)).expect("load store before link");
    std::os::unix::fs::symlink(&outside, &permissions_link).expect("symlink permissions");

    let err = store
        .append_ask_rules(&[ToolAskRule::exec_shell("cargo test")])
        .expect_err("symlink permissions should fail");

    assert!(format!("{err:#}").contains("must not be a symlink"));
    assert_eq!(
        fs::read_to_string(&outside).expect("read outside permissions"),
        ""
    );
}

#[cfg(unix)]
#[test]
fn write_config_backup_rejects_symlink_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join(CONFIG_FILE_NAME);
    let outside = dir.path().join("outside-backup.toml");
    let backup_link = config_backup_path(&config_path);
    fs::write(&config_path, "model = \"deepseek-v4-flash\"\n").expect("write config");
    fs::write(&outside, "").expect("write outside backup");
    std::os::unix::fs::symlink(&outside, &backup_link).expect("symlink backup");

    let err = write_one_time_config_backup(&config_path).expect_err("symlink backup should fail");
    assert!(format!("{err:#}").contains("must not be a symlink"));
}

#[cfg(unix)]
#[test]
fn save_clamps_existing_config_permissions() {
    use std::time::{SystemTime, UNIX_EPOCH};

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "deepseek-config-perms-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("mkdir");
    let path = dir.join(CONFIG_FILE_NAME);
    fs::write(&path, "api_key = \"old\"\n").expect("seed config");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("chmod seed");

    let store = ConfigStore {
        path: path.clone(),
        config: ConfigToml {
            api_key: Some("new-secret".to_string()),
            ..ConfigToml::default()
        },
        permissions: PermissionsToml::default(),
        original_raw: None,
    };
    store.save().expect("save");

    let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn config_store_save_skips_identical_serialized_body() {
    use std::time::{SystemTime, UNIX_EPOCH};

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "codewhale-config-noop-save-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("mkdir");
    let path = dir.join(CONFIG_FILE_NAME);
    let config = ConfigToml {
        model: Some("deepseek-v4-flash".to_string()),
        ..ConfigToml::default()
    };
    let body = toml::to_string_pretty(&config).expect("serialize");
    fs::write(&path, &body).expect("seed config");
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).expect("chmod seed");

    let store = ConfigStore {
        path: path.clone(),
        config,
        permissions: PermissionsToml::default(),
        original_raw: None,
    };
    store.save().expect("identical save should not rewrite");

    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("chmod restore");
    assert_eq!(fs::read_to_string(&path).expect("read config"), body);
    assert!(
        !config_backup_path(&path).exists(),
        "no-op save must not create a migration backup"
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn config_store_save_creates_one_time_backup_before_changed_write() {
    use std::time::{SystemTime, UNIX_EPOCH};

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "codewhale-config-backup-save-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("mkdir");
    let path = dir.join(CONFIG_FILE_NAME);
    let original = "model = \"deepseek-v4-flash\"\n";
    fs::write(&path, original).expect("seed config");

    let store = ConfigStore {
        path: path.clone(),
        config: ConfigToml {
            model: Some("deepseek-v4-pro".to_string()),
            ..ConfigToml::default()
        },
        permissions: PermissionsToml::default(),
        original_raw: None,
    };
    store.save().expect("changed save");

    let backup_path = config_backup_path(&path);
    assert_eq!(
        fs::read_to_string(&backup_path).expect("read backup"),
        original
    );
    let updated = fs::read_to_string(&path).expect("read updated config");
    assert!(updated.contains("model = \"deepseek-v4-pro\""));

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn config_backup_strips_plaintext_api_keys_but_preserves_non_secret_auth_metadata() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(CONFIG_FILE_NAME);
    let original = r#"api_key = "root-test-credential"
default_text_model = "deepseek-v4-pro"

[providers.openrouter]
api_key = "provider-test-credential"
api_key_env = "OPENROUTER_API_KEY"
auth_mode = "api_key"
"#;
    fs::write(&path, original).expect("seed config");

    let mut store = ConfigStore::load(Some(path.clone())).expect("load config");
    store.config.default_text_model = Some("deepseek-v4-flash".to_string());
    store.save().expect("changed save");

    let backup = fs::read_to_string(config_backup_path(&path)).expect("read backup");
    assert!(!backup.contains("root-test-credential"), "{backup}");
    assert!(!backup.contains("provider-test-credential"), "{backup}");
    assert!(
        !backup
            .lines()
            .any(|line| line.trim_start().starts_with("api_key ="))
    );
    assert!(backup.contains("api_key_env = \"OPENROUTER_API_KEY\""));
    assert!(backup.contains("auth_mode = \"api_key\""));
    assert!(backup.contains("default_text_model = \"deepseek-v4-pro\""));
}

#[test]
fn config_backup_scrub_repairs_an_existing_plaintext_backup() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(CONFIG_FILE_NAME);
    fs::write(&path, "model = \"new-model\"\n").expect("seed config");
    let backup_path = config_backup_path(&path);
    fs::write(
        &backup_path,
        "api_key = \"old-test-credential\"\nmodel = \"old-model\"\n",
    )
    .expect("seed backup");

    scrub_plaintext_api_keys_from_config_backup(&path).expect("scrub backup");

    let backup = fs::read_to_string(backup_path).expect("read backup");
    assert!(!backup.contains("old-test-credential"), "{backup}");
    assert!(!backup.contains("api_key"), "{backup}");
    assert!(backup.contains("model = \"old-model\""));
}

#[test]
fn config_store_save_preserves_comments() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join(CONFIG_FILE_NAME);
    let original = "# my model\nmodel = \"deepseek-v4-flash\"\n# end comment\n";
    fs::write(&config_path, original).expect("write config");

    let mut store = ConfigStore::load(Some(config_path.clone())).expect("load config store");
    store.config.model = Some("deepseek-v4-pro".to_string());
    store.save().expect("save");

    let body = fs::read_to_string(&config_path).expect("read config");
    assert!(body.contains("# my model"), "prefix comment preserved");
    assert!(body.contains("# end comment"), "suffix comment preserved");
    assert!(body.contains("model = \"deepseek-v4-pro\""));
}

#[test]
fn config_store_save_preserves_disabled_keys() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join(CONFIG_FILE_NAME);
    fs::write(
        &config_path,
        "# my note\nmodel = \"deepseek-v4-flash\"\n# base_url = \"http://localhost:11434/v1\"\n",
    )
    .expect("write config");

    let mut store = ConfigStore::load(Some(config_path.clone())).expect("load config store");
    store.config.model = Some("deepseek-v4-pro".to_string());
    store.save().expect("save");

    let body = fs::read_to_string(&config_path).expect("read config");
    assert!(
        body.contains("# base_url = \"http://localhost:11434/v1\""),
        "disabled key preserved as comment"
    );
    assert!(body.contains("model = \"deepseek-v4-pro\""));
}

#[test]
fn config_store_save_preserves_comments_with_other_keys() {
    // Realistic scenario: user already has api_key + model, adds a comment,
    // then changes model via `codewhale config set model`.
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join(CONFIG_FILE_NAME);
    fs::write(
        &config_path,
        "# my deepseek key\napi_key = \"sk-1234\"\n\n# my current model\nmodel = \"deepseek-v4-flash\"\n",
    )
    .expect("write config");

    let mut store = ConfigStore::load(Some(config_path.clone())).expect("load config store");
    store.config.model = Some("deepseek-v4-pro".to_string());
    store.save().expect("save");

    let body = fs::read_to_string(&config_path).expect("read config");
    assert!(body.contains("# my deepseek key"), "api_key comment lost");
    assert!(body.contains("# my current model"), "model comment lost");
    assert!(
        body.contains("model = \"deepseek-v4-pro\""),
        "new model not written"
    );
    assert!(body.contains("api_key = \"sk-1234\""), "api_key lost");
}

#[test]
fn setup_transaction_applies_config_store_body_preserving_comments() {
    // #3410: the comment-preserving ConfigStore write must compose with
    // SetupTransaction so a setup step can update config.toml atomically
    // alongside sibling setup files.
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join(CONFIG_FILE_NAME);
    let state_path = dir.path().join(crate::setup_state::SETUP_STATE_FILE_NAME);
    fs::write(
        &config_path,
        "# my model\nmodel = \"deepseek-v4-flash\"\n# end comment\n",
    )
    .expect("write config");

    let mut store = ConfigStore::load(Some(config_path.clone())).expect("load config store");
    store.config.model = Some("deepseek-v4-pro".to_string());

    let mut transaction = persistence::SetupTransaction::new();
    transaction.stage(
        &config_path,
        store.rendered_body().expect("rendered body").into_bytes(),
    );
    transaction
        .stage_json(&state_path, &SetupState::default())
        .expect("stage setup state");
    transaction.commit().expect("commit");

    let body = fs::read_to_string(&config_path).expect("read config");
    assert!(body.contains("# my model"), "prefix comment preserved");
    assert!(body.contains("# end comment"), "suffix comment preserved");
    assert!(body.contains("model = \"deepseek-v4-pro\""));
    assert!(state_path.exists(), "sibling setup state written");
}

#[test]
fn setup_transaction_rolls_back_config_store_body_on_sibling_failure() {
    // #3410 rollback expectation: when a sibling stage fails to apply, the
    // already-written config.toml is restored byte-for-byte, comments and
    // all — no half-applied setup.
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join(CONFIG_FILE_NAME);
    let original = "# my model\nmodel = \"deepseek-v4-flash\"\n# end comment\n";
    fs::write(&config_path, original).expect("write config");
    // A parent that is a regular file makes the second stage unwritable.
    let blocker = dir.path().join("blocker");
    fs::write(&blocker, b"file, not a directory").expect("write blocker");

    let mut store = ConfigStore::load(Some(config_path.clone())).expect("load config store");
    store.config.model = Some("deepseek-v4-pro".to_string());

    let mut transaction = persistence::SetupTransaction::new();
    transaction.stage(
        &config_path,
        store.rendered_body().expect("rendered body").into_bytes(),
    );
    transaction.stage(blocker.join("nested.json"), b"{}".to_vec());
    transaction
        .commit()
        .expect_err("commit must fail on unwritable sibling");

    let body = fs::read_to_string(&config_path).expect("read config");
    assert_eq!(body, original, "config restored byte-for-byte on rollback");
}

#[test]
fn config_store_load_fails_on_malformed_config_without_touching_file() {
    // #3410 malformed-config posture: repair is explicit, never implicit.
    // Loading a malformed config surfaces a parse error naming the path and
    // leaves the file bytes untouched for the user (or doctor) to repair.
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join(CONFIG_FILE_NAME);
    let malformed = "# half-edited config\nmodel = \"deepseek-v4-flash\n";
    fs::write(&config_path, malformed).expect("write config");

    let err = ConfigStore::load(Some(config_path.clone())).expect_err("malformed must not parse");

    assert!(
        format!("{err:#}").contains("failed to parse config"),
        "error should name the parse failure: {err:#}"
    );
    let body = fs::read_to_string(&config_path).expect("read config");
    assert_eq!(body, malformed, "malformed config left untouched");
}

#[test]
fn config_store_rendered_body_preserves_comments_at_legacy_deepseek_path() {
    // #3410 legacy case: a config still living under `.deepseek/` keeps its
    // comments when written back through a transaction at the same path.
    let dir = tempfile::tempdir().expect("tempdir");
    let legacy_dir = dir.path().join(".deepseek");
    fs::create_dir_all(&legacy_dir).expect("legacy dir");
    let config_path = legacy_dir.join(CONFIG_FILE_NAME);
    fs::write(
        &config_path,
        "# legacy home config\nmodel = \"deepseek-v4-flash\"\n",
    )
    .expect("write config");

    let mut store = ConfigStore::load(Some(config_path.clone())).expect("load config store");
    store.config.model = Some("deepseek-v4-pro".to_string());

    let mut transaction = persistence::SetupTransaction::new();
    transaction.stage(
        &config_path,
        store.rendered_body().expect("rendered body").into_bytes(),
    );
    transaction.commit().expect("commit");

    let body = fs::read_to_string(&config_path).expect("read config");
    assert!(body.contains("# legacy home config"), "comment preserved");
    assert!(body.contains("model = \"deepseek-v4-pro\""));
}

#[test]
fn merge_and_preserve_comments_returns_err_on_invalid_serialized() {
    let err = merge_and_preserve_comments("{{{ not toml", "model = 1\n")
        .expect_err("invalid serialized should fail");
    assert!(
        format!("{err:#}").contains("failed to parse serialized"),
        "unexpected error: {err:#}"
    );
}

#[test]
fn merge_and_preserve_comments_returns_err_on_invalid_original() {
    let err = merge_and_preserve_comments("model = 1\n", "{{{ not toml")
        .expect_err("invalid original should fail");
    assert!(
        format!("{err:#}").contains("failed to parse original"),
        "unexpected error: {err:#}"
    );
}

#[test]
fn config_store_save_falls_back_when_comment_merge_fails() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join(CONFIG_FILE_NAME);
    // Valid TOML so load succeeds, but the raw is corrupt so the merge
    // will fail inside save() — save must still succeed and write the
    // plain serialized config.
    fs::write(&config_path, "model = \"deepseek-v4-flash\"\n").expect("write config");

    // Bypass ConfigStore::load to inject a deliberately broken original_raw.
    let store = ConfigStore {
        path: config_path.clone(),
        config: ConfigToml {
            model: Some("deepseek-v4-pro".to_string()),
            ..ConfigToml::default()
        },
        permissions: PermissionsToml::default(),
        original_raw: Some("{ broken".to_string()),
    };
    store
        .save()
        .expect("save should succeed even when merge fails");

    let body = fs::read_to_string(&config_path).expect("read config");
    assert!(
        body.contains("deepseek-v4-pro"),
        "config should be written: {body}"
    );
}

#[test]
fn provider_kind_parses_openrouter_and_novita_aliases() {
    assert_eq!(
        ProviderKind::parse("openrouter"),
        Some(ProviderKind::Openrouter)
    );
    assert_eq!(
        ProviderKind::parse("OPEN_ROUTER"),
        Some(ProviderKind::Openrouter)
    );
    assert_eq!(
        ProviderKind::parse("xiaomi-mimo"),
        Some(ProviderKind::XiaomiMimo)
    );
    assert_eq!(
        ProviderKind::parse("xiaomi"),
        Some(ProviderKind::XiaomiMimo)
    );
    assert_eq!(ProviderKind::parse("novita"), Some(ProviderKind::Novita));
    assert_eq!(ProviderKind::parse("Novita"), Some(ProviderKind::Novita));
    assert_eq!(
        ProviderKind::parse("fireworks-ai"),
        Some(ProviderKind::Fireworks)
    );
    assert_eq!(
        ProviderKind::parse("silicon-flow"),
        Some(ProviderKind::Siliconflow)
    );
    assert_eq!(
        ProviderKind::parse("silicon_flow"),
        Some(ProviderKind::Siliconflow)
    );
    assert_eq!(ProviderKind::parse("kimi"), Some(ProviderKind::Moonshot));
    assert_eq!(
        ProviderKind::parse("moonshot-ai"),
        Some(ProviderKind::Moonshot)
    );
    assert_eq!(ProviderKind::parse("sg-lang"), Some(ProviderKind::Sglang));
    assert_eq!(ProviderKind::parse("v-llm"), Some(ProviderKind::Vllm));
    assert_eq!(ProviderKind::parse("vllm"), Some(ProviderKind::Vllm));
    assert_eq!(ProviderKind::parse("ollama"), Some(ProviderKind::Ollama));
    assert_eq!(
        ProviderKind::parse("ollama-local"),
        Some(ProviderKind::Ollama)
    );
    assert_eq!(
        ProviderKind::parse("wanjie-ark"),
        Some(ProviderKind::WanjieArk)
    );
    assert_eq!(
        ProviderKind::parse("ark_wanjie"),
        Some(ProviderKind::WanjieArk)
    );
    for alias in ["huggingface", "hugging-face", "hugging_face", "hf"] {
        assert_eq!(ProviderKind::parse(alias), Some(ProviderKind::Huggingface));

        let parsed: ConfigToml =
            toml::from_str(&format!("provider = \"{alias}\"")).expect("huggingface alias");
        assert_eq!(parsed.provider, ProviderKind::Huggingface);
    }

    for alias in ["deepinfra", "deep-infra", "deep_infra"] {
        assert_eq!(ProviderKind::parse(alias), Some(ProviderKind::Deepinfra));

        let parsed: ConfigToml =
            toml::from_str(&format!("provider = \"{alias}\"")).expect("deepinfra alias");
        assert_eq!(parsed.provider, ProviderKind::Deepinfra);
    }

    for alias in ["sakana", "sakana-ai", "sakana_ai", "fugu"] {
        assert_eq!(ProviderKind::parse(alias), Some(ProviderKind::Sakana));

        let parsed: ConfigToml =
            toml::from_str(&format!("provider = \"{alias}\"")).expect("sakana alias");
        assert_eq!(parsed.provider, ProviderKind::Sakana);
    }

    for alias in ["qianfan", "baidu-qianfan", "baidu_qianfan", "baidu"] {
        assert_eq!(ProviderKind::parse(alias), Some(ProviderKind::Qianfan));

        let parsed: ConfigToml =
            toml::from_str(&format!("provider = \"{alias}\"")).expect("qianfan alias");
        assert_eq!(parsed.provider, ProviderKind::Qianfan);
    }

    let parsed: ConfigToml =
        toml::from_str("provider = \"ark-wanjie\"").expect("wanjie provider alias");
    assert_eq!(parsed.provider, ProviderKind::WanjieArk);

    let parsed: ConfigToml =
        toml::from_str("provider = \"silicon-flow\"").expect("siliconflow provider alias");
    assert_eq!(parsed.provider, ProviderKind::Siliconflow);
}

/// Models.dev publishes provider ids that do not always match CodeWhale's
/// canonical id (`fireworks-ai`, `togetherai`, `novita-ai`, `moonshotai`).
/// These MUST normalize onto the right [`ProviderKind`] via
/// [`ProviderKind::parse`], which is the seam `ModelReferenceCard::from_offering`
/// uses to label a live-catalog row's provider kind. A miss here means
/// Fireworks/Together/Novita/Moonshot models from the live Models.dev catalog
/// land under an `unknown` kind (Refs #4186).
#[test]
fn provider_kind_normalizes_models_dev_provider_ids() {
    let cases = [
        ("fireworks-ai", ProviderKind::Fireworks),
        ("togetherai", ProviderKind::Together),
        ("together-ai", ProviderKind::Together),
        ("together_ai", ProviderKind::Together),
        ("novita-ai", ProviderKind::Novita),
        ("novita_ai", ProviderKind::Novita),
        // Live Models.dev key for Moonshot/Kimi (verified 2026-07-08).
        ("moonshotai", ProviderKind::Moonshot),
        ("moonshot-ai", ProviderKind::Moonshot),
        ("moonshot_ai", ProviderKind::Moonshot),
        ("nvidia", ProviderKind::NvidiaNim),
        ("xiaomi", ProviderKind::XiaomiMimo),
        ("deepinfra", ProviderKind::Deepinfra),
        ("siliconflow", ProviderKind::Siliconflow),
        // Models.dev spells the China endpoint `siliconflow-cn`; CodeWhale's
        // canonical id is `siliconflow-CN` and `parse` is case-insensitive.
        ("siliconflow-cn", ProviderKind::SiliconflowCN),
        ("openrouter", ProviderKind::Openrouter),
        ("longcat", ProviderKind::LongCat),
        ("xai", ProviderKind::Xai),
        ("x-ai", ProviderKind::Xai),
        ("x_ai", ProviderKind::Xai),
        ("grok", ProviderKind::Xai),
    ];
    for (models_dev_id, expected) in cases {
        assert_eq!(
            ProviderKind::parse(models_dev_id),
            Some(expected),
            "Models.dev id {models_dev_id:?} must normalize onto {expected:?}"
        );
    }

    // The separator-free Models.dev ids must also deserialize from config TOML,
    // so a `provider = "togetherai"` / `"novita-ai"` / `"moonshotai"` line
    // resolves identically.
    for (alias, expected) in [
        ("togetherai", ProviderKind::Together),
        ("novita-ai", ProviderKind::Novita),
        ("fireworks-ai", ProviderKind::Fireworks),
        ("moonshotai", ProviderKind::Moonshot),
        ("grok", ProviderKind::Xai),
    ] {
        let parsed: ConfigToml =
            toml::from_str(&format!("provider = \"{alias}\"")).expect("models.dev id alias");
        assert_eq!(parsed.provider, expected, "toml provider = {alias:?}");
    }
}

/// Pin the Fireworks and Together transport metadata against the real provider
/// APIs: the OpenAI-compatible base URL and the canonical API-key env var. These
/// are the two primary providers this audit targets, so a regression to a wrong
/// base URL or env var name fails here.
#[test]
fn fireworks_and_together_base_url_and_auth_metadata() {
    let fireworks = provider::provider_for_kind(ProviderKind::Fireworks);
    assert_eq!(fireworks.id(), "fireworks");
    assert_eq!(
        fireworks.default_base_url(),
        "https://api.fireworks.ai/inference/v1"
    );
    assert_eq!(fireworks.default_base_url(), DEFAULT_FIREWORKS_BASE_URL);
    assert_eq!(fireworks.env_vars(), &["FIREWORKS_API_KEY"]);
    // Fireworks wire model ids are namespaced `accounts/fireworks/models/<name>`.
    assert!(
        fireworks
            .default_model()
            .starts_with("accounts/fireworks/models/"),
        "Fireworks default model must use the accounts/fireworks/models/ prefix, got {:?}",
        fireworks.default_model()
    );

    let together = provider::provider_for_kind(ProviderKind::Together);
    assert_eq!(together.id(), "together");
    assert_eq!(together.default_base_url(), "https://api.together.xyz/v1");
    assert_eq!(together.default_base_url(), DEFAULT_TOGETHER_BASE_URL);
    assert_eq!(together.env_vars(), &["TOGETHER_API_KEY"]);
    // Together wire model ids are `<org>/<Model>` (a slash-namespaced id).
    assert!(
        together.default_model().contains('/'),
        "Together default model must be an <org>/<Model> id, got {:?}",
        together.default_model()
    );

    // The env-based key resolver (secrets crate) must recognize both providers
    // by canonical id; a shell-exported key would otherwise be ignored.
    let _lock = env_lock();
    unsafe {
        std::env::set_var("FIREWORKS_API_KEY", "fw-test-key");
        std::env::set_var("TOGETHER_API_KEY", "tg-test-key");
    }
    assert_eq!(
        codewhale_secrets::env_for("fireworks").as_deref(),
        Some("fw-test-key")
    );
    assert_eq!(
        codewhale_secrets::env_for("together").as_deref(),
        Some("tg-test-key")
    );
    unsafe {
        std::env::remove_var("FIREWORKS_API_KEY");
        std::env::remove_var("TOGETHER_API_KEY");
    }
}

#[test]
fn unknown_provider_error_lists_huggingface() {
    let mut config = ConfigToml::default();
    let err = config
        .set_value("provider", "not-a-provider")
        .expect_err("unknown provider should fail");
    let message = err.to_string();
    assert!(message.contains("unknown provider 'not-a-provider'"));
    assert!(message.contains("huggingface"));
}

#[test]
fn provider_kind_accepts_legacy_deepseek_cn_aliases() {
    for alias in [
        "deepseek-cn",
        "deepseek_china",
        "deepseekcn",
        "deepseek-china",
    ] {
        assert_eq!(ProviderKind::parse(alias), Some(ProviderKind::Deepseek));

        let parsed: ConfigToml =
            toml::from_str(&format!("provider = \"{alias}\"")).expect("legacy provider alias");
        assert_eq!(parsed.provider, ProviderKind::Deepseek);
    }
}

#[test]
fn deepseek_anthropic_route_defaults_to_anthropic_endpoint() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    for alias in [
        "deepseek-anthropic",
        "deepseek_anthropic",
        "deepseek-claude",
        "deepseek_claude",
    ] {
        assert_eq!(
            ProviderKind::parse(alias),
            Some(ProviderKind::DeepseekAnthropic)
        );

        let parsed: ConfigToml =
            toml::from_str(&format!("provider = \"{alias}\"")).expect("deepseek anthropic alias");
        assert_eq!(parsed.provider, ProviderKind::DeepseekAnthropic);
    }

    let provider = provider::resolve_provider("deepseek-anthropic")
        .expect("deepseek anthropic metadata resolves");
    assert_eq!(provider.kind(), ProviderKind::DeepseekAnthropic);
    assert_eq!(provider.provider_config_key(), "deepseek_anthropic");
    assert_eq!(provider.default_model(), DEFAULT_DEEPSEEK_ANTHROPIC_MODEL);
    assert_eq!(
        provider.default_base_url(),
        DEFAULT_DEEPSEEK_ANTHROPIC_BASE_URL
    );
    assert_eq!(provider.env_vars(), &["DEEPSEEK_API_KEY"]);
    assert_eq!(
        provider.wire_policy().fixed(),
        Some(provider::WireFormat::AnthropicMessages)
    );

    let config = ConfigToml {
        provider: ProviderKind::DeepseekAnthropic,
        ..ConfigToml::default()
    };
    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::DeepseekAnthropic);
    assert_eq!(resolved.base_url, DEFAULT_DEEPSEEK_ANTHROPIC_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_DEEPSEEK_ANTHROPIC_MODEL);

    unsafe {
        std::env::set_var(
            "DEEPSEEK_ANTHROPIC_BASE_URL",
            "https://gateway.example.test/anthropic",
        );
    }
    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());
    assert_eq!(resolved.base_url, "https://gateway.example.test/anthropic");
    unsafe {
        std::env::remove_var("DEEPSEEK_ANTHROPIC_BASE_URL");
    }
}

#[test]
fn openmodel_route_defaults_to_messages_endpoint() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    for alias in ["openmodel", "open-model", "open_model"] {
        assert_eq!(ProviderKind::parse(alias), Some(ProviderKind::Openmodel));

        let parsed: ConfigToml =
            toml::from_str(&format!("provider = \"{alias}\"")).expect("openmodel alias");
        assert_eq!(parsed.provider, ProviderKind::Openmodel);
    }

    let provider = provider::resolve_provider("openmodel").expect("openmodel metadata resolves");
    assert_eq!(provider.kind(), ProviderKind::Openmodel);
    assert_eq!(provider.provider_config_key(), "openmodel");
    assert_eq!(provider.default_model(), DEFAULT_OPENMODEL_MODEL);
    assert_eq!(provider.default_base_url(), DEFAULT_OPENMODEL_BASE_URL);
    assert_eq!(provider.env_vars(), &["OPENMODEL_API_KEY"]);
    assert_eq!(
        provider.wire_policy().fixed(),
        Some(provider::WireFormat::AnthropicMessages)
    );

    let config = ConfigToml {
        provider: ProviderKind::Openmodel,
        ..ConfigToml::default()
    };
    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Openmodel);
    assert_eq!(resolved.base_url, DEFAULT_OPENMODEL_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_OPENMODEL_MODEL);

    unsafe {
        std::env::set_var("OPENMODEL_BASE_URL", "https://gateway.example.test");
        std::env::set_var("OPENMODEL_MODEL", "claude-sonnet-4-20250514");
    }
    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());
    assert_eq!(resolved.base_url, "https://gateway.example.test");
    assert_eq!(resolved.model, "claude-sonnet-4-20250514");
    unsafe {
        std::env::remove_var("OPENMODEL_BASE_URL");
        std::env::remove_var("OPENMODEL_MODEL");
    }
}

#[test]
fn xai_api_key_provider_resolves_defaults_and_scopes_env_credentials() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();

    let config = ConfigToml {
        provider: ProviderKind::Xai,
        ..ConfigToml::default()
    };
    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Xai);
    assert_eq!(resolved.base_url, DEFAULT_XAI_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_XAI_MODEL);
    assert_eq!(resolved.api_key, None);

    unsafe {
        std::env::set_var("XAI_API_KEY", "xai-env-key");
        std::env::set_var("XAI_BASE_URL", "https://xai-gateway.example/v1");
        std::env::set_var("XAI_MODEL", "grok-4.3");
    }

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());
    assert_eq!(resolved.api_key, None);
    assert_eq!(resolved.api_key_source, None);
    assert_eq!(resolved.base_url, "https://xai-gateway.example/v1");
    assert_eq!(resolved.model, "grok-4.3");
}

#[test]
fn opencode_go_resolves_current_chat_completions_route() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();

    for alias in ["opencode-go", "opencode_go", "opencodego"] {
        assert_eq!(ProviderKind::parse(alias), Some(ProviderKind::OpencodeGo));

        let parsed: ConfigToml =
            toml::from_str(&format!("provider = \"{alias}\"")).expect("OpenCode Go alias");
        assert_eq!(parsed.provider, ProviderKind::OpencodeGo);
    }

    let metadata = provider::resolve_provider("opencode_go").expect("provider metadata");
    assert_eq!(metadata.id(), "opencode-go");
    assert_eq!(metadata.display_name(), "OpenCode Go");
    assert_eq!(metadata.provider_config_key(), "opencode_go");
    assert_eq!(metadata.default_base_url(), DEFAULT_OPENCODE_GO_BASE_URL);
    assert_eq!(metadata.default_model(), DEFAULT_OPENCODE_GO_MODEL);
    assert_eq!(metadata.env_vars(), &["OPENCODE_GO_API_KEY"]);
    assert_eq!(
        metadata.wire_policy().fixed(),
        Some(provider::WireFormat::ChatCompletions)
    );

    let config: ConfigToml = toml::from_str(
        r#"
provider = "opencode-go"

[providers.opencode_go]
api_key = "go-config-key"
model = "opencode-go/glm-5.2"
"#,
    )
    .expect("OpenCode Go provider table");
    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());
    assert_eq!(resolved.provider, ProviderKind::OpencodeGo);
    assert_eq!(resolved.base_url, DEFAULT_OPENCODE_GO_BASE_URL);
    assert_eq!(resolved.model, OPENCODE_GO_GLM_5_2_MODEL);
    assert_eq!(resolved.api_key.as_deref(), Some("go-config-key"));
    assert_eq!(
        resolved.api_key_source,
        Some(RuntimeApiKeySource::ConfigFile)
    );

    // Provider-specific environment overrides remain available, but model ids
    // stay inside the Chat Completions allowlist.
    unsafe {
        std::env::set_var("OPENCODE_GO_API_KEY", "go-env-key");
        std::env::set_var("OPENCODE_GO_MODEL", "opencode-go/mimo-v2.5-pro");
    }
    assert_eq!(
        codewhale_secrets::env_for("opencode-go").as_deref(),
        Some("go-env-key")
    );
    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());
    assert_eq!(resolved.base_url, DEFAULT_OPENCODE_GO_BASE_URL);
    assert_eq!(resolved.model, OPENCODE_GO_MIMO_V2_5_PRO_MODEL);

    // The Go roster also includes Messages-only models. Even a custom base URL
    // cannot make one safe for this Chat Completions provider; resolution falls
    // back to the provider default instead of sending an incompatible request.
    unsafe {
        std::env::set_var("OPENCODE_GO_BASE_URL", "https://go-gateway.example/v1");
        std::env::set_var("OPENCODE_GO_MODEL", "minimax-m3");
    }
    assert!(opencode_go_chat_model_id("minimax-m3").is_none());
    assert!(opencode_go_chat_model_id("qwen3.7-max").is_none());
    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());
    assert_eq!(resolved.base_url, "https://go-gateway.example/v1");
    assert_eq!(resolved.model, DEFAULT_OPENCODE_GO_MODEL);
    assert_eq!(resolved.api_key, None);
}

#[test]
fn opencode_zen_configures_model_aware_provider_and_fails_closed_without_catalog_proof() {
    for alias in [
        "opencode-zen",
        "opencode_zen",
        "opencodezen",
        "zen",
        "opencode",
    ] {
        assert_eq!(ProviderKind::parse(alias), Some(ProviderKind::OpencodeZen));
    }

    let metadata = provider::resolve_provider("opencode_zen").expect("Zen provider metadata");
    assert_eq!(metadata.id(), "opencode-zen");
    assert_eq!(metadata.display_name(), "OpenCode Zen");
    assert_eq!(metadata.provider_config_key(), "opencode_zen");
    assert_eq!(metadata.default_base_url(), DEFAULT_OPENCODE_ZEN_BASE_URL);
    assert_eq!(metadata.default_model(), DEFAULT_OPENCODE_ZEN_MODEL);
    assert_eq!(
        metadata.env_vars(),
        &["OPENCODE_ZEN_API_KEY", "OPENCODE_API_KEY"]
    );
    assert_eq!(metadata.wire_policy(), provider::WirePolicy::ModelAware);

    let config: ConfigToml = toml::from_str(
        r#"
provider = "opencode-zen"

[providers.opencode_zen]
api_key = "zen-config-key"
base_url = "https://zen-gateway.example/v1"
model = "gpt-5.5"
"#,
    )
    .expect("OpenCode Zen provider table");
    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());
    assert_eq!(resolved.provider, ProviderKind::OpencodeZen);
    assert_eq!(resolved.base_url, "https://zen-gateway.example/v1");
    assert_eq!(resolved.model, "gpt-5.5");
    assert_eq!(resolved.api_key.as_deref(), Some("zen-config-key"));

    let resolver = crate::route::RouteResolver::new();
    let error = resolver
        .resolve(&crate::route::RouteRequest {
            explicit_provider: Some(ProviderKind::OpencodeZen),
            model_selector: Some(crate::route::LogicalModelRef::from("gpt-5.5")),
            saved_provider_model: None,
            base_url_override: None,
        })
        .expect_err("Zen model without catalog protocol proof must fail closed");
    assert!(
        matches!(
            error,
            crate::route::RouteError::UnsupportedModelProtocol { .. }
        ),
        "unexpected error: {error:?}"
    );
}

#[test]
fn meta_model_api_scopes_both_documented_key_names_to_official_endpoint() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();

    let config = ConfigToml {
        provider: ProviderKind::Meta,
        ..ConfigToml::default()
    };
    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Meta);
    assert_eq!(resolved.base_url, DEFAULT_META_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_META_MODEL);
    assert_eq!(resolved.api_key, None);

    unsafe {
        std::env::set_var("MODEL_API_KEY", "meta-official-key");
        std::env::set_var("MODEL_API_BASE_URL", "https://meta-gateway.example/v1");
        std::env::set_var("MODEL_API_MODEL", "muse-spark-canary");
    }
    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());
    assert_eq!(resolved.api_key, None);
    assert_eq!(resolved.api_key_source, None);
    assert_eq!(resolved.base_url, "https://meta-gateway.example/v1");
    assert_eq!(resolved.model, "muse-spark-canary");

    unsafe {
        std::env::set_var("META_MODEL_API_KEY", "meta-models-dev-key");
        std::env::set_var("META_MODEL_API_BASE_URL", "https://meta-primary.example/v1");
        std::env::set_var("META_MODEL_API_MODEL", "muse-spark-1.1");
    }
    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());
    assert_eq!(resolved.api_key, None);
    assert_eq!(resolved.api_key_source, None);
    assert_eq!(resolved.base_url, "https://meta-primary.example/v1");
    assert_eq!(resolved.model, "muse-spark-1.1");
}

#[test]
fn provider_metadata_registry_covers_every_provider_kind_once() {
    let providers = provider::all_providers();
    assert_eq!(providers.len(), ProviderKind::ALL.len());

    for (kind, provider) in ProviderKind::ALL.iter().zip(providers.iter()) {
        assert_eq!(provider.kind(), *kind);
        assert_eq!(provider.id(), kind.as_str());
        assert_eq!(kind.provider().id(), kind.as_str());
    }

    let mut ids = std::collections::BTreeSet::new();
    for provider in providers {
        assert!(ids.insert(provider.id()), "duplicate provider id");
    }
}

#[test]
fn provider_metadata_lookup_does_not_fall_back_to_deepseek() {
    assert!(provider::lookup_provider("not-a-provider").is_none());
    assert!(provider::resolve_provider("not-a-provider").is_none());
    assert!(provider::lookup_provider("deepseek-cn").is_none());
    assert_eq!(
        provider::resolve_provider("deepseek-cn")
            .expect("legacy alias resolves")
            .kind(),
        ProviderKind::Deepseek
    );
}

#[test]
fn provider_metadata_preserves_alias_and_config_key_semantics() {
    assert_eq!(
        provider::resolve_provider("open_router")
            .expect("openrouter alias")
            .kind(),
        ProviderKind::Openrouter
    );
    assert_eq!(
        provider::resolve_provider("xiaomi")
            .expect("xiaomi alias")
            .kind(),
        ProviderKind::XiaomiMimo
    );
    assert_eq!(
        provider::resolve_provider("kimi")
            .expect("kimi alias")
            .kind(),
        ProviderKind::Moonshot
    );
    assert_eq!(
        provider::resolve_provider("hf")
            .expect("huggingface alias")
            .kind(),
        ProviderKind::Huggingface
    );
    assert_eq!(
        provider::resolve_provider("grok")
            .expect("xAI grok alias")
            .kind(),
        ProviderKind::Xai
    );
    assert_eq!(
        provider::resolve_provider("muse-spark")
            .expect("Meta Muse Spark alias")
            .kind(),
        ProviderKind::Meta
    );

    let siliconflow_cn =
        provider::resolve_provider("siliconflow-cn").expect("siliconflow-cn alias resolves");
    assert_eq!(siliconflow_cn.kind(), ProviderKind::SiliconflowCN);
    assert_eq!(siliconflow_cn.id(), "siliconflow-CN");
    assert_eq!(siliconflow_cn.provider_config_key(), "siliconflow_cn");

    let config = ProvidersToml::default();
    let shared_table = config.for_provider(ProviderKind::SiliconflowCN);
    assert!(!std::ptr::eq(
        shared_table,
        config.for_provider(ProviderKind::Siliconflow)
    ));
}

#[test]
fn provider_metadata_defaults_match_runtime_helpers() {
    for kind in ProviderKind::ALL {
        let provider = kind.provider();
        assert_eq!(provider.default_model(), default_model_for_provider(kind));
        assert_eq!(
            provider.default_base_url(),
            default_base_url_for_provider(kind)
        );
        assert!(!provider.display_name().trim().is_empty());
        // The dynamic custom provider (#1519) intentionally declares no
        // built-in auth env var: the key env var name is supplied per entry via
        // `[providers.<name>] api_key_env = "..."`. Every built-in provider
        // still must declare at least one.
        if kind != ProviderKind::Custom {
            assert!(!provider.env_vars().is_empty());
        }
        // OpenAI Codex (ChatGPT) speaks the Responses API; Anthropic and the
        // Anthropic-compatible routes speak the native Messages API; every
        // other built-in provider is OpenAI-compatible Chat Completions.
        let expected_wire = match kind {
            ProviderKind::OpencodeZen => None,
            ProviderKind::OpenaiCodex => Some(provider::WireFormat::Responses),
            ProviderKind::Anthropic
            | ProviderKind::DeepseekAnthropic
            | ProviderKind::MinimaxAnthropic
            | ProviderKind::Openmodel => Some(provider::WireFormat::AnthropicMessages),
            _ => Some(provider::WireFormat::ChatCompletions),
        };
        assert_eq!(provider.wire_policy().fixed(), expected_wire);
    }
}

#[test]
fn openrouter_provider_defaults_to_canonical_endpoint_and_model() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let config = ConfigToml {
        provider: ProviderKind::Openrouter,
        ..ConfigToml::default()
    };

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Openrouter);
    assert_eq!(resolved.base_url, DEFAULT_OPENROUTER_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_OPENROUTER_MODEL);
}

#[test]
fn xiaomi_mimo_provider_defaults_to_canonical_endpoint_and_model() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let config = ConfigToml {
        provider: ProviderKind::XiaomiMimo,
        ..ConfigToml::default()
    };

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::XiaomiMimo);
    assert_eq!(resolved.base_url, DEFAULT_XIAOMI_MIMO_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_XIAOMI_MIMO_MODEL);
}

#[test]
fn xiaomi_provider_alias_table_maps_to_mimo_runtime_config() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let config: ConfigToml = toml::from_str(
        r#"
provider = "xiaomi-mimo"
default_text_model = "deepseek/deepseek-v4-pro"

[providers.xiaomi]
api_key = "mimo-table-key"
base_url = "https://token-plan-sgp.xiaomimimo.com/v1"
model = "mimo-v2.5-pro"
"#,
    )
    .expect("xiaomi provider alias config");

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::XiaomiMimo);
    assert_eq!(resolved.api_key.as_deref(), Some("mimo-table-key"));
    assert_eq!(
        resolved.base_url,
        "https://token-plan-sgp.xiaomimimo.com/v1"
    );
    assert_eq!(resolved.model, DEFAULT_XIAOMI_MIMO_MODEL);
}

#[test]
fn xiaomi_token_plan_key_rewrites_saved_pay_as_you_go_base_url() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let config: ConfigToml = toml::from_str(
        r#"
provider = "xiaomi-mimo"

[providers.xiaomi_mimo]
api_key = "tp-test-token-plan-key"
base_url = "https://api.xiaomimimo.com/v1"
model = "mimo-v2.5-pro"
"#,
    )
    .expect("xiaomi token-plan config");

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::XiaomiMimo);
    assert_eq!(resolved.base_url, DEFAULT_XIAOMI_MIMO_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_XIAOMI_MIMO_MODEL);
}

#[test]
fn xiaomi_mimo_token_plan_mode_accepts_region_aliases() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let config: ConfigToml = toml::from_str(
        r#"
provider = "mimo"

[providers.mimo]
mode = "token-plan-ams"
"#,
    )
    .expect("xiaomi token-plan region config");

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::XiaomiMimo);
    assert_eq!(resolved.base_url, XIAOMI_MIMO_TOKEN_PLAN_AMS_BASE_URL);
}

#[test]
fn xiaomi_mimo_unknown_mode_stays_on_token_plan_endpoint() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let config: ConfigToml = toml::from_str(
        r#"
provider = "mimo"

[providers.mimo]
mode = "token-plan-usa"
"#,
    )
    .expect("xiaomi token-plan unknown mode config");

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::XiaomiMimo);
    assert_eq!(resolved.base_url, DEFAULT_XIAOMI_MIMO_BASE_URL);
}

#[test]
fn xiaomi_mimo_aliases_resolve_to_canonical_models() {
    assert_eq!(
        normalize_model_for_provider(ProviderKind::XiaomiMimo, "omni"),
        "mimo-v2.5"
    );
    assert_eq!(
        normalize_model_for_provider(ProviderKind::XiaomiMimo, "pro-ultraspeed"),
        "mimo-v2.5-pro-ultraspeed"
    );
    assert_eq!(
        normalize_model_for_provider(ProviderKind::XiaomiMimo, "tts"),
        "mimo-v2.5-tts"
    );
    assert_eq!(
        normalize_model_for_provider(ProviderKind::XiaomiMimo, "voice-design"),
        "mimo-v2.5-tts-voicedesign"
    );
    assert_eq!(
        normalize_model_for_provider(ProviderKind::XiaomiMimo, "voiceclone"),
        "mimo-v2.5-tts-voiceclone"
    );
    assert_eq!(
        normalize_model_for_provider(ProviderKind::XiaomiMimo, "custom-mimo-model"),
        "custom-mimo-model"
    );
}

#[test]
fn zai_aliases_resolve_to_canonical_models() {
    // GLM-5.2 is the default; the glm-5.1 alias must still resolve to 5.1
    // (not to the default), and GLM-5-Turbo resolves to its own id.
    assert_eq!(
        normalize_model_for_provider(ProviderKind::Zai, "glm-5.1"),
        ZAI_GLM_5_1_MODEL
    );
    assert_eq!(
        normalize_model_for_provider(ProviderKind::Zai, "glm-5-2"),
        DEFAULT_ZAI_MODEL
    );
    assert_eq!(DEFAULT_ZAI_MODEL, ZAI_GLM_5_2_MODEL);
    assert_eq!(
        normalize_model_for_provider(ProviderKind::Zai, "glm-5-turbo"),
        ZAI_GLM_5_TURBO_MODEL
    );
    assert_eq!(
        normalize_model_for_provider(ProviderKind::Zai, "custom-glm-preview"),
        "custom-glm-preview"
    );
}

#[test]
fn zhipu_aliases_fold_into_zai_provider() {
    // Zhipu AI and Z.ai are the same vendor; `zhipu`/`zhipuai`/`bigmodel`
    // resolve to the single Zai provider rather than a separate one.
    assert_eq!(ProviderKind::parse("zhipu"), Some(ProviderKind::Zai));
    assert_eq!(ProviderKind::parse("zhipuai"), Some(ProviderKind::Zai));
    assert_eq!(ProviderKind::parse("bigmodel"), Some(ProviderKind::Zai));
    assert_eq!(ProviderKind::parse("big-model"), Some(ProviderKind::Zai));

    // A `[providers.zhipu]` table (BigModel China endpoint) merges into the Zai
    // provider config through the serde alias.
    let parsed: ConfigToml = toml::from_str(
        r#"
        [providers.zhipu]
        api_key = "$ZHIPU_API_KEY"
        base_url = "https://open.bigmodel.cn/api/paas/v4/"
        model = "glm-5-2"
        "#,
    )
    .expect("zhipu provider table parses");

    let provider = parsed.providers.for_provider(ProviderKind::Zai);
    assert_eq!(provider.api_key.as_deref(), Some("$ZHIPU_API_KEY"));
    assert_eq!(
        provider.base_url.as_deref(),
        Some("https://open.bigmodel.cn/api/paas/v4/")
    );
    assert_eq!(provider.model.as_deref(), Some("glm-5-2"));

    // GLM aliases canonicalize under the Zai umbrella.
    assert_eq!(
        normalize_model_for_provider(ProviderKind::Zai, "glm-5-2"),
        DEFAULT_ZAI_MODEL
    );
}

#[test]
fn novita_provider_defaults_to_canonical_endpoint_and_model() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let config = ConfigToml {
        provider: ProviderKind::Novita,
        ..ConfigToml::default()
    };

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Novita);
    assert_eq!(resolved.base_url, DEFAULT_NOVITA_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_NOVITA_MODEL);
}

#[test]
fn fireworks_provider_defaults_to_canonical_endpoint_and_model() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let config = ConfigToml {
        provider: ProviderKind::Fireworks,
        ..ConfigToml::default()
    };

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Fireworks);
    assert_eq!(resolved.base_url, DEFAULT_FIREWORKS_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_FIREWORKS_MODEL);
}

#[test]
fn siliconflow_provider_defaults_to_canonical_endpoint_and_model() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let config = ConfigToml {
        provider: ProviderKind::Siliconflow,
        ..ConfigToml::default()
    };

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Siliconflow);
    assert_eq!(resolved.base_url, DEFAULT_SILICONFLOW_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_SILICONFLOW_MODEL);
}

#[test]
fn siliconflow_cn_config_falls_back_to_shared_table_when_unset() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let mut config = ConfigToml {
        provider: ProviderKind::SiliconflowCN,
        ..ConfigToml::default()
    };
    config.providers.siliconflow.api_key = Some("sf-shared-key".to_string());
    config.providers.siliconflow.base_url = Some(DEFAULT_SILICONFLOW_BASE_URL.to_string());
    config.providers.siliconflow.model = Some("deepseek-chat".to_string());
    config.providers.siliconflow_cn.base_url = Some(DEFAULT_SILICONFLOW_CN_BASE_URL.to_string());

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::SiliconflowCN);
    assert_eq!(resolved.api_key.as_deref(), Some("sf-shared-key"));
    assert_eq!(resolved.base_url, DEFAULT_SILICONFLOW_CN_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_SILICONFLOW_FLASH_MODEL);
}

#[test]
fn siliconflow_cn_first_class_config_preserves_provider_scoped_route() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let mut config = ConfigToml {
        provider: ProviderKind::SiliconflowCN,
        ..ConfigToml::default()
    };
    config.providers.siliconflow_cn.api_key = Some("sf-cn-file-key".to_string());
    config.providers.siliconflow_cn.base_url = Some(DEFAULT_SILICONFLOW_CN_BASE_URL.to_string());
    config.providers.siliconflow_cn.model = Some(DEFAULT_SILICONFLOW_MODEL.to_string());

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::SiliconflowCN);
    assert_eq!(resolved.api_key.as_deref(), Some("sf-cn-file-key"));
    assert_eq!(resolved.base_url, DEFAULT_SILICONFLOW_CN_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_SILICONFLOW_MODEL);
}

#[test]
fn moonshot_provider_defaults_to_kimi_k27_code() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let config = ConfigToml {
        provider: ProviderKind::Moonshot,
        ..ConfigToml::default()
    };

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Moonshot);
    assert_eq!(resolved.base_url, DEFAULT_MOONSHOT_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_MOONSHOT_MODEL);
}

#[test]
fn zai_stepfun_minimax_and_sakana_default_to_first_party_routes() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();

    for (provider, expected_base_url, expected_model) in [
        (ProviderKind::Zai, DEFAULT_ZAI_BASE_URL, DEFAULT_ZAI_MODEL),
        (
            ProviderKind::Stepfun,
            DEFAULT_STEPFUN_BASE_URL,
            DEFAULT_STEPFUN_MODEL,
        ),
        (
            ProviderKind::Minimax,
            DEFAULT_MINIMAX_BASE_URL,
            DEFAULT_MINIMAX_MODEL,
        ),
        (
            ProviderKind::MinimaxAnthropic,
            DEFAULT_MINIMAX_ANTHROPIC_BASE_URL,
            DEFAULT_MINIMAX_MODEL,
        ),
        (
            ProviderKind::Sakana,
            DEFAULT_SAKANA_BASE_URL,
            DEFAULT_SAKANA_MODEL,
        ),
    ] {
        let config = ConfigToml {
            provider,
            ..ConfigToml::default()
        };
        let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, provider);
        assert_eq!(resolved.base_url, expected_base_url);
        assert_eq!(resolved.model, expected_model);
    }
}

#[test]
fn qianfan_provider_defaults_to_openai_compatible_endpoint_and_model() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let config = ConfigToml {
        provider: ProviderKind::Qianfan,
        ..ConfigToml::default()
    };

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Qianfan);
    assert_eq!(resolved.base_url, DEFAULT_QIANFAN_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_QIANFAN_MODEL);
}

#[test]
fn qianfan_provider_preserves_configured_base_url_and_model() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let mut config = ConfigToml {
        provider: ProviderKind::Qianfan,
        ..ConfigToml::default()
    };
    config.providers.qianfan.api_key = Some("qianfan-table-key".to_string());
    config.providers.qianfan.base_url = Some("https://qianfan.baidubce.com/v2".to_string());
    config.providers.qianfan.model = Some("custom-qianfan-service-id".to_string());

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Qianfan);
    assert_eq!(resolved.api_key.as_deref(), Some("qianfan-table-key"));
    assert_eq!(resolved.base_url, "https://qianfan.baidubce.com/v2");
    assert_eq!(resolved.model, "custom-qianfan-service-id");
}

#[test]
fn first_party_provider_env_model_overrides_pass_through() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    unsafe {
        env::set_var("CODEWHALE_PROVIDER", "minimax");
        env::set_var("MINIMAX_MODEL", "MiniMax-M2.7-highspeed");
        env::set_var("MINIMAX_BASE_URL", "https://minimax.example/v1");
    }

    let resolved = ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Minimax);
    assert_eq!(resolved.base_url, "https://minimax.example/v1");
    assert_eq!(resolved.model, "MiniMax-M2.7-highspeed");
}

#[test]
fn minimax_env_model_override_canonicalizes_known_aliases() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    unsafe {
        env::set_var("CODEWHALE_PROVIDER", "minimax");
        env::set_var("MINIMAX_MODEL", "minimax-m2-5-highspeed");
    }

    let resolved = ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Minimax);
    assert_eq!(resolved.model, "MiniMax-M2.5-highspeed");
}

#[test]
fn minimax_anthropic_env_overrides_use_messages_base_url() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    unsafe {
        env::set_var("CODEWHALE_PROVIDER", "minimax-anthropic");
        env::set_var(
            "MINIMAX_ANTHROPIC_BASE_URL",
            "https://messages.minimax.example/anthropic",
        );
        env::set_var("MINIMAX_MODEL", "MiniMax-M2.7");
    }

    let resolved = ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::MinimaxAnthropic);
    assert_eq!(
        resolved.base_url,
        "https://messages.minimax.example/anthropic"
    );
    assert_eq!(resolved.model, "MiniMax-M2.7");
}

#[test]
fn sakana_env_overrides_resolve_fugu_route() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    unsafe {
        env::set_var("CODEWHALE_PROVIDER", "sakana");
        env::set_var("SAKANA_BASE_URL", "https://sakana.example/v1");
        env::set_var("SAKANA_MODEL", "fugu-ultra-20260615");
    }

    let resolved = ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Sakana);
    assert_eq!(resolved.base_url, "https://sakana.example/v1");
    assert_eq!(resolved.model, "fugu-ultra-20260615");
}

#[test]
fn moonshot_provider_preserves_explicit_kimi_k26() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let mut config = ConfigToml {
        provider: ProviderKind::Moonshot,
        ..ConfigToml::default()
    };
    config.providers.moonshot.model = Some("kimi-k2.6".to_string());

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Moonshot);
    assert_eq!(resolved.model, MOONSHOT_KIMI_K2_6_MODEL);
}

#[test]
fn moonshot_kimi_oauth_uses_kimi_code_endpoint_and_model() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let mut config = ConfigToml {
        provider: ProviderKind::Moonshot,
        ..ConfigToml::default()
    };
    config.providers.moonshot.auth_mode = Some("kimi_oauth".to_string());

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Moonshot);
    assert_eq!(resolved.auth_mode.as_deref(), Some("kimi_oauth"));
    assert_eq!(resolved.base_url, DEFAULT_KIMI_CODE_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_KIMI_CODE_MODEL);
    assert_eq!(resolved.api_key, None);
    assert_eq!(resolved.api_key_source, None);
}

#[test]
fn moonshot_kimi_code_api_key_endpoint_defaults_to_kimi_for_coding() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let mut config = ConfigToml {
        provider: ProviderKind::Moonshot,
        ..ConfigToml::default()
    };
    config.providers.moonshot.api_key = Some("kimi-code-key".to_string());
    config.providers.moonshot.base_url = Some(DEFAULT_KIMI_CODE_BASE_URL.to_string());

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Moonshot);
    assert_eq!(resolved.auth_mode, None);
    assert_eq!(resolved.base_url, DEFAULT_KIMI_CODE_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_KIMI_CODE_MODEL);
    assert_eq!(resolved.api_key.as_deref(), Some("kimi-code-key"));
    assert_eq!(
        resolved.api_key_source,
        Some(RuntimeApiKeySource::ConfigFile)
    );
}

/// `CODEWHALE_PROVIDER` is the user-facing env alias for switching the
/// active provider. It must be honored by the runtime resolver and win
/// over a root `provider = "deepseek"` config entry.
#[test]
fn codewhale_provider_env_switches_active_provider() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only env mutation guarded by env_lock().
    unsafe {
        env::set_var("CODEWHALE_PROVIDER", "moonshot");
    }
    let mut config = ConfigToml {
        provider: ProviderKind::Deepseek,
        ..ConfigToml::default()
    };
    config.providers.moonshot.api_key = Some("kimi-code-key".to_string());
    config.providers.moonshot.base_url = Some(DEFAULT_KIMI_CODE_BASE_URL.to_string());

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Moonshot);
    assert_eq!(
        resolved.provider_source,
        ProviderSource::Env("CODEWHALE_PROVIDER")
    );
    assert_eq!(resolved.base_url, DEFAULT_KIMI_CODE_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_KIMI_CODE_MODEL);
    assert_eq!(resolved.api_key.as_deref(), Some("kimi-code-key"));
}

/// When both `CODEWHALE_PROVIDER` and the legacy `DEEPSEEK_PROVIDER`
/// are set, the public alias wins — a user adopting `CODEWHALE_*` in a
/// fresh shell config is not tripped up by a stale legacy export still
/// living in their dotfiles.
#[test]
fn codewhale_provider_env_wins_over_deepseek_provider_env() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only env mutation guarded by env_lock().
    unsafe {
        env::set_var("CODEWHALE_PROVIDER", "moonshot");
        env::set_var("DEEPSEEK_PROVIDER", "openrouter");
    }
    let config = ConfigToml {
        provider: ProviderKind::Deepseek,
        ..ConfigToml::default()
    };

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Moonshot);
    assert_eq!(
        resolved.provider_source,
        ProviderSource::Env("CODEWHALE_PROVIDER")
    );
}

#[test]
fn legacy_deepseek_provider_env_records_provider_source() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only env mutation guarded by env_lock().
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "openrouter");
    }
    let config = ConfigToml {
        provider: ProviderKind::Deepseek,
        ..ConfigToml::default()
    };

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Openrouter);
    assert_eq!(
        resolved.provider_source,
        ProviderSource::Env("DEEPSEEK_PROVIDER")
    );
}

#[test]
fn cli_provider_records_provider_source() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only env mutation guarded by env_lock().
    unsafe {
        env::set_var("CODEWHALE_PROVIDER", "moonshot");
    }
    let cli = CliRuntimeOverrides {
        provider: Some(ProviderKind::Openai),
        ..CliRuntimeOverrides::default()
    };
    let config = ConfigToml {
        provider: ProviderKind::Deepseek,
        ..ConfigToml::default()
    };

    let resolved = config.resolve_runtime_options(&cli);

    assert_eq!(resolved.provider, ProviderKind::Openai);
    assert_eq!(resolved.provider_source, ProviderSource::Cli);
}

#[test]
fn config_provider_records_provider_source() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let config = ConfigToml {
        provider: ProviderKind::Moonshot,
        ..ConfigToml::default()
    };

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Moonshot);
    assert_eq!(resolved.provider_source, ProviderSource::Config);
}

/// `CODEWHALE_MODEL` is the user-facing env alias for picking a model
/// against the active provider. It must be honored by the runtime
/// resolver in place of `DEEPSEEK_MODEL`.
#[test]
fn codewhale_model_env_alias_overrides_default_for_active_provider() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only env mutation guarded by env_lock().
    unsafe {
        env::set_var("CODEWHALE_PROVIDER", "moonshot");
        env::set_var("CODEWHALE_MODEL", "custom-kimi-test-model");
    }
    let config = ConfigToml::default();

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Moonshot);
    assert_eq!(resolved.model, "custom-kimi-test-model");
}

#[test]
fn blank_codewhale_model_env_alias_does_not_override_default_for_active_provider() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only env mutation guarded by env_lock().
    unsafe {
        env::set_var("CODEWHALE_PROVIDER", "moonshot");
        env::set_var("CODEWHALE_MODEL", "   ");
    }
    let config = ConfigToml::default();

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Moonshot);
    assert_eq!(resolved.model, DEFAULT_MOONSHOT_MODEL);
}

#[test]
fn deepseek_default_text_model_legacy_alias_still_overrides_active_provider_model() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only env mutation guarded by env_lock().
    unsafe {
        env::set_var("CODEWHALE_PROVIDER", "moonshot");
        env::set_var("DEEPSEEK_DEFAULT_TEXT_MODEL", "legacy-env-model");
    }
    let config = ConfigToml::default();

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Moonshot);
    assert_eq!(resolved.model, "legacy-env-model");
}

#[test]
fn wanjie_ark_provider_defaults_to_openai_compatible_endpoint_and_model() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let config = ConfigToml {
        provider: ProviderKind::WanjieArk,
        ..ConfigToml::default()
    };

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::WanjieArk);
    assert_eq!(resolved.base_url, DEFAULT_WANJIE_ARK_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_WANJIE_ARK_MODEL);
}

#[test]
fn sglang_provider_defaults_to_local_endpoint_and_model() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let config = ConfigToml {
        provider: ProviderKind::Sglang,
        ..ConfigToml::default()
    };

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Sglang);
    assert_eq!(resolved.base_url, DEFAULT_SGLANG_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_SGLANG_MODEL);
}

#[test]
fn vllm_provider_defaults_to_local_endpoint_and_model() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let config = ConfigToml {
        provider: ProviderKind::Vllm,
        ..ConfigToml::default()
    };

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Vllm);
    assert_eq!(resolved.base_url, DEFAULT_VLLM_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_VLLM_MODEL);
}

#[test]
fn ollama_provider_defaults_to_local_endpoint_and_small_model() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let config = ConfigToml {
        provider: ProviderKind::Ollama,
        ..ConfigToml::default()
    };

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Ollama);
    assert_eq!(resolved.base_url, DEFAULT_OLLAMA_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_OLLAMA_MODEL);
    assert_eq!(resolved.api_key, None);
}

#[test]
fn self_hosted_providers_do_not_probe_secret_store_by_default() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let store = Arc::new(RecordingSecretsStore::with_value("secret-store-key"));
    let secrets = Secrets::new(store.clone());

    for provider in [
        ProviderKind::Sglang,
        ProviderKind::Vllm,
        ProviderKind::Ollama,
    ] {
        let config = ConfigToml {
            provider,
            ..ConfigToml::default()
        };

        let resolved =
            config.resolve_runtime_options_with_secrets(&CliRuntimeOverrides::default(), &secrets);

        assert_eq!(resolved.provider, provider);
        assert_eq!(resolved.api_key, None);
    }

    assert!(
        store.gets.lock().unwrap().is_empty(),
        "self-hosted providers should not read the secret store by default"
    );
}

#[test]
fn self_hosted_api_key_auth_can_use_secret_store_when_requested() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let store = Arc::new(RecordingSecretsStore::with_value("secret-store-key"));
    let secrets = Secrets::new(store.clone());
    let config = ConfigToml {
        provider: ProviderKind::Ollama,
        auth_mode: Some("api_key".to_string()),
        ..ConfigToml::default()
    };

    let resolved =
        config.resolve_runtime_options_with_secrets(&CliRuntimeOverrides::default(), &secrets);

    assert_eq!(resolved.api_key.as_deref(), Some("secret-store-key"));
    assert_eq!(store.gets.lock().unwrap().as_slice(), ["ollama"]);
}

#[test]
fn moonshot_api_key_mode_can_use_secret_store_by_default() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let store = Arc::new(RecordingSecretsStore::with_value("secret-store-key"));
    let secrets = Secrets::new(store.clone());
    let config = ConfigToml {
        provider: ProviderKind::Moonshot,
        ..ConfigToml::default()
    };

    let resolved =
        config.resolve_runtime_options_with_secrets(&CliRuntimeOverrides::default(), &secrets);

    assert_eq!(resolved.api_key.as_deref(), Some("secret-store-key"));
    assert_eq!(resolved.api_key_source, Some(RuntimeApiKeySource::Keyring));
    assert_eq!(store.gets.lock().unwrap().as_slice(), ["moonshot"]);
}

#[test]
fn loopback_custom_deepseek_base_url_does_not_probe_secret_store_by_default() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let store = Arc::new(RecordingSecretsStore::with_value("stale-deepseek-key"));
    let secrets = Secrets::new(store.clone());
    let config = ConfigToml {
        base_url: Some("http://127.0.0.1:8000/v1".to_string()),
        ..ConfigToml::default()
    };

    let resolved =
        config.resolve_runtime_options_with_secrets(&CliRuntimeOverrides::default(), &secrets);

    assert_eq!(resolved.provider, ProviderKind::Deepseek);
    assert_eq!(resolved.base_url, "http://127.0.0.1:8000/v1");
    assert_eq!(resolved.api_key, None);
    assert!(
        store.gets.lock().unwrap().is_empty(),
        "loopback custom endpoints should not read macOS Keychain or any secret store"
    );
}

#[test]
fn remote_custom_provider_endpoint_does_not_reuse_ambient_official_credentials() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only env mutation guarded by env_lock().
    unsafe { env::set_var("OPENROUTER_API_KEY", "ambient-official-key") };
    let store = Arc::new(RecordingSecretsStore::with_value("saved-official-key"));
    let secrets = Secrets::new(store.clone());
    let mut config = ConfigToml {
        provider: ProviderKind::Openrouter,
        ..ConfigToml::default()
    };
    config.providers.openrouter.base_url = Some("https://gateway.example/v1".to_string());

    let resolved =
        config.resolve_runtime_options_with_secrets(&CliRuntimeOverrides::default(), &secrets);

    assert_eq!(resolved.base_url, "https://gateway.example/v1");
    assert_eq!(resolved.api_key, None);
    assert_eq!(resolved.api_key_source, None);
    assert!(
        store.gets.lock().unwrap().is_empty(),
        "a custom endpoint must not read the provider's global secret slot"
    );
}

#[test]
fn env_custom_provider_endpoint_does_not_reuse_ambient_official_credentials() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // A provider-specific base URL and API key are independent ambient
    // exports; the resolver cannot safely assume the key belongs to a remote
    // custom gateway. Custom routes must bind their credential explicitly.
    // Safety: test-only env mutation guarded by env_lock().
    unsafe {
        env::set_var("OPENROUTER_BASE_URL", "https://gateway.example/v1");
        env::set_var("OPENROUTER_API_KEY", "ambient-official-key");
    }
    let store = Arc::new(RecordingSecretsStore::with_value("saved-official-key"));
    let secrets = Secrets::new(store.clone());
    let config = ConfigToml {
        provider: ProviderKind::Openrouter,
        ..ConfigToml::default()
    };

    let resolved =
        config.resolve_runtime_options_with_secrets(&CliRuntimeOverrides::default(), &secrets);

    assert_eq!(resolved.base_url, "https://gateway.example/v1");
    assert_eq!(resolved.api_key, None);
    assert_eq!(resolved.api_key_source, None);
    assert!(
        store.gets.lock().unwrap().is_empty(),
        "an env-supplied custom endpoint must not read the provider's global secret slot"
    );
}

#[test]
fn remote_custom_provider_endpoint_accepts_only_explicitly_bound_credentials() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only env mutation guarded by env_lock().
    unsafe { env::set_var("OPENROUTER_API_KEY", "ambient-official-key") };
    let store = Arc::new(RecordingSecretsStore::with_value("saved-official-key"));
    let secrets = Secrets::new(store.clone());
    let mut config = ConfigToml {
        provider: ProviderKind::Openrouter,
        ..ConfigToml::default()
    };
    config.providers.openrouter.base_url = Some("https://gateway.example/v1".to_string());
    config.providers.openrouter.api_key = Some("route-bound-key".to_string());

    let resolved =
        config.resolve_runtime_options_with_secrets(&CliRuntimeOverrides::default(), &secrets);

    assert_eq!(resolved.api_key.as_deref(), Some("route-bound-key"));
    assert_eq!(
        resolved.api_key_source,
        Some(RuntimeApiKeySource::ConfigFile)
    );
    assert!(store.gets.lock().unwrap().is_empty());
}

#[test]
fn auth_mode_none_suppresses_every_runtime_credential_source() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only env mutation guarded by env_lock().
    unsafe { env::set_var("OPENROUTER_API_KEY", "ambient-key") };
    let store = Arc::new(RecordingSecretsStore::with_value("saved-key"));
    let secrets = Secrets::new(store.clone());
    let mut config = ConfigToml {
        provider: ProviderKind::Openrouter,
        auth_mode: Some("none".to_string()),
        ..ConfigToml::default()
    };
    config.providers.openrouter.api_key = Some("configured-key".to_string());
    config.http_headers.insert(
        "aUtHoRiZaTiOn".to_string(),
        "Bearer configured-header-secret".to_string(),
    );
    config.providers.openrouter.http_headers.insert(
        "X-API-Key".to_string(),
        "configured-x-api-key-secret".to_string(),
    );
    config.providers.openrouter.http_headers.insert(
        "Api-Key".to_string(),
        "configured-api-key-secret".to_string(),
    );
    config.providers.openrouter.http_headers.extend([
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
    ]);
    config
        .providers
        .openrouter
        .http_headers
        .insert("X-Route-Metadata".to_string(), "safe".to_string());
    let cli = CliRuntimeOverrides {
        api_key: Some("explicit-cli-key".to_string()),
        ..CliRuntimeOverrides::default()
    };

    let resolved = config.resolve_runtime_options_with_secrets(&cli, &secrets);

    assert_eq!(resolved.api_key, None);
    assert_eq!(resolved.api_key_source, None);
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
            !resolved
                .http_headers
                .keys()
                .any(|candidate| candidate.eq_ignore_ascii_case(name)),
            "disabled auth leaked {name}: {:?}",
            resolved.http_headers
        );
    }
    assert_eq!(
        resolved
            .http_headers
            .get("X-Route-Metadata")
            .map(String::as_str),
        Some("safe")
    );
    assert!(
        store.gets.lock().unwrap().is_empty(),
        "disabled auth must not probe any durable credential source"
    );
}

#[test]
fn deepseek_official_endpoint_family_shares_canonical_model_namespace() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();

    for base_url in [
        "https://api.deepseek.com",
        "https://api.deepseek.com/v1/",
        "https://api.deepseek.com/beta",
    ] {
        assert!(provider_base_url_is_official(
            ProviderKind::Deepseek,
            base_url
        ));
        assert!(!provider_preserves_custom_base_url_model(
            ProviderKind::Deepseek,
            base_url
        ));
    }
    assert!(!provider_base_url_is_official(
        ProviderKind::Deepseek,
        "https://api.deepseek.com.evil.example/v1"
    ));
}

#[test]
fn ollama_provider_preserves_model_tags() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let cli = CliRuntimeOverrides {
        provider: Some(ProviderKind::Ollama),
        model: Some("deepseek-coder-v2:16b".to_string()),
        ..CliRuntimeOverrides::default()
    };

    let resolved = ConfigToml::default().resolve_runtime_options(&cli);

    assert_eq!(resolved.provider, ProviderKind::Ollama);
    assert_eq!(resolved.model, "deepseek-coder-v2:16b");
}

#[test]
fn ollama_remote_env_url_does_not_inherit_ambient_optional_key() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only environment mutation guarded by a module mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "ollama-local");
        env::set_var("OLLAMA_BASE_URL", "http://ollama.example/v1");
        env::set_var("OLLAMA_API_KEY", "ollama-env-key");
    }

    let resolved = ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Ollama);
    assert_eq!(resolved.base_url, "http://ollama.example/v1");
    assert_eq!(resolved.api_key, None);
    assert_eq!(resolved.api_key_source, None);
}

#[test]
fn openrouter_env_overrides_key_and_model_when_config_missing() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only environment mutation guarded by a module mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "openrouter");
        env::set_var("OPENROUTER_API_KEY", "or-env-key");
        env::set_var("OPENROUTER_MODEL", "deepseek-v4-flash");
    }

    let resolved = ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Openrouter);
    assert_eq!(resolved.api_key.as_deref(), Some("or-env-key"));
    assert_eq!(resolved.base_url, DEFAULT_OPENROUTER_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_OPENROUTER_FLASH_MODEL);
}

#[test]
fn xiaomi_mimo_custom_env_url_does_not_inherit_ambient_key() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only environment mutation guarded by a module mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "xiaomi-mimo");
        env::set_var("MIMO_API_KEY", "mimo-env-key");
        env::set_var("MIMO_BASE_URL", "https://mimo-gateway.example/v1");
        env::set_var("MIMO_MODEL", "mimo-v2.5");
    }

    let resolved = ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::XiaomiMimo);
    assert_eq!(resolved.api_key, None);
    assert_eq!(resolved.api_key_source, None);
    assert_eq!(resolved.base_url, "https://mimo-gateway.example/v1");
    assert_eq!(resolved.model, "mimo-v2.5");
}

#[test]
fn xiaomi_mimo_env_token_plan_mode_uses_token_plan_key_and_endpoint() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only environment mutation guarded by a module mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "xiaomi-mimo");
        env::set_var("XIAOMI_MIMO_MODE", "token-plan-cn");
        env::set_var("XIAOMI_MIMO_TOKEN_PLAN_API_KEY", "tp-env-key");
        env::set_var("XIAOMI_MIMO_API_KEY", "sk-env-key");
    }

    let resolved = ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::XiaomiMimo);
    assert_eq!(resolved.api_key.as_deref(), Some("tp-env-key"));
    assert_eq!(resolved.api_key_source, Some(RuntimeApiKeySource::Env));
    assert_eq!(resolved.base_url, XIAOMI_MIMO_TOKEN_PLAN_CN_BASE_URL);
}

#[test]
fn xiaomi_mimo_env_pay_as_you_go_mode_prefers_standard_key() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only environment mutation guarded by a module mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "xiaomi-mimo");
        env::set_var("XIAOMI_MIMO_MODE", "pay-as-you-go");
        env::set_var("XIAOMI_MIMO_TOKEN_PLAN_API_KEY", "tp-env-key");
        env::set_var("XIAOMI_MIMO_API_KEY", "sk-env-key");
    }

    let resolved = ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::XiaomiMimo);
    assert_eq!(resolved.api_key.as_deref(), Some("sk-env-key"));
    assert_eq!(resolved.api_key_source, Some(RuntimeApiKeySource::Env));
    assert_eq!(resolved.base_url, XIAOMI_MIMO_PAY_AS_YOU_GO_BASE_URL);
}

#[test]
fn novita_env_overrides_key_and_model_when_config_missing() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only environment mutation guarded by a module mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "novita");
        env::set_var("NOVITA_API_KEY", "novita-env-key");
        env::set_var("NOVITA_MODEL", "deepseek-v4-flash");
    }

    let resolved = ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Novita);
    assert_eq!(resolved.api_key.as_deref(), Some("novita-env-key"));
    assert_eq!(resolved.base_url, DEFAULT_NOVITA_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_NOVITA_FLASH_MODEL);
}

#[test]
fn fireworks_env_overrides_key_and_model_when_config_missing() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only environment mutation guarded by a module mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "fireworks");
        env::set_var("FIREWORKS_API_KEY", "fw-env-key");
        env::set_var(
            "FIREWORKS_MODEL",
            "accounts/fireworks/models/account-specific-model",
        );
    }

    let resolved = ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Fireworks);
    assert_eq!(resolved.api_key.as_deref(), Some("fw-env-key"));
    assert_eq!(resolved.base_url, DEFAULT_FIREWORKS_BASE_URL);
    assert_eq!(
        resolved.model,
        "accounts/fireworks/models/account-specific-model"
    );
}

#[test]
fn siliconflow_custom_env_url_does_not_inherit_ambient_key() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only environment mutation guarded by a module mutex.
    unsafe {
        env::set_var("CODEWHALE_PROVIDER", "siliconflow");
        env::set_var("SILICONFLOW_API_KEY", "sf-env-key");
        env::set_var("SILICONFLOW_BASE_URL", "https://sf-mirror.example/v1");
        env::set_var("SILICONFLOW_MODEL", "deepseek-v4-flash");
    }

    let resolved = ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Siliconflow);
    assert_eq!(resolved.api_key, None);
    assert_eq!(resolved.api_key_source, None);
    assert_eq!(resolved.base_url, "https://sf-mirror.example/v1");
    assert_eq!(resolved.model, "deepseek-v4-flash");
}

#[test]
fn arcee_provider_defaults_to_direct_api_endpoint_and_model() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let config = ConfigToml {
        provider: ProviderKind::Arcee,
        ..ConfigToml::default()
    };

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Arcee);
    assert_eq!(resolved.base_url, DEFAULT_ARCEE_BASE_URL);
    assert_eq!(resolved.model, DEFAULT_ARCEE_MODEL);
}

#[test]
fn arcee_custom_env_url_does_not_inherit_ambient_key() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only environment mutation guarded by a module mutex.
    unsafe {
        env::set_var("CODEWHALE_PROVIDER", "arcee");
        env::set_var("ARCEE_API_KEY", "arcee-env-key");
        env::set_var("ARCEE_BASE_URL", "https://arcee-mirror.example/api/v1");
        env::set_var("ARCEE_MODEL", "trinity-large-preview");
    }

    let resolved = ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Arcee);
    assert_eq!(resolved.api_key, None);
    assert_eq!(resolved.api_key_source, None);
    assert_eq!(resolved.base_url, "https://arcee-mirror.example/api/v1");
    assert_eq!(resolved.model, "trinity-large-preview");
}

#[test]
fn arcee_provider_config_overrides_runtime_defaults() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let mut config = ConfigToml {
        provider: ProviderKind::Arcee,
        ..ConfigToml::default()
    };
    config.providers.arcee.api_key = Some("arcee-file-key".to_string());
    config.providers.arcee.base_url = Some(DEFAULT_ARCEE_BASE_URL.to_string());
    config.providers.arcee.model = Some("arcee-trinity-large-preview".to_string());

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Arcee);
    assert_eq!(resolved.api_key.as_deref(), Some("arcee-file-key"));
    assert_eq!(resolved.base_url, DEFAULT_ARCEE_BASE_URL);
    assert_eq!(resolved.model, ARCEE_TRINITY_LARGE_PREVIEW_MODEL);
}

#[test]
fn huggingface_custom_env_url_does_not_inherit_documented_key_names() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only environment mutation guarded by a module mutex.
    unsafe {
        env::set_var("CODEWHALE_PROVIDER", "hf");
        env::set_var("HUGGINGFACE_API_KEY", "hf-full-key");
        env::set_var("HF_TOKEN", "hf-token-fallback");
        env::set_var("HUGGINGFACE_BASE_URL", "https://hf-full.example/v1");
        env::set_var("HF_BASE_URL", "https://hf-short.example/v1");
        env::set_var("HUGGINGFACE_MODEL", "org/full-model");
        env::set_var("HF_MODEL", "org/short-model");
    }

    let resolved = ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Huggingface);
    assert_eq!(resolved.api_key, None);
    assert_eq!(resolved.api_key_source, None);
    assert_eq!(resolved.base_url, "https://hf-full.example/v1");
    assert_eq!(resolved.model, "org/full-model");
}

#[test]
fn huggingface_short_custom_env_url_does_not_inherit_ambient_token() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only environment mutation guarded by a module mutex.
    unsafe {
        env::set_var("CODEWHALE_PROVIDER", "huggingface");
        env::set_var("HF_TOKEN", "hf-token-fallback");
        env::set_var("HF_BASE_URL", "https://hf-short.example/v1");
        env::set_var("HF_MODEL", "org/short-model");
    }

    let resolved = ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Huggingface);
    assert_eq!(resolved.api_key, None);
    assert_eq!(resolved.api_key_source, None);
    assert_eq!(resolved.base_url, "https://hf-short.example/v1");
    assert_eq!(resolved.model, "org/short-model");
}

#[test]
fn huggingface_token_fallback_resolves_when_primary_api_key_is_blank() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only environment mutation guarded by a module mutex.
    unsafe {
        env::set_var("CODEWHALE_PROVIDER", "huggingface");
        env::set_var("HUGGINGFACE_API_KEY", " ");
        env::set_var("HF_TOKEN", "hf-token-fallback");
    }

    let resolved = ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Huggingface);
    assert_eq!(resolved.api_key.as_deref(), Some("hf-token-fallback"));
}

#[test]
fn siliconflow_cn_base_url_env_normalizes_model_aliases() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only environment mutation guarded by a module mutex.
    unsafe {
        env::set_var("CODEWHALE_PROVIDER", "siliconflow");
        env::set_var("SILICONFLOW_API_KEY", "sf-env-key");
        env::set_var("SILICONFLOW_BASE_URL", "https://api.siliconflow.cn/v1");
    }

    for (alias, expected) in [
        ("deepseek-v4-flash", DEFAULT_SILICONFLOW_FLASH_MODEL),
        ("deepseek-reasoner", DEFAULT_SILICONFLOW_MODEL),
    ] {
        // Safety: test-only environment mutation guarded by a module mutex.
        unsafe {
            env::set_var("SILICONFLOW_MODEL", alias);
        }

        let resolved =
            ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

        assert_eq!(resolved.provider, ProviderKind::Siliconflow);
        assert_eq!(resolved.base_url, "https://api.siliconflow.cn/v1");
        assert_eq!(resolved.model, expected);
    }
}

#[test]
fn wanjie_ark_custom_env_url_does_not_inherit_ambient_key() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only environment mutation guarded by a module mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "wanjie-ark");
        env::set_var("WANJIE_ARK_API_KEY", "wanjie-env-key");
        env::set_var("WANJIE_ARK_BASE_URL", "https://wanjie.example/api/v1");
        env::set_var("WANJIE_ARK_MODEL", "account-model-id");
    }

    let resolved = ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::WanjieArk);
    assert_eq!(resolved.api_key, None);
    assert_eq!(resolved.api_key_source, None);
    assert_eq!(resolved.base_url, "https://wanjie.example/api/v1");
    assert_eq!(resolved.model, "account-model-id");
}

#[test]
fn volcengine_custom_env_alias_url_does_not_inherit_ambient_key() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: test-only environment mutation guarded by a module mutex.
    unsafe {
        env::set_var("DEEPSEEK_PROVIDER", "volcengine");
        env::set_var("ARK_API_KEY", "volcengine-env-key");
        env::set_var("ARK_BASE_URL", "https://volcengine.example/api/coding/v3");
        env::set_var("VOLCENGINE_ARK_MODEL", "DeepSeek-V4-Flash");
    }

    let resolved = ConfigToml::default().resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Volcengine);
    assert_eq!(resolved.api_key, None);
    assert_eq!(resolved.api_key_source, None);
    assert_eq!(
        resolved.base_url,
        "https://volcengine.example/api/coding/v3"
    );
    assert_eq!(resolved.model, "DeepSeek-V4-Flash");
}

#[test]
fn openrouter_provider_normalizes_flash_aliases() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let cli = CliRuntimeOverrides {
        provider: Some(ProviderKind::Openrouter),
        model: Some("deepseek-v4-flash".to_string()),
        ..CliRuntimeOverrides::default()
    };

    let resolved = ConfigToml::default().resolve_runtime_options(&cli);

    assert_eq!(resolved.provider, ProviderKind::Openrouter);
    assert_eq!(resolved.model, DEFAULT_OPENROUTER_FLASH_MODEL);
}

#[test]
fn qwen3_6_plus_resolves_to_canonical_on_openrouter() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let config = ConfigToml {
        provider: ProviderKind::Openrouter,
        ..ConfigToml::default()
    };

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides {
        model: Some("qwen3.6-plus".to_string()),
        ..CliRuntimeOverrides::default()
    });

    assert_eq!(resolved.provider, ProviderKind::Openrouter);
    assert_eq!(resolved.model, OPENROUTER_QWEN_3_6_PLUS_MODEL);
}

#[test]
fn qwen3_6_plus_alias_qwen_dash_resolves() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let config = ConfigToml {
        provider: ProviderKind::Openrouter,
        ..ConfigToml::default()
    };

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides {
        model: Some("qwen-3.6-plus".to_string()),
        ..CliRuntimeOverrides::default()
    });

    assert_eq!(resolved.model, OPENROUTER_QWEN_3_6_PLUS_MODEL);
}

#[test]
fn openrouter_provider_normalizes_recent_large_model_aliases() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();

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
        let cli = CliRuntimeOverrides {
            provider: Some(ProviderKind::Openrouter),
            model: Some(alias.to_string()),
            ..CliRuntimeOverrides::default()
        };

        let resolved = ConfigToml::default().resolve_runtime_options(&cli);

        assert_eq!(resolved.provider, ProviderKind::Openrouter);
        assert_eq!(resolved.model, expected);
    }
}

#[test]
fn novita_provider_normalizes_flash_aliases() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let cli = CliRuntimeOverrides {
        provider: Some(ProviderKind::Novita),
        model: Some("deepseek-v4-flash".to_string()),
        ..CliRuntimeOverrides::default()
    };

    let resolved = ConfigToml::default().resolve_runtime_options(&cli);

    assert_eq!(resolved.provider, ProviderKind::Novita);
    assert_eq!(resolved.model, DEFAULT_NOVITA_FLASH_MODEL);
}

#[test]
fn siliconflow_provider_normalizes_flash_aliases() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let cli = CliRuntimeOverrides {
        provider: Some(ProviderKind::Siliconflow),
        model: Some("deepseek-v4-flash".to_string()),
        ..CliRuntimeOverrides::default()
    };

    let resolved = ConfigToml::default().resolve_runtime_options(&cli);

    assert_eq!(resolved.provider, ProviderKind::Siliconflow);
    assert_eq!(resolved.model, DEFAULT_SILICONFLOW_FLASH_MODEL);
}

#[test]
fn siliconflow_provider_normalizes_reasoning_aliases_to_pro() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();

    for alias in ["deepseek-reasoner", "deepseek-r1"] {
        let cli = CliRuntimeOverrides {
            provider: Some(ProviderKind::Siliconflow),
            model: Some(alias.to_string()),
            ..CliRuntimeOverrides::default()
        };

        let resolved = ConfigToml::default().resolve_runtime_options(&cli);

        assert_eq!(resolved.provider, ProviderKind::Siliconflow);
        assert_eq!(resolved.model, DEFAULT_SILICONFLOW_MODEL);
    }
}

#[test]
fn siliconflow_provider_preserves_deepseek_v3_2_alias() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let cli = CliRuntimeOverrides {
        provider: Some(ProviderKind::Siliconflow),
        model: Some("deepseek-v3.2".to_string()),
        ..CliRuntimeOverrides::default()
    };

    let resolved = ConfigToml::default().resolve_runtime_options(&cli);

    assert_eq!(resolved.provider, ProviderKind::Siliconflow);
    assert_eq!(resolved.model, "deepseek-v3.2");
}

#[test]
fn sglang_provider_normalizes_flash_aliases() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let cli = CliRuntimeOverrides {
        provider: Some(ProviderKind::Sglang),
        model: Some("deepseek-v4-flash".to_string()),
        ..CliRuntimeOverrides::default()
    };

    let resolved = ConfigToml::default().resolve_runtime_options(&cli);

    assert_eq!(resolved.provider, ProviderKind::Sglang);
    assert_eq!(resolved.model, DEFAULT_SGLANG_FLASH_MODEL);
}

#[test]
fn vllm_provider_normalizes_flash_aliases() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let cli = CliRuntimeOverrides {
        provider: Some(ProviderKind::Vllm),
        model: Some("deepseek-v4-flash".to_string()),
        ..CliRuntimeOverrides::default()
    };

    let resolved = ConfigToml::default().resolve_runtime_options(&cli);

    assert_eq!(resolved.provider, ProviderKind::Vllm);
    assert_eq!(resolved.model, DEFAULT_VLLM_FLASH_MODEL);
}

#[test]
fn openrouter_provider_specific_config_overrides_env() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let mut config = ConfigToml {
        provider: ProviderKind::Openrouter,
        ..ConfigToml::default()
    };
    config.providers.openrouter.api_key = Some("file-key".to_string());
    config.providers.openrouter.base_url = Some("https://or-mirror.example/v1".to_string());

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.api_key.as_deref(), Some("file-key"));
    assert_eq!(resolved.base_url, "https://or-mirror.example/v1");
}

#[test]
fn openrouter_custom_base_url_preserves_provider_model() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let mut config = ConfigToml {
        provider: ProviderKind::Openrouter,
        ..ConfigToml::default()
    };
    config.providers.openrouter.base_url = Some("https://gateway.example.com/v1".to_string());
    config.providers.openrouter.model = Some("DeepSeek-V4-Pro".to_string());

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Openrouter);
    assert_eq!(resolved.base_url, "https://gateway.example.com/v1");
    assert_eq!(resolved.model, "DeepSeek-V4-Pro");
}

#[test]
fn openai_compatible_tokenhub_route_preserves_provider_scope() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let mut config = ConfigToml {
        provider: ProviderKind::Openai,
        ..ConfigToml::default()
    };
    config.providers.openai.api_key = Some("tokenhub-file-key".to_string());
    config.providers.openai.base_url = Some("https://tokenhub.tencentmaas.com/v1".to_string());
    config.providers.openai.model = Some("deepseek-ai/DeepSeek-V4-Pro".to_string());

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Openai);
    assert_eq!(resolved.api_key.as_deref(), Some("tokenhub-file-key"));
    assert_eq!(resolved.base_url, "https://tokenhub.tencentmaas.com/v1");
    assert_eq!(resolved.model, "deepseek-ai/DeepSeek-V4-Pro");
}

#[test]
fn openrouter_compatible_base_url_preserves_namespaced_wire_model() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let mut config = ConfigToml {
        provider: ProviderKind::Openrouter,
        ..ConfigToml::default()
    };
    config.providers.openrouter.base_url = Some("https://openrouter-compatible.example/v1".into());
    config.providers.openrouter.model = Some("deepseek/deepseek-v4-pro".into());

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Openrouter);
    assert_eq!(resolved.provider_source, ProviderSource::Config);
    assert_eq!(
        resolved.base_url,
        "https://openrouter-compatible.example/v1"
    );
    assert_eq!(resolved.model, "deepseek/deepseek-v4-pro");
}

#[test]
fn fireworks_custom_base_url_preserves_provider_model() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let mut config = ConfigToml {
        provider: ProviderKind::Fireworks,
        ..ConfigToml::default()
    };
    config.providers.fireworks.base_url = Some("https://my-gateway.example/v1".to_string());
    config.providers.fireworks.model = Some("DeepSeek-V4-Pro".to_string());

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Fireworks);
    assert_eq!(resolved.base_url, "https://my-gateway.example/v1");
    // Custom base URL skips provider-specific model prefixing.
    assert_eq!(resolved.model, "DeepSeek-V4-Pro");
}

#[test]
fn siliconflow_custom_base_url_preserves_provider_model() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let mut config = ConfigToml {
        provider: ProviderKind::Siliconflow,
        ..ConfigToml::default()
    };
    config.providers.siliconflow.base_url = Some("https://my-gateway.example/v1".to_string());
    config.providers.siliconflow.model = Some("DeepSeek-V4-Pro".to_string());

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::Siliconflow);
    assert_eq!(resolved.base_url, "https://my-gateway.example/v1");
    assert_eq!(resolved.model, "DeepSeek-V4-Pro");
}

#[test]
fn config_file_resolves_above_env_and_keyring() {
    use codewhale_secrets::KeyringStore;
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: env mutation guarded by env_lock().
    unsafe { std::env::set_var("DEEPSEEK_API_KEY", "env-key") };

    let store = std::sync::Arc::new(codewhale_secrets::InMemoryKeyringStore::new());
    store.set("deepseek", "ring-key").unwrap();
    let secrets = Secrets::new(store);

    let mut config = ConfigToml::default();
    config.providers.deepseek.api_key = Some("file-key".to_string());

    let resolved =
        config.resolve_runtime_options_with_secrets(&CliRuntimeOverrides::default(), &secrets);
    assert_eq!(resolved.api_key.as_deref(), Some("file-key"));
    assert_eq!(
        resolved.api_key_source,
        Some(RuntimeApiKeySource::ConfigFile)
    );

    // Safety: env mutation guarded by env_lock().
    unsafe { std::env::remove_var("DEEPSEEK_API_KEY") };
}

#[test]
fn env_resolves_when_config_file_and_keyring_empty() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: env mutation guarded by env_lock().
    unsafe { std::env::set_var("DEEPSEEK_API_KEY", "env-key") };

    let secrets = Secrets::new(std::sync::Arc::new(
        codewhale_secrets::InMemoryKeyringStore::new(),
    ));
    let config = ConfigToml::default();

    let resolved =
        config.resolve_runtime_options_with_secrets(&CliRuntimeOverrides::default(), &secrets);
    assert_eq!(resolved.api_key.as_deref(), Some("env-key"));
    assert_eq!(resolved.api_key_source, Some(RuntimeApiKeySource::Env));

    // Safety: env mutation guarded by env_lock().
    unsafe { std::env::remove_var("DEEPSEEK_API_KEY") };
}

#[test]
fn config_file_resolves_when_keyring_and_env_empty() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();

    let secrets = Secrets::new(std::sync::Arc::new(
        codewhale_secrets::InMemoryKeyringStore::new(),
    ));
    let mut config = ConfigToml::default();
    config.providers.deepseek.api_key = Some("file-key".to_string());

    let resolved =
        config.resolve_runtime_options_with_secrets(&CliRuntimeOverrides::default(), &secrets);
    assert_eq!(resolved.api_key.as_deref(), Some("file-key"));
    assert_eq!(
        resolved.api_key_source,
        Some(RuntimeApiKeySource::ConfigFile)
    );
}

#[test]
fn keyring_resolves_when_config_file_empty_even_if_env_is_set() {
    use codewhale_secrets::KeyringStore;
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    // Safety: env mutation guarded by env_lock().
    unsafe { std::env::set_var("DEEPSEEK_API_KEY", "stale-env-key") };

    let store = std::sync::Arc::new(codewhale_secrets::InMemoryKeyringStore::new());
    store.set("deepseek", "ring-key").unwrap();
    let secrets = Secrets::new(store);

    let resolved = ConfigToml::default()
        .resolve_runtime_options_with_secrets(&CliRuntimeOverrides::default(), &secrets);
    assert_eq!(resolved.api_key.as_deref(), Some("ring-key"));
    assert_eq!(resolved.api_key_source, Some(RuntimeApiKeySource::Keyring));

    // Safety: env mutation guarded by env_lock().
    unsafe { std::env::remove_var("DEEPSEEK_API_KEY") };
}

#[test]
fn cli_flag_still_overrides_keyring() {
    use codewhale_secrets::KeyringStore;
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();

    let store = std::sync::Arc::new(codewhale_secrets::InMemoryKeyringStore::new());
    store.set("deepseek", "ring-key").unwrap();
    let secrets = Secrets::new(store);

    let cli = CliRuntimeOverrides {
        api_key: Some("cli-key".to_string()),
        ..CliRuntimeOverrides::default()
    };
    let resolved = ConfigToml::default().resolve_runtime_options_with_secrets(&cli, &secrets);
    assert_eq!(resolved.api_key.as_deref(), Some("cli-key"));
    assert_eq!(resolved.api_key_source, Some(RuntimeApiKeySource::Cli));
}

#[test]
fn provider_chain_initial_current_is_active() {
    let chain = ProviderChain::new(
        ProviderKind::NvidiaNim,
        &[ProviderKind::Deepseek, ProviderKind::Openrouter],
    );

    assert_eq!(chain.current(), ProviderKind::NvidiaNim);
    assert_eq!(chain.position(), 0);
    assert_eq!(
        chain.providers(),
        &[
            ProviderKind::NvidiaNim,
            ProviderKind::Deepseek,
            ProviderKind::Openrouter,
        ]
    );
    assert!(!chain.is_fallback_active());
}

#[test]
fn provider_chain_advance_switches_to_fallback() {
    let mut chain = ProviderChain::new(
        ProviderKind::NvidiaNim,
        &[ProviderKind::Deepseek, ProviderKind::Openrouter],
    );

    assert!(chain.has_next());
    assert_eq!(chain.advance(), Some(ProviderKind::Deepseek));
    assert_eq!(chain.current(), ProviderKind::Deepseek);
    assert!(chain.is_fallback_active());
}

#[test]
fn provider_chain_exhausts_returns_none() {
    let mut chain = ProviderChain::new(ProviderKind::Deepseek, &[ProviderKind::Openrouter]);

    assert_eq!(chain.advance(), Some(ProviderKind::Openrouter));
    assert!(!chain.has_next());
    assert_eq!(chain.advance(), None);
}

#[test]
fn provider_chain_skips_duplicates() {
    let chain = ProviderChain::new(
        ProviderKind::Deepseek,
        &[
            ProviderKind::Deepseek,
            ProviderKind::NvidiaNim,
            ProviderKind::Deepseek,
        ],
    );

    assert_eq!(
        chain.providers(),
        &[ProviderKind::Deepseek, ProviderKind::NvidiaNim]
    );
}

#[test]
fn provider_chain_remaining_counts_current_and_untried_entries() {
    let mut chain = ProviderChain::new(
        ProviderKind::Deepseek,
        &[ProviderKind::NvidiaNim, ProviderKind::Openrouter],
    );

    assert_eq!(chain.remaining(), 3);
    assert_eq!(chain.advance(), Some(ProviderKind::NvidiaNim));
    assert_eq!(chain.remaining(), 2);
}

#[test]
fn config_toml_parses_fallback_providers() {
    let config: ConfigToml = toml::from_str(
        r#"
provider = "nvidia-nim"
fallback_providers = ["deepseek", "openrouter"]
"#,
    )
    .expect("fallback providers config");

    assert_eq!(config.provider, ProviderKind::NvidiaNim);
    assert_eq!(
        config.fallback_providers,
        [ProviderKind::Deepseek, ProviderKind::Openrouter]
    );
}

#[test]
fn empty_fallback_providers_do_not_serialize() {
    let serialized = toml::to_string_pretty(&ConfigToml::default()).expect("config serializes");

    assert!(!serialized.contains("fallback_providers"));
}

#[test]
fn empty_provider_header_tables_do_not_survive_round_trip() {
    let polluted = r#"
[http_headers]

[providers.anthropic.http_headers]
" " = "ignored"
"X-Blank" = "   "

[providers.openrouter.http_headers]

[providers.xai]
model = "   "
"#;
    let config: ConfigToml = toml::from_str(polluted).expect("polluted config parses");
    let serialized = toml::to_string_pretty(&config).expect("config serializes");

    assert!(
        !serialized.contains("[http_headers]"),
        "empty root headers must not be serialized:\n{serialized}"
    );
    assert!(
        !serialized.contains("[providers.anthropic"),
        "empty Anthropic provider state must not be serialized:\n{serialized}"
    );
    assert!(
        !serialized.contains("[providers.openrouter"),
        "empty OpenRouter provider state must not be serialized:\n{serialized}"
    );
    assert!(
        !serialized.contains("[providers.xai"),
        "blank provider fields must not be serialized:\n{serialized}"
    );

    let round_tripped: ConfigToml = toml::from_str(&serialized).expect("canonical config parses");
    assert!(round_tripped.http_headers.is_empty());
    assert!(round_tripped.providers.anthropic.http_headers.is_empty());
    assert!(round_tripped.providers.openrouter.http_headers.is_empty());
}

#[test]
fn workflow_config_defaults_match_product_surface() {
    // #4128 / Section 2.11: omitted `[workflow]` keys resolve to the
    // documented product defaults so launch/approval/persist share one model.
    let defaults = WorkflowConfigToml::default();
    assert!(defaults.automatic);
    assert!(defaults.auto_start_read_only);
    assert!(defaults.require_approval_for_writes);
    assert_eq!(defaults.auto_start_child_limit, 16);
    assert_eq!(defaults.max_children, 1000);
    assert_eq!(defaults.max_concurrent, 16);
    assert_eq!(defaults.max_depth, 2);
    assert_eq!(defaults.default_token_budget, 120_000);
    assert_eq!(defaults.max_parallel_writes_without_worktree, 0);
    assert!(defaults.persist_completed_activity);
    assert!(defaults.persist_completed_across_restarts);
}

#[test]
fn workflow_config_absent_table_stays_none_empty_table_fills_defaults() {
    let absent: ConfigToml = toml::from_str("").expect("empty config parses");
    assert!(absent.workflow.is_none());

    let empty_table: ConfigToml = toml::from_str(
        r#"
[workflow]
"#,
    )
    .expect("empty workflow table should parse");
    assert_eq!(
        empty_table.workflow.expect("workflow table present"),
        WorkflowConfigToml::default()
    );
}

#[test]
fn workflow_config_partial_override_and_round_trip() {
    let config: ConfigToml = toml::from_str(
        r#"
[workflow]
automatic = false
max_children = 16
default_token_budget = 50000
"#,
    )
    .expect("workflow overrides should parse");

    let workflow = config.workflow.expect("workflow table");
    assert!(!workflow.automatic);
    assert_eq!(workflow.max_children, 16);
    assert_eq!(workflow.default_token_budget, 50_000);
    // Unset keys keep product defaults.
    assert!(workflow.auto_start_read_only);
    assert!(workflow.require_approval_for_writes);
    assert_eq!(workflow.auto_start_child_limit, 16);
    assert_eq!(workflow.max_concurrent, 16);
    assert_eq!(workflow.max_depth, 2);
    assert_eq!(workflow.max_parallel_writes_without_worktree, 0);
    assert!(workflow.persist_completed_activity);
    assert!(workflow.persist_completed_across_restarts);

    let serialized = toml::to_string_pretty(&workflow).expect("workflow serializes");
    let round_tripped: WorkflowConfigToml =
        toml::from_str(&serialized).expect("serialized workflow parses");
    assert_eq!(round_tripped, workflow);
}

#[test]
fn fleet_exec_config_default_matches_subagent_depth() {
    // Fleet workers and standalone sub-agents share one recursion axis:
    // the fleet default equals DEFAULT_SPAWN_DEPTH (3) and affords >=3
    // nested delegation levels out of the box.
    assert_eq!(
        FleetExecConfig::default().max_spawn_depth,
        DEFAULT_SPAWN_DEPTH
    );
    assert_eq!(FleetExecConfig::default().max_spawn_depth, 3);
    const { assert!(DEFAULT_SPAWN_DEPTH <= MAX_SPAWN_DEPTH_CEILING) };
}

#[test]
fn fleet_exec_config_parses_max_spawn_depth() {
    let config: ConfigToml = toml::from_str(
        r#"
[fleet.exec]
max_spawn_depth = 2
"#,
    )
    .expect("fleet exec config should parse");

    assert_eq!(config.fleet.expect("fleet config").exec.max_spawn_depth, 2);
}

#[test]
fn fleet_profile_defaults_round_trip_through_config() {
    let config: ConfigToml = toml::from_str(
        r#"
[fleet.profiles.default]
"#,
    )
    .expect("fleet profile config should parse");

    let profile = config
        .fleet
        .expect("fleet config")
        .profiles
        .get("default")
        .expect("default profile")
        .clone();

    assert_eq!(profile, FleetProfile::default());
    assert!(!profile.permissions.allow_shell);
    assert!(!profile.permissions.trust);
    assert!(profile.permissions.approval_required);

    let serialized = toml::to_string_pretty(&profile).expect("profile serializes");
    let round_tripped: FleetProfile =
        toml::from_str(&serialized).expect("serialized profile parses");
    assert_eq!(round_tripped, profile);
}

#[test]
fn fleet_profile_explicit_config_parses_role_loadout_permissions() {
    let config: ConfigToml = toml::from_str(
        r#"
[fleet.profiles.verifier]
slot = "verifier"
loadout = "review"
model = "deepseek-v4-pro"

[fleet.profiles.verifier.role]
name = "verifier"
description = "Read-only verification worker"
instructions = "Check the patch and report evidence."

[fleet.profiles.verifier.permissions]
allow_shell = false
trust = false
approval_required = true

[fleet.profiles.verifier.delegation]
max_spawn_depth = 0
concurrency = 3
"#,
    )
    .expect("fleet profile config should parse");

    let profile = config
        .fleet
        .expect("fleet config")
        .profiles
        .get("verifier")
        .expect("verifier profile")
        .clone();

    assert_eq!(profile.slot, FleetSlot::Verifier);
    assert_eq!(profile.role.name, "verifier");
    assert_eq!(
        profile.role.description.as_deref(),
        Some("Read-only verification worker")
    );
    assert_eq!(
        profile.role.instructions.as_deref(),
        Some("Check the patch and report evidence.")
    );
    // "review" was a retired decorative tier: it parses as Custom and keeps
    // the same auto routing it always had.
    assert_eq!(profile.loadout, FleetLoadout::Custom("review".to_string()));
    assert_eq!(profile.model.as_deref(), Some("deepseek-v4-pro"));
    assert!(!profile.permissions.allow_shell);
    assert!(!profile.permissions.trust);
    assert!(profile.permissions.approval_required);
    assert_eq!(profile.delegation.max_spawn_depth, Some(0));
    assert_eq!(profile.delegation.max_concurrency, Some(3));
}

#[test]
fn fleet_loadout_accepts_default_model_classes() {
    assert_eq!(FleetLoadout::from_name("fast"), FleetLoadout::Fast);
    assert_eq!(FleetLoadout::from_name("inherit"), FleetLoadout::Inherit);
    assert_eq!(FleetLoadout::from_name(""), FleetLoadout::Inherit);
    assert_eq!(FleetLoadout::Fast.as_str(), "fast");
    // Retired tiers stay parseable as Custom so old configs keep loading
    // with identical (auto) routing.
    assert_eq!(
        FleetLoadout::from_name("strong"),
        FleetLoadout::Custom("strong".to_string())
    );
    assert_eq!(
        FleetLoadout::from_name("tool-heavy"),
        FleetLoadout::Custom("tool-heavy".to_string())
    );
    assert_eq!(
        FleetLoadout::Custom("strong".to_string()).as_str(),
        "strong"
    );
}

#[test]
fn fallback_providers_do_not_change_runtime_resolution() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let config = ConfigToml {
        provider: ProviderKind::NvidiaNim,
        fallback_providers: vec![ProviderKind::Deepseek],
        ..ConfigToml::default()
    };

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());

    assert_eq!(resolved.provider, ProviderKind::NvidiaNim);
}

#[test]
fn harness_posture_default_is_standard() {
    let posture = HarnessPosture::default();

    assert_eq!(
        posture,
        HarnessPosture {
            kind: HarnessPostureKind::Standard,
            max_subagents: 0,
            prefer_codebase_search: false,
            compaction_strategy: HarnessCompactionStrategy::Default,
            tool_surface: HarnessToolSurface::Full,
            safety_posture: HarnessSafetyPosture::Standard,
        }
    );
}

#[test]
fn harness_posture_factories_are_typed() {
    assert_eq!(
        HarnessPosture::cache_heavy(),
        HarnessPosture {
            kind: HarnessPostureKind::CacheHeavy,
            max_subagents: 10,
            prefer_codebase_search: false,
            compaction_strategy: HarnessCompactionStrategy::PrefixCache,
            tool_surface: HarnessToolSurface::Full,
            safety_posture: HarnessSafetyPosture::Standard,
        }
    );
    assert_eq!(
        HarnessPosture::lean(),
        HarnessPosture {
            kind: HarnessPostureKind::Lean,
            max_subagents: 20,
            prefer_codebase_search: true,
            compaction_strategy: HarnessCompactionStrategy::Aggressive,
            tool_surface: HarnessToolSurface::Full,
            safety_posture: HarnessSafetyPosture::Standard,
        }
    );
}

#[test]
fn harness_profile_serde_round_trips_as_a_whole_struct() {
    let profile = HarnessProfile {
        provider_route: "deepseek".to_string(),
        model_pattern: "deepseek-v4.*".to_string(),
        posture: HarnessPosture::cache_heavy(),
    };

    let json = serde_json::to_string(&profile).expect("serialize profile");
    let round_tripped: HarnessProfile = serde_json::from_str(&json).expect("deserialize profile");

    assert_eq!(round_tripped, profile);
}

#[test]
fn config_toml_accepts_harness_profiles() {
    let config: ConfigToml = toml::from_str(
        r#"
provider = "deepseek"
model = "deepseek-v4-pro"

[[harness_profiles]]
provider_route = "deepseek"
model_pattern = "deepseek-v4.*"

[harness_profiles.posture]
kind = "cache-heavy"
max_subagents = 10
compaction_strategy = "prefix-cache"
tool_surface = "read-only"
safety_posture = "strict"
"#,
    )
    .expect("parse harness profiles");

    assert_eq!(
        config.harness_profiles,
        vec![HarnessProfile {
            provider_route: "deepseek".to_string(),
            model_pattern: "deepseek-v4.*".to_string(),
            posture: HarnessPosture {
                kind: HarnessPostureKind::CacheHeavy,
                max_subagents: 10,
                prefer_codebase_search: false,
                compaction_strategy: HarnessCompactionStrategy::PrefixCache,
                tool_surface: HarnessToolSurface::ReadOnly,
                safety_posture: HarnessSafetyPosture::Strict,
            },
        }]
    );
}

#[test]
fn harness_profile_matches_provider_alias_and_model_wildcard() {
    let profile = HarnessProfile {
        provider_route: "xiaomi-mimo".to_string(),
        model_pattern: "mimo-v2.?-pro".to_string(),
        posture: HarnessPosture::cache_heavy(),
    };

    assert!(profile.matches_route("mimo", "mimo-v2.5-pro"));
    assert!(!profile.matches_route("mimo", "mimo-v2.50-pro"));
    assert!(!profile.matches_route("deepseek", "mimo-v2.5-pro"));
}

#[test]
fn resolve_harness_profile_returns_first_matching_profile() {
    let config = ConfigToml {
        harness_profiles: vec![
            HarnessProfile {
                provider_route: "deepseek".to_string(),
                model_pattern: "deepseek-v4-flash".to_string(),
                posture: HarnessPosture::lean(),
            },
            HarnessProfile {
                provider_route: "deepseek".to_string(),
                model_pattern: "deepseek-v4-*".to_string(),
                posture: HarnessPosture::cache_heavy(),
            },
        ],
        ..ConfigToml::default()
    };

    let flash = config
        .resolve_harness_profile("deepseek-cn", "deepseek-v4-flash")
        .expect("exact profile should match first");
    assert_eq!(flash.posture.kind, HarnessPostureKind::Lean);

    let pro = config
        .resolve_harness_profile("deepseek", "deepseek-v4-pro")
        .expect("wildcard profile should match pro model");
    assert_eq!(pro.posture.kind, HarnessPostureKind::CacheHeavy);
}

#[test]
fn resolve_harness_profile_uses_built_in_seed_when_config_has_no_match() {
    let config = ConfigToml::default();

    let xiaomi = config
        .resolve_harness_profile("xiaomi", "mimo-v2.5-pro")
        .expect("direct Xiaomi MiMo seed should resolve");
    assert_eq!(xiaomi.provider_route, "xiaomi-mimo");
    assert_eq!(xiaomi.posture.kind, HarnessPostureKind::CacheHeavy);

    let arcee = config
        .resolve_harness_profile("arcee", "trinity-large-thinking")
        .expect("direct Arcee seed should resolve");
    assert_eq!(arcee.posture.kind, HarnessPostureKind::CacheHeavy);

    let local = config
        .resolve_harness_profile("vllm", "Qwen/Qwen3.6-Coder")
        .expect("local seed should resolve");
    assert_eq!(local.posture.kind, HarnessPostureKind::Lean);
    assert!(local.posture.prefer_codebase_search);
}

#[test]
fn configured_harness_profile_overrides_built_in_seed() {
    let config = ConfigToml {
        harness_profiles: vec![HarnessProfile {
            provider_route: "xiaomi-mimo".to_string(),
            model_pattern: "mimo-v2.5-pro".to_string(),
            posture: HarnessPosture {
                kind: HarnessPostureKind::Custom,
                max_subagents: 3,
                prefer_codebase_search: true,
                compaction_strategy: HarnessCompactionStrategy::Default,
                tool_surface: HarnessToolSurface::Auto,
                safety_posture: HarnessSafetyPosture::Strict,
            },
        }],
        ..ConfigToml::default()
    };

    let profile = config
        .resolve_harness_profile("xiaomi-mimo", "mimo-v2.5-pro")
        .expect("configured profile should match first");

    assert_eq!(profile.posture.kind, HarnessPostureKind::Custom);
    assert_eq!(profile.posture.max_subagents, 3);
    assert_eq!(profile.posture.tool_surface, HarnessToolSurface::Auto);
    assert_eq!(profile.posture.safety_posture, HarnessSafetyPosture::Strict);
}

#[test]
fn resolve_harness_profile_returns_none_when_route_or_model_misses() {
    let config = ConfigToml {
        harness_profiles: vec![HarnessProfile {
            provider_route: "huggingface".to_string(),
            model_pattern: "deepseek-ai/*".to_string(),
            posture: HarnessPosture::lean(),
        }],
        ..ConfigToml::default()
    };

    assert!(
        config
            .resolve_harness_profile("openrouter", "deepseek-ai/DeepSeek-V4-Pro")
            .is_none()
    );
    assert!(
        config
            .resolve_harness_profile("deepseek", "Qwen/Qwen3.6-Coder")
            .is_none()
    );
    assert!(
        config
            .resolve_harness_profile("openai", "mimo-v2.5-pro")
            .is_none()
    );
}

#[test]
fn resolving_harness_profile_does_not_change_runtime_options() {
    let _lock = env_lock();
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    let config = ConfigToml {
        provider: ProviderKind::Deepseek,
        model: Some("deepseek-v4-pro".to_string()),
        harness_profiles: vec![HarnessProfile {
            provider_route: "deepseek".to_string(),
            model_pattern: "deepseek-v4-*".to_string(),
            posture: HarnessPosture::lean(),
        }],
        ..ConfigToml::default()
    };

    let profile = config
        .resolve_harness_profile("deepseek", "deepseek-v4-pro")
        .expect("profile should resolve for display/future runtime");
    assert_eq!(profile.posture.kind, HarnessPostureKind::Lean);

    let resolved = config.resolve_runtime_options(&CliRuntimeOverrides::default());
    assert_eq!(resolved.provider, ProviderKind::Deepseek);
    assert_eq!(resolved.model, "deepseek-v4-pro");
}

#[test]
fn harness_posture_kind_rejects_unknown_values() {
    let err = toml::from_str::<ConfigToml>(
        r#"
[[harness_profiles]]
provider_route = "deepseek"
model_pattern = "deepseek-v4.*"

[harness_profiles.posture]
kind = "cahce-heavy"
"#,
    )
    .expect_err("misspelled kind should not deserialize as custom");

    assert!(err.to_string().contains("cahce-heavy"));
}

#[test]
fn harness_posture_rejects_unknown_policy_keys() {
    let err = toml::from_str::<ConfigToml>(
        r#"
[[harness_profiles]]
provider_route = "deepseek"
model_pattern = "deepseek-v4.*"

[harness_profiles.posture]
kind = "custom"
unknown_policy = "surprise"
"#,
    )
    .expect_err("unknown posture keys should not be ignored");

    assert!(err.to_string().contains("unknown_policy"));
}

#[test]
fn test_verbosity_resolution() {
    let _lock = env_lock();
    // Test TOML parsing
    let toml_str = r#"
        verbosity = "concise"
    "#;
    let config: ConfigToml = toml::from_str(toml_str).unwrap();
    assert_eq!(config.verbosity, Some("concise".to_string()));

    // Test Env overrides
    let _env = EnvGuard::without_deepseek_runtime_overrides();
    unsafe {
        std::env::set_var("CODEWHALE_VERBOSITY", "normal");
    }
    let env_overrides = EnvRuntimeOverrides::load();
    assert_eq!(env_overrides.verbosity, Some("normal".to_string()));
    unsafe {
        std::env::remove_var("CODEWHALE_VERBOSITY");
    }

    // Test fallback to DEEPSEEK_VERBOSITY
    unsafe {
        std::env::set_var("DEEPSEEK_VERBOSITY", "concise");
    }
    let env_overrides = EnvRuntimeOverrides::load();
    assert_eq!(env_overrides.verbosity, Some("concise".to_string()));
    unsafe {
        std::env::remove_var("DEEPSEEK_VERBOSITY");
    }
}
