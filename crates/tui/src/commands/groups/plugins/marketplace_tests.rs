//! Command-surface tests for `/plugin marketplace` (#5311): add/list/show/
//! remove over real catalog schemas, install routed through the existing
//! reviewed installer, and the no-auto-anything invariants.

use super::*;
use crate::config::Config;
use crate::localization::Locale;
use crate::tui::app::{App, TuiOptions};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn create_test_app(root: &Path) -> (App, TempDir) {
    let temp = TempDir::new().expect("tempdir");
    let tools_dir = root.join("tools");
    fs::create_dir_all(&tools_dir).unwrap();
    let config_path = temp.path().join("config.toml");
    fs::write(&config_path, "[tools]\n").unwrap();
    let options = TuiOptions {
        config_path: Some(config_path),
        skills_dir: temp.path().join("skills"),
        memory_path: temp.path().join("memory.md"),
        notes_path: temp.path().join("notes.txt"),
        mcp_config_path: temp.path().join("mcp.json"),
        ..crate::test_support::test_tui_options(root)
    };
    let config = Config::default();
    let discovery = crate::plugins::PluginDiscoveryContext::capture_pre_dotenv();
    let registry = discovery.registry_for_workspace(root);
    let mut app = App::new_with_plugin_registry(options, &config, registry);
    app.ui_locale = Locale::En;
    (app, temp)
}

/// A real-schema Kimi catalog: two entries, one local (installable via the
/// reviewed installer once a bundle exists) and one zip URL (honestly
/// unsupported until fetch support exists).
fn write_kimi_catalog(dir: &Path) -> std::path::PathBuf {
    let catalog = serde_json::json!({
        "version": "2",
        "plugins": [
            {
                "id": "demo-bundle",
                "source": "./demo-bundle",
                "tier": "official",
                "displayName": "Demo Bundle",
                "version": "1.2.3",
                "description": "Local demo bundle",
                "homepage": "https://example.invalid/demo",
                "keywords": ["demo"]
            },
            {
                "id": "remote-thing",
                "source": "https://example.invalid/remote-thing.zip",
                "tier": "curated",
                "displayName": "Remote Thing"
            }
        ]
    });
    let path = dir.join("kimi-marketplace.json");
    fs::write(&path, serde_json::to_string_pretty(&catalog).unwrap()).unwrap();
    path
}

fn write_demo_bundle(dir: &Path) {
    let bundle = dir.join("demo-bundle");
    fs::create_dir_all(bundle.join("skills/hello")).unwrap();
    fs::write(
        bundle.join("plugin.toml"),
        "schema_version = 1\n[plugin]\nname = \"demo-bundle\"\nversion = \"1.0.0\"\ndescription = \"Demo\"\n[skills]\npath = \"skills\"\n",
    )
    .unwrap();
    fs::write(
        bundle.join("skills/hello/SKILL.md"),
        "---\nname: hello\ndescription: hello\n---\nbody\n",
    )
    .unwrap();
}

fn marketplace_state_path(codewhale_home: &Path) -> std::path::PathBuf {
    codewhale_home.join("plugins/marketplaces.json")
}

