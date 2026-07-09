use std::collections::HashMap;
use std::path::PathBuf;

use super::discovery::{load_overrides, save_overrides};
use super::manifest::{LoadedPlugin, PluginManifest, PluginMeta};
use super::registry::PluginRegistry;

fn plugin_named(name: &str, enabled: bool) -> LoadedPlugin {
    LoadedPlugin {
        manifest: PluginManifest {
            plugin: PluginMeta {
                name: name.to_string(),
                description: None,
                version: None,
                author: None,
            },
            skills: None,
            mcp_servers: None,
            when: None,
        },
        base_path: PathBuf::new(),
        enabled,
    }
}

#[test]
fn test_manifest_parsing() {
    let toml_content = r#"
[plugin]
name = "test-plugin"
description = "A test plugin"
version = "1.0.0"
author = "Test Author"

[when]
os = ["windows", "linux"]
binaries = ["cargo"]
"#;

    let manifest: PluginManifest = toml::from_str(toml_content).unwrap();
    assert_eq!(manifest.plugin.name, "test-plugin");
    assert_eq!(manifest.plugin.description.unwrap(), "A test plugin");
    assert_eq!(manifest.plugin.version.unwrap(), "1.0.0");
    assert_eq!(manifest.plugin.author.unwrap(), "Test Author");
}

#[test]
fn test_manifest_when_os_filter() {
    let manifest = PluginManifest {
        plugin: PluginMeta {
            name: "test".to_string(),
            description: None,
            version: None,
            author: None,
        },
        skills: None,
        mcp_servers: None,
        when: Some(super::manifest::PluginWhen {
            os: Some(vec![std::env::consts::OS.to_string()]),
            binaries: None,
        }),
    };

    assert!(manifest.check_when());
}

#[test]
fn test_manifest_when_os_mismatch() {
    let manifest = PluginManifest {
        plugin: PluginMeta {
            name: "test".to_string(),
            description: None,
            version: None,
            author: None,
        },
        skills: None,
        mcp_servers: None,
        when: Some(super::manifest::PluginWhen {
            os: Some(vec!["nonexistent-os".to_string()]),
            binaries: None,
        }),
    };

    assert!(!manifest.check_when());
}

#[test]
fn test_registry_enable_disable() {
    let mut registry = PluginRegistry::new();

    let manifest = PluginManifest {
        plugin: PluginMeta {
            name: "test-plugin".to_string(),
            description: None,
            version: None,
            author: None,
        },
        skills: None,
        mcp_servers: None,
        when: None,
    };

    let plugin = super::manifest::LoadedPlugin {
        manifest,
        base_path: PathBuf::new(),
        enabled: false,
    };

    registry.register("test-plugin".to_string(), plugin);

    assert!(!registry.is_enabled("test-plugin"));
    assert!(registry.enable("test-plugin"));
    assert!(registry.is_enabled("test-plugin"));
    assert!(registry.disable("test-plugin"));
    assert!(!registry.is_enabled("test-plugin"));
}

#[test]
fn test_registry_list() {
    let mut registry = PluginRegistry::new();

    let manifest1 = PluginManifest {
        plugin: PluginMeta {
            name: "plugin-1".to_string(),
            description: None,
            version: None,
            author: None,
        },
        skills: None,
        mcp_servers: None,
        when: None,
    };

    let manifest2 = PluginManifest {
        plugin: PluginMeta {
            name: "plugin-2".to_string(),
            description: None,
            version: None,
            author: None,
        },
        skills: None,
        mcp_servers: None,
        when: None,
    };

    let plugin1 = super::manifest::LoadedPlugin {
        manifest: manifest1,
        base_path: PathBuf::new(),
        enabled: true,
    };

    let plugin2 = super::manifest::LoadedPlugin {
        manifest: manifest2,
        base_path: PathBuf::new(),
        enabled: false,
    };

    registry.register("plugin-1".to_string(), plugin1);
    registry.register("plugin-2".to_string(), plugin2);

    assert_eq!(registry.len(), 2);
    assert_eq!(registry.enabled_plugins().len(), 1);
    assert_eq!(registry.list_enabled().len(), 1);
    assert!(registry.is_enabled("plugin-1"));
    assert!(!registry.is_enabled("plugin-2"));
}

#[test]
fn overrides_save_load_roundtrip() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("plugins").join("overrides.json");

    let mut overrides = HashMap::new();
    overrides.insert("alpha".to_string(), false);
    overrides.insert("beta".to_string(), true);

    save_overrides(&path, &overrides).expect("save");
    assert!(path.exists(), "save_overrides should create parent dirs");

    let loaded = load_overrides(&path);
    assert_eq!(loaded.get("alpha"), Some(&false));
    assert_eq!(loaded.get("beta"), Some(&true));
}

#[test]
fn load_overrides_missing_or_malformed_is_empty() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let missing = tmp.path().join("nope.json");
    assert!(load_overrides(&missing).is_empty());

    let malformed = tmp.path().join("bad.json");
    std::fs::write(&malformed, "{ not json").expect("write");
    assert!(load_overrides(&malformed).is_empty());
}

/// The core #3918 regression: a `/plugin disable` must survive the next
/// launch even though discovery recomputes `enabled` from `!builtin`.
#[test]
fn disable_persists_across_rediscovery() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("overrides.json");

    // First session: a user plugin defaults to enabled, user disables it.
    let mut first = PluginRegistry::new();
    first.set_overrides_store(path.clone(), load_overrides(&path));
    first.register("demo".to_string(), plugin_named("demo", true));
    first.apply_overrides();
    assert!(first.is_enabled("demo"));
    assert!(first.disable("demo"));
    assert!(path.exists(), "disable should persist the override file");

    // Second session: fresh discovery re-registers it enabled, but the
    // persisted override must win and keep it disabled.
    let mut second = PluginRegistry::new();
    second.set_overrides_store(path.clone(), load_overrides(&path));
    second.register("demo".to_string(), plugin_named("demo", true));
    second.apply_overrides();
    assert!(
        !second.is_enabled("demo"),
        "a persisted disable must survive re-discovery"
    );
}

/// Symmetric case: enabling a built-in (default-disabled) plugin sticks.
#[test]
fn enable_persists_across_rediscovery() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("overrides.json");

    let mut first = PluginRegistry::new();
    first.set_overrides_store(path.clone(), load_overrides(&path));
    first.register("builtin".to_string(), plugin_named("builtin", false));
    first.apply_overrides();
    assert!(!first.is_enabled("builtin"));
    assert!(first.enable("builtin"));

    let mut second = PluginRegistry::new();
    second.set_overrides_store(path.clone(), load_overrides(&path));
    second.register("builtin".to_string(), plugin_named("builtin", false));
    second.apply_overrides();
    assert!(
        second.is_enabled("builtin"),
        "a persisted enable must survive re-discovery"
    );
}
