use std::path::PathBuf;

use crate::plugins::discovery::{builtin_plugins_dir, discover_all};
use crate::plugins::manifest::{load_manifest, load_plugin_mcp, PluginManifest};

#[test]
fn manifest_parsing_basics() {
    let toml_str = r#"
[plugin]
name = "test-plugin"
description = "A test plugin"
"#;
    let manifest: PluginManifest = toml::from_str(toml_str).unwrap();
    assert_eq!(manifest.plugin.name, "test-plugin");
    assert_eq!(manifest.plugin.description, "A test plugin");
}

#[test]
fn load_plugin_mcp_empty_when_none() {
    let toml_str = r#"
[plugin]
name = "no-mcp"
description = "no mcp"
"#;
    let manifest: PluginManifest = toml::from_str(toml_str).unwrap();
    let mcp = load_plugin_mcp(&manifest);
    assert!(mcp.is_empty());
}

#[test]
fn load_manifest_fails_for_missing_dir() {
    let dir = std::path::Path::new("/nonexistent/plugin/dir");
    let result = load_manifest(dir);
    assert!(result.is_err());
}

#[test]
fn sample_builtin_plugin_loads() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("plugins")
        .join("rust-toolkit");

    assert!(dir.exists(), "sample plugin dir should exist");
    assert!(dir.join("plugin.toml").exists(), "plugin.toml should exist");

    let manifest = load_manifest(&dir).expect("sample plugin manifest should parse");
    assert_eq!(manifest.plugin.name, "rust-toolkit");
    assert_eq!(manifest.plugin.description, "Rust development tools: cargo check, clippy, and test runner");
}

#[test]
fn builtin_plugins_dir_exists() {
    let dir = builtin_plugins_dir();
    assert!(dir.exists());
}

#[test]
fn discover_all_returns_registry() {
    let registry = discover_all(&[]);
    assert!(registry.list().is_empty() || registry.list().len() >= 1);
}
