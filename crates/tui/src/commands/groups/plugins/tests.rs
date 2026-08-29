use super::*;
// `fs` was a cfg(test) import on the parent, and `Path` is now only used by
// the legacy/render seams. Both belong here.
use crate::config::Config;
use crate::localization::Locale;
use crate::tui::app::{App, TuiOptions};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn create_test_app(root: &Path) -> (App, TempDir) {
    let temp = TempDir::new().expect("tempdir");
    let config_path = temp.path().join("config.toml");
    let tools_dir = root.join("tools");
    fs::create_dir_all(&tools_dir).unwrap();
    fs::write(
        &config_path,
        format!(
            "[tools]\nplugin_dir = {}\n",
            toml::Value::String(tools_dir.to_string_lossy().to_string())
        ),
    )
    .unwrap();
    let options = TuiOptions {
        config_path: Some(config_path),
        skills_dir: temp.path().join("skills"),
        memory_path: temp.path().join("memory.md"),
        notes_path: temp.path().join("notes.txt"),
        mcp_config_path: temp.path().join("mcp.json"),
        ..crate::test_support::test_tui_options(root)
    };
    let config = Config {
        tools: Some(crate::config::ToolsConfig {
            plugin_dir: Some(tools_dir.to_string_lossy().into_owned()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let discovery = crate::plugins::PluginDiscoveryContext::capture_pre_dotenv();
    let registry = discovery.registry_for_workspace(root);
    let mut app = App::new_with_plugin_registry(options, &config, registry);
    app.ui_locale = Locale::En;
    (app, temp)
}

fn write_bundle(root: &Path) {
    let bundle = root.join(".codewhale/plugins/demo");
    fs::create_dir_all(bundle.join("skills/hello")).unwrap();
    fs::write(
        bundle.join("plugin.toml"),
        "schema_version = 1\n[plugin]\nname = \"demo\"\nversion = \"1.0.0\"\ndescription = \"Import spreadsheet data safely\"\n[skills]\npath = \"skills\"\n",
    )
    .unwrap();
    fs::write(
        bundle.join("skills/hello/SKILL.md"),
        "---\nname: hello\ndescription: hello\n---\nbody\n",
    )
    .unwrap();
}

fn write_mcp_review_bundle(root: &Path) {
    let bundle = root.join(".codewhale/plugins/review-mcp");
    fs::create_dir_all(&bundle).unwrap();
    fs::write(bundle.join("server.js"), "// reviewed entrypoint\n").unwrap();
    fs::write(
        bundle.join("plugin.toml"),
        r#"schema_version = 1
[plugin]
name = "review-mcp"
version = "1.0.0"

[mcp_servers.local]
command = "node"
args = ["server.js", "--mode=worker", "-e", "console.log('ready')"]

[mcp_servers.local.env]
PLUGIN_TOKEN = "${PLUGIN_TOKEN_SOURCE}"

[mcp_servers.remote]
url = "https://example.invalid/mcp"
bearer_token_env_var = "REMOTE_TOKEN"

[mcp_servers.remote.env_headers]
X_Api_Key = "REMOTE_API_KEY"

[capabilities]
network_hosts = ["example.invalid"]
"#,
    )
    .unwrap();
}

#[test]
fn bare_plugin_command_opens_unified_extensions_modal() {
    let _lock = crate::test_support::lock_test_env();
    let root = TempDir::new().unwrap();
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", root.path().join("home"));
    let (mut app, _temp) = create_test_app(root.path());

    let result = plugins(&mut app, None);

    assert!(matches!(
        result.action,
        Some(AppAction::OpenExtensions {
            tab: crate::tui::views::extensions::ExtensionsTab::Plugins
        })
    ));
    assert!(result.message.is_none());
}

#[test]
fn list_show_validate_are_read_only_and_label_legacy_tools() {
    let _lock = crate::test_support::lock_test_env();
    let root = TempDir::new().unwrap();
    let codewhale_home = root.path().join("home");
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
    write_bundle(root.path());
    let (mut app, _temp) = create_test_app(root.path());
    fs::write(
        root.path().join("tools/greet.sh"),
        "# name: greet\n# description: hello\n",
    )
    .unwrap();
    // The app already resolved the legacy tools path during startup.
    // Read-only plugin commands must not reopen a credential-bearing
    // config file merely to inventory those tools.
    fs::write(
        app.config_path.as_ref().unwrap(),
        "api_key = [\"must-not-be-re-read\"\n",
    )
    .unwrap();
    let state_path = codewhale_home.join("plugins/state.json");

    for arg in [Some("list"), Some("show demo"), Some("validate")] {
        let result = plugins(&mut app, arg);
        assert!(!result.is_error, "{:?}", result.message);
        assert!(!state_path.exists(), "read-only command wrote plugin state");
    }
    let list = plugins(&mut app, Some("list")).message.unwrap();
    assert!(list.contains("Plugin bundles (1)"));
    assert!(list.contains("disabled"));
    assert!(list.contains("Legacy executable plugin tools (1)"));
}

#[test]
fn suggest_ranks_installed_plugins_without_trusting_or_enabling_them() {
    let _lock = crate::test_support::lock_test_env();
    let root = TempDir::new().unwrap();
    let codewhale_home = root.path().join("home");
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
    write_bundle(root.path());
    let (mut app, _temp) = create_test_app(root.path());

    for arg in ["suggest", "suggest go"] {
        let result = plugins(&mut app, Some(arg));
        assert!(
            result.is_error,
            "expected usage error for {arg}: {result:?}"
        );
    }

    let result = plugins(&mut app, Some("suggest spreadsheet import"));
    assert!(!result.is_error, "{result:?}");
    let message = result.message.expect("suggestion message");
    assert!(message.contains("Suggested plugins"), "{message}");
    assert!(message.contains("demo — disabled"), "{message}");
    assert!(message.contains("Why:"), "{message}");
    assert!(message.contains("/plugin trust demo"), "{message}");
    assert!(
        message.contains("spreadsheet") || message.contains("import"),
        "{message}"
    );
    assert!(message.contains("Nothing was installed, trusted, or enabled."));
    assert!(!codewhale_home.join("plugins/state.json").exists());
    let plugin = app.plugin_registry.get("demo").expect("demo plugin");
    assert!(!plugin.enabled && !plugin.trusted());
}

#[test]
fn suggest_matches_manifest_keywords_for_a_named_integration() {
    let _lock = crate::test_support::lock_test_env();
    let root = TempDir::new().unwrap();
    let codewhale_home = root.path().join("home");
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
    let bundle = root.path().join(".codewhale/plugins/supabase");
    fs::create_dir_all(&bundle).unwrap();
    fs::write(
        bundle.join("plugin.toml"),
        "schema_version = 1\n[plugin]\nname = \"supabase\"\nversion = \"1.0.0\"\ndescription = \"Hosted Postgres and auth\"\nkeywords = [\"supabase\", \"postgres\"]\n",
    )
    .unwrap();
    let (mut app, _temp) = create_test_app(root.path());

    let result = plugins(&mut app, Some("suggest add supabase auth"));
    assert!(!result.is_error, "{result:?}");
    let message = result.message.expect("suggestion message");
    assert!(message.contains("supabase"), "{message}");
    assert!(message.contains("/plugin trust supabase"), "{message}");
    assert!(message.contains("Nothing was installed, trusted, or enabled."));
}

#[test]
fn trust_requires_content_and_capability_bound_review_token() {
    let _lock = crate::test_support::lock_test_env();
    let root = TempDir::new().unwrap();
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", root.path().join("home"));
    write_bundle(root.path());
    let (mut app, _temp) = create_test_app(root.path());
    let enable_review = plugins(&mut app, Some("enable demo"));
    assert!(!enable_review.is_error);
    assert!(
        enable_review
            .message
            .as_deref()
            .is_some_and(|message| message.contains("/plugin trust demo "))
    );
    assert!(!app.plugin_registry.get("demo").unwrap().trusted());

    let review = plugins(&mut app, Some("trust demo")).message.unwrap();
    let confirmation = review
        .lines()
        .find(|line| line.starts_with("/plugin trust demo "))
        .unwrap();
    let token = confirmation
        .split_whitespace()
        .last()
        .expect("review confirmation token");
    let (content_digest, capability_digest) = token
        .split_once('.')
        .expect("content and capability digests");
    assert_eq!(content_digest.len(), 64);
    assert_eq!(capability_digest.len(), 64);
    assert!(content_digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert!(
        capability_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    );
    assert!(!app.plugin_registry.get("demo").unwrap().trusted());

    assert!(plugins(&mut app, Some("trust demo wrong")).is_error);
    let shortened = format!(
        "trust demo {}.{}",
        &content_digest[..12],
        &capability_digest[..12]
    );
    assert!(
        plugins(&mut app, Some(&shortened)).is_error,
        "the legacy 48-bit content prefix must not authorize trust"
    );
    let arg = confirmation.trim_start_matches("/plugin ");
    assert!(!plugins(&mut app, Some(arg)).is_error);
    assert!(!plugins(&mut app, Some("enable demo")).is_error);
    assert!(app.plugin_registry.is_active("demo"));
    assert!(!plugins(&mut app, Some("disable demo")).is_error);
    assert!(!app.plugin_registry.is_active("demo"));
}

fn write_mixed_bundle(root: &Path) {
    let bundle = root.join(".codewhale/plugins/mixed");
    fs::create_dir_all(bundle.join("skills/hello")).unwrap();
    fs::create_dir_all(bundle.join("commands")).unwrap();
    fs::create_dir_all(bundle.join("hooks")).unwrap();
    fs::create_dir_all(bundle.join("lsp")).unwrap();
    fs::write(
        bundle.join("plugin.toml"),
        "schema_version = 1\n[plugin]\nname = \"mixed\"\nversion = \"1.0.0\"\n[skills]\npath = \"skills\"\n[commands]\npath = \"commands\"\n[hooks]\npath = \"hooks\"\n[lsp]\npath = \"lsp\"\n",
    )
    .unwrap();
    fs::write(
        bundle.join("skills/hello/SKILL.md"),
        "---\nname: hello\ndescription: hello\n---\nbody\n",
    )
    .unwrap();
}

#[test]
fn mixed_bundle_review_and_enable_keep_supported_components_active() {
    let _lock = crate::test_support::lock_test_env();
    let root = TempDir::new().unwrap();
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", root.path().join("home"));
    write_mixed_bundle(root.path());
    let (mut app, _temp) = create_test_app(root.path());

    let list = plugins(&mut app, Some("list")).message.unwrap();
    assert!(list.contains("compatibility=partial"), "{list}");
    assert!(list.contains("commands=1"), "{list}");
    assert!(list.contains("hooks=1"), "{list}");

    let show = plugins(&mut app, Some("show mixed")).message.unwrap();
    assert!(show.contains("Compatibility: partial"), "{show}");
    assert!(show.contains("Inactive components: [lsp]"), "{show}");
    assert!(show.contains("Active components: [none]"), "{show}");

    let review = plugins(&mut app, Some("trust mixed")).message.unwrap();
    let confirmation = review
        .lines()
        .find(|line| line.starts_with("/plugin trust mixed "))
        .unwrap();
    let arg = confirmation.trim_start_matches("/plugin ");
    assert!(!plugins(&mut app, Some(arg)).is_error);
    let enabled = plugins(&mut app, Some("enable mixed"));
    assert!(!enabled.is_error, "{:?}", enabled.message);
    let message = enabled.message.unwrap();
    assert!(message.contains("Compatibility: partial"), "{message}");
    assert!(message.contains("inactive: lsp"), "{message}");
    assert!(app.plugin_registry.is_active("mixed"));
    assert_eq!(
        app.plugin_registry
            .get("mixed")
            .unwrap()
            .compatibility()
            .as_str(),
        "partial"
    );

    let show = plugins(&mut app, Some("show mixed")).message.unwrap();
    assert!(show.contains("State: active"), "{show}");
    assert!(show.contains("Inactive components: [lsp]"), "{show}");
    assert!(
        show.contains("Active components: [skills, commands, hooks]"),
        "{show}"
    );
    assert!(show.contains("Qualified skills: [mixed:hello]"), "{show}");
}

#[test]
fn mcp_review_discloses_host_authority_and_names_without_secret_values() {
    let _lock = crate::test_support::lock_test_env();
    let root = TempDir::new().unwrap();
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", root.path().join("home"));
    write_mcp_review_bundle(root.path());
    let (mut app, _temp) = create_test_app(root.path());
    let review = plugins(&mut app, Some("trust review-mcp"))
        .message
        .expect("review output");
    assert!(review.contains("mcp=2 (stdio=1 remote=1)"));
    assert!(review.contains("host-user filesystem/network authority"));
    assert!(review.contains("PLUGIN\\_TOKEN <- PLUGIN\\_TOKEN\\_SOURCE"));
    assert!(review.contains("X\\_Api\\_Key <- REMOTE\\_API\\_KEY"));
    assert!(review.contains("bearer_env=REMOTE\\_TOKEN"));
    assert!(review.contains("redirects=same-origin-only"));
    assert!(review.contains("Qualified skills: [none]"));
    assert!(review.contains("#2 value=\"--mode=worker\""));
    assert!(review.contains("#3 value=\"-e\""));
    assert!(review.contains("#4 value=\"console.log('ready')\""));
    assert!(review.contains("oauth=disabled"));
}

#[test]
fn legacy_tool_detail_remains_available_under_tools_namespace() {
    let _lock = crate::test_support::lock_test_env();
    let root = TempDir::new().unwrap();
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", root.path().join("home"));
    let (mut app, _temp) = create_test_app(root.path());
    fs::write(
        root.path().join("tools/greet.sh"),
        "# name: greet\n# description: Say hello\n# approval: required\n",
    )
    .unwrap();
    let result = plugins(&mut app, Some("tools greet"));
    assert!(!result.is_error);
    let message = result.message.unwrap();
    assert!(message.contains("Say hello"));
    assert!(message.contains("required"));
}

#[test]
fn install_update_uninstall_verbs_validate_arguments() {
    let _lock = crate::test_support::lock_test_env();
    let root = TempDir::new().unwrap();
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", root.path().join("home"));
    let (mut app, _temp) = create_test_app(root.path());
    for arg in ["install", "update", "uninstall"] {
        let result = plugins(&mut app, Some(arg));
        assert!(result.is_error, "bare `{arg}` must print usage");
    }
    let invalid = plugins(&mut app, Some("install github:"));
    assert!(invalid.is_error);
    assert!(
        invalid
            .message
            .unwrap()
            .contains("Invalid plugin install source"),
        "invalid specs must be rejected before any network or disk access"
    );
}

#[test]
fn install_update_uninstall_verbs_drive_the_guided_trust_flow() {
    let _lock = crate::test_support::lock_test_env();
    let root = TempDir::new().unwrap();
    let codewhale_home = root.path().join("home");
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);

    let source = root.path().join("source/installed-demo");
    fs::create_dir_all(&source).unwrap();
    fs::write(
        source.join("plugin.toml"),
        "schema_version = 1\n[plugin]\nname = \"installed-demo\"\nversion = \"1.0.0\"\n",
    )
    .unwrap();

    let (mut app, _temp) = create_test_app(root.path());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let installed = plugins(&mut app, Some(&format!("install {}", source.display())));
        assert!(!installed.is_error, "{:?}", installed.message);
        let message = installed.message.unwrap();
        assert!(message.contains("disabled and untrusted"), "{message}");
        let confirmation = message
            .lines()
            .find(|line| line.starts_with("/plugin trust installed-demo "))
            .expect("install must route into the trust review")
            .to_string();
        let plugin = app.plugin_registry.get("installed-demo").unwrap();
        assert!(!plugin.enabled && !plugin.trusted());
        assert!(
            codewhale_home
                .join("plugins/installed-demo/.installed-from")
                .exists()
        );

        // Local-path installs cannot be updated from the network.
        let update = plugins(&mut app, Some("update installed-demo"));
        assert!(update.is_error);
        assert!(update.message.unwrap().contains("local path"));

        let arg = confirmation.trim_start_matches("/plugin ").to_string();
        assert!(!plugins(&mut app, Some(&arg)).is_error);
        assert!(!plugins(&mut app, Some("enable installed-demo")).is_error);
        assert!(app.plugin_registry.is_active("installed-demo"));

        // Uninstall requires disabled, then removes bits and prunes state.
        let refused = plugins(&mut app, Some("uninstall installed-demo"));
        assert!(refused.is_error);
        assert!(codewhale_home.join("plugins/installed-demo").exists());
        assert!(!plugins(&mut app, Some("disable installed-demo")).is_error);
        let removed = plugins(&mut app, Some("uninstall installed-demo"));
        assert!(!removed.is_error, "{:?}", removed.message);
        assert!(!codewhale_home.join("plugins/installed-demo").exists());
        assert!(app.plugin_registry.get("installed-demo").is_none());
        let raw = fs::read_to_string(codewhale_home.join("plugins/state.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(
            parsed["plugins"].as_object().unwrap().is_empty(),
            "uninstall must prune the state entry: {raw}"
        );
    });
}

#[test]
fn kimi_managed_import_is_read_only_until_hash_bound_approval() {
    let _lock = crate::test_support::lock_test_env();
    let root = TempDir::new().unwrap();
    let codewhale_home = root.path().join("codewhale-home");
    let _codewhale_home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
    let managed = root.path().join(".kimi-code/plugins/managed/kimi-demo");
    fs::create_dir_all(managed.join("skills/kimi-demo")).unwrap();
    fs::write(
        managed.join("kimi.plugin.json"),
        r#"{
          "name": "kimi-demo",
          "version": "1.0.0",
          "license": "Proprietary",
          "skills": "./skills/",
          "interface": {
            "displayName": "Kimi Demo",
            "hostKind": "local",
            "platforms": ["macos"]
          }
        }"#,
    )
    .unwrap();
    fs::write(
        managed.join("skills/kimi-demo/SKILL.md"),
        "---\nname: kimi-demo\ndescription: local managed fixture\n---\n",
    )
    .unwrap();

    let (mut app, _temp) = create_test_app(root.path());
    let help = plugins(&mut app, Some("help")).message.unwrap();
    assert!(help.contains("/plugin import kimi [list]"), "{help}");
    let listed = plugins_with_kimi_home(&mut app, Some("import kimi"), root.path());
    assert!(!listed.is_error, "{:?}", listed.message);
    let message = listed.message.unwrap();
    assert!(
        message.contains("Kimi Demo") || message.contains("kimi-demo"),
        "{message}"
    );
    assert!(message.contains("license=Proprietary"), "{message}");
    assert!(message.contains("content hash:"), "{message}");
    assert!(message.contains("External Kimi apps"), "{message}");
    let approval = message
        .lines()
        .find_map(|line| line.trim().strip_prefix("approve: /plugin "))
        .expect("listing must render an exact approval command")
        .to_string();
    assert!(app.plugin_registry.get("kimi-demo").is_none());
    assert!(!codewhale_home.join("plugins/kimi-demo").exists());
    assert!(!codewhale_home.join("plugins/state.json").exists());

    // The approval token is tied to the bytes that were inspected.
    fs::write(
        managed.join("skills/kimi-demo/SKILL.md"),
        "---\nname: kimi-demo\ndescription: changed fixture\n---\n",
    )
    .unwrap();
    let changed = plugins_with_kimi_home(&mut app, Some(&approval), root.path());
    assert!(changed.is_error);
    assert!(changed.message.unwrap().contains("changed since review"));
    assert!(!codewhale_home.join("plugins/kimi-demo").exists());

    let refreshed = plugins_with_kimi_home(&mut app, Some("import kimi"), root.path())
        .message
        .unwrap();
    let approval = refreshed
        .lines()
        .find_map(|line| line.trim().strip_prefix("approve: /plugin "))
        .unwrap()
        .to_string();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let installed = plugins_with_kimi_home(&mut app, Some(&approval), root.path());
        assert!(!installed.is_error, "{:?}", installed.message);
        assert!(
            installed
                .message
                .as_deref()
                .is_some_and(|message| message.contains("disabled and untrusted"))
        );
    });
    let plugin = app.plugin_registry.get("kimi-demo").unwrap();
    assert!(!plugin.enabled && !plugin.trusted());
    assert!(
        codewhale_home
            .join("plugins/kimi-demo/kimi.plugin.json")
            .is_file()
    );
}