#[test]
fn marketplace_add_list_show_remove_roundtrip() {
    let _lock = crate::test_support::lock_test_env();
    let root = TempDir::new().unwrap();
    let codewhale_home = root.path().join("home");
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
    let (mut app, _temp) = create_test_app(root.path());
    let catalogs = root.path().join("catalogs");
    fs::create_dir_all(&catalogs).unwrap();
    let catalog_path = write_kimi_catalog(&catalogs);

    // Usage errors are honest before anything is touched.
    assert!(!plugins(&mut app, Some("marketplace")).is_error); // list, empty
    assert!(plugins(&mut app, Some("marketplace add")).is_error);
    assert!(plugins(&mut app, Some("marketplace add 'bad name' x")).is_error);

    let added = plugins(
        &mut app,
        Some(&format!("marketplace add kimi {}", catalog_path.display())),
    );
    assert!(!added.is_error, "{:?}", added.message);
    let message = added.message.unwrap();
    assert!(message.contains("Added marketplace `kimi`"), "{message}");
    assert!(message.contains("2 candidate(s)"), "{message}");
    assert!(message.contains("display-only"), "{message}");
    assert!(marketplace_state_path(&codewhale_home).exists());

    let list = plugins(&mut app, Some("marketplace list")).message.unwrap();
    assert!(list.contains("`kimi`"), "{list}");
    assert!(list.contains(r"demo\-bundle"), "{list}");
    assert!(list.contains(r"remote\-thing"), "{list}");
    assert!(list.contains("tier=official"), "{list}");
    assert!(list.contains("tier=curated"), "{list}");

    // Stored plans keep stable codes; rendering resolves the current locale.
    app.ui_locale = Locale::Es419;
    let localized = plugins(&mut app, Some("marketplace list")).message.unwrap();
    assert!(localized.contains("no admite paquetes ZIP"), "{localized}");
    assert!(!localized.contains("kimi_zip_unsupported"), "{localized}");
    assert!(
        !localized.contains("ZIP bundles are not supported"),
        "{localized}"
    );
    app.ui_locale = Locale::En;

    let show = plugins(&mut app, Some("marketplace show kimi"))
        .message
        .unwrap();
    assert!(show.contains("Demo Bundle"), "{show}");
    assert!(show.contains(r"v1\.2\.3"), "{show}");
    assert!(show.contains("catalogs"), "{show}");

    // read-only verbs never rewrite the store
    let before = fs::read_to_string(marketplace_state_path(&codewhale_home)).unwrap();
    plugins(&mut app, Some("marketplace list"));
    plugins(&mut app, Some("marketplace show kimi"));
    let after = fs::read_to_string(marketplace_state_path(&codewhale_home)).unwrap();
    assert_eq!(
        before, after,
        "list/show must not rewrite marketplace state"
    );

    // duplicate name is refused
    let dup = plugins(
        &mut app,
        Some(&format!("marketplace add kimi {}", catalog_path.display())),
    );
    assert!(dup.is_error);

    let removed = plugins(&mut app, Some("marketplace remove kimi"));
    assert!(!removed.is_error, "{:?}", removed.message);
    assert!(
        removed
            .message
            .unwrap()
            .contains("Installed plugins and their trust state are unaffected")
    );
    assert!(plugins(&mut app, Some("marketplace show kimi")).is_error);
    let empty = plugins(&mut app, Some("marketplace list")).message.unwrap();
    assert!(empty.contains("No marketplace catalogs"), "{empty}");
}

#[test]
fn marketplace_add_rejects_symlinks_and_bad_documents() {
    let _lock = crate::test_support::lock_test_env();
    let root = TempDir::new().unwrap();
    let codewhale_home = root.path().join("home");
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
    let (mut app, _temp) = create_test_app(root.path());
    let catalogs = root.path().join("catalogs");
    fs::create_dir_all(&catalogs).unwrap();
    let catalog_path = write_kimi_catalog(&catalogs);

    // missing file
    let missing = plugins(&mut app, Some("marketplace add nope /nonexistent/x.json"));
    assert!(missing.is_error);

    // symlink to a real catalog is refused, not followed
    let link = catalogs.join("link.json");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&catalog_path, &link).unwrap();
    #[cfg(not(unix))]
    fs::copy(&catalog_path, &link).unwrap();
    #[cfg(unix)]
    {
        let symlinked = plugins(
            &mut app,
            Some(&format!("marketplace add evil {}", link.display())),
        );
        assert!(symlinked.is_error);
        assert!(symlinked.message.unwrap().contains("symlink"));
    }

    // a document with no documented markers fails honestly and is not stored
    let junk = catalogs.join("junk.json");
    fs::write(&junk, "{\"hello\": \"world\"}").unwrap();
    let bad = plugins(
        &mut app,
        Some(&format!("marketplace add junk {}", junk.display())),
    );
    assert!(bad.is_error);
    assert!(bad.message.unwrap().contains("could not be parsed"));
    assert!(
        plugins(&mut app, Some("marketplace list"))
            .message
            .unwrap()
            .contains("No marketplace catalogs")
    );

    // corrupt stored state fails closed and is never rewritten
    let store_path = marketplace_state_path(&codewhale_home);
    fs::create_dir_all(store_path.parent().unwrap()).unwrap();
    fs::write(&store_path, "{ not json").unwrap();
    let corrupt = plugins(&mut app, Some("marketplace list"));
    assert!(corrupt.is_error);
    assert!(corrupt.message.unwrap().contains("fail-closed"));
    assert_eq!(fs::read_to_string(&store_path).unwrap(), "{ not json");
}

#[test]
fn marketplace_install_routes_through_reviewed_installer() {
    let _lock = crate::test_support::lock_test_env();
    let root = TempDir::new().unwrap();
    let codewhale_home = root.path().join("home");
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
    let (mut app, _temp) = create_test_app(root.path());
    let catalogs = root.path().join("catalogs");
    fs::create_dir_all(&catalogs).unwrap();
    write_kimi_catalog(&catalogs);
    write_demo_bundle(&catalogs);
    assert!(
        !plugins(
            &mut app,
            Some("marketplace add kimi catalogs/kimi-marketplace.json")
        )
        .is_error
    );

    // The unsupported plan is refused before any runtime or network work.
    let remote = plugins(&mut app, Some("marketplace install kimi remote-thing"));
    assert!(remote.is_error);
    assert!(remote.message.unwrap().contains("cannot be installed"));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let installed = plugins(&mut app, Some("marketplace install kimi demo-bundle"));
        assert!(!installed.is_error, "{:?}", installed.message);
        let message = installed.message.unwrap();
        assert!(message.contains("disabled and untrusted"), "{message}");
        assert!(
            message
                .lines()
                .any(|line| line.starts_with("/plugin trust demo-bundle ")),
            "marketplace install must route into the trust review:\n{message}"
        );

        // No auto-trust, no auto-enable, no inherited vendor trust.
        let plugin = app.plugin_registry.get("demo-bundle").unwrap();
        assert!(!plugin.enabled);
        assert!(!plugin.trusted());
        assert!(
            codewhale_home
                .join("plugins/demo-bundle/.installed-from")
                .exists()
        );
    });
}

/// A Codex catalog whose entry declares `INSTALLED_BY_DEFAULT`: the policy is
/// visible but nothing is auto-installed, and the npm source stays honestly
/// unsupported.
#[test]
fn marketplace_codex_installed_by_default_never_auto_installs() {
    let _lock = crate::test_support::lock_test_env();
    let root = TempDir::new().unwrap();
    let codewhale_home = root.path().join("home");
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
    let (mut app, _temp) = create_test_app(root.path());
    let catalogs = root.path().join("catalogs");
    fs::create_dir_all(&catalogs).unwrap();
    let codex = serde_json::json!({
        "name": "codex-catalog",
        "plugins": [
            {
                "name": "defaulted-thing",
                "source": { "source": "npm", "package": "@scope/defaulted-thing" },
                "policy": { "installation": "INSTALLED_BY_DEFAULT" }
            }
        ]
    });
    let path = catalogs.join("codex-marketplace.json");
    fs::write(&path, serde_json::to_string_pretty(&codex).unwrap()).unwrap();

    let added = plugins(
        &mut app,
        Some(&format!("marketplace add codex {}", path.display())),
    );
    assert!(!added.is_error, "{:?}", added.message);

    let list = plugins(&mut app, Some("marketplace list")).message.unwrap();
    assert!(list.contains(r"defaulted\-thing"), "{list}");
    assert!(list.contains("not installable"), "{list}");
    assert!(list.contains("npm"), "{list}");
    // Foreign auto-install policy never ran anything: no bundle on disk.
    assert!(!codewhale_home.join("plugins/defaulted-thing").exists());
    assert!(app.plugin_registry.get("defaulted-thing").is_none());
}