#[test]
fn kimi_managed_import_renders_in_the_selected_non_english_locale() {
    let root = TempDir::new().unwrap();
    let (mut app, _temp) = create_test_app(root.path());
    app.ui_locale = Locale::Es419;

    let message = plugins_with_kimi_home(&mut app, Some("import kimi"), root.path())
        .message
        .expect("localized Kimi listing");
    assert!(
        message.contains("Plugins gestionados por Kimi"),
        "{message}"
    );
    assert!(
        message.contains("No se encontraron plugins gestionados válidos"),
        "{message}"
    );
    assert!(
        !message.contains("No valid managed plugins found"),
        "{message}"
    );
}

#[cfg(unix)]
#[test]
fn kimi_managed_import_refuses_linked_children() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().unwrap();
    let managed_root = root.path().join(".kimi-code/plugins/managed");
    let outside = root.path().join("outside");
    fs::create_dir_all(&managed_root).unwrap();
    fs::create_dir_all(&outside).unwrap();
    symlink(&outside, managed_root.join("linked-plugin")).unwrap();
    let (mut app, _temp) = create_test_app(root.path());

    let result = plugins_with_kimi_home(&mut app, Some("import kimi"), root.path());
    assert!(!result.is_error);
    let message = result.message.unwrap();
    assert!(message.contains("Rejected entries"), "{message}");
    assert!(message.contains("links and reparse points are refused"));
    assert!(!message.contains("approve: /plugin import kimi approve linked-plugin"));
}

#[test]
fn export_verb_writes_agent_plugins_bundle() {
    let _lock = crate::test_support::lock_test_env();
    let root = TempDir::new().unwrap();
    let codewhale_home = root.path().join("home");
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
    write_bundle(root.path());
    let (mut app, _temp) = create_test_app(root.path());

    let usage = plugins(&mut app, Some("export"));
    assert!(usage.is_error, "export without arguments is a usage error");
    let missing = plugins(&mut app, Some("export nope out"));
    assert!(missing.is_error, "exporting an unknown plugin fails");

    let result = plugins(&mut app, Some("export demo exported/demo"));
    assert!(!result.is_error, "{result:?}");
    let message = result.message.expect("export message");
    assert!(message.contains("Exported `demo`"), "{message}");
    assert!(message.contains("plugin.json"), "{message}");

    let bundle = root.path().join("exported/demo");
    let plugin_json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(bundle.join("plugin.json")).unwrap()).unwrap();
    crate::plugins::agent_plugin::validate_plugin_json(&plugin_json).unwrap();
    assert_eq!(plugin_json["name"], "demo");
    assert_eq!(plugin_json["description"], "Import spreadsheet data safely");
    assert!(bundle.join("skills/hello/SKILL.md").is_file());
    assert!(!bundle.join("plugin.toml").exists());
    // No MCP servers declared, so no mcp.json is written.
    assert!(!bundle.join("mcp.json").exists());
    // The installed bundle keeps its legacy manifest and stays untouched.
    assert!(
        root.path()
            .join(".codewhale/plugins/demo/plugin.toml")
            .exists()
    );
}
