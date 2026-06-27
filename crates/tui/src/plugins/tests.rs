//! Integration tests for the plugin system.
//!
//! Tests the end-to-end plugin lifecycle: manifest parsing, skill loading,
//! registry operations, and system prompt injection.

use std::path::PathBuf;

use crate::plugins::discovery::discover_all;
use crate::plugins::injection::render_plugin_block;
use crate::plugins::manifest::{load_manifest, load_plugin_mcp, load_plugin_skills};

/// Verify the sample built-in plugin `rust-toolkit` loads correctly.
#[test]
fn sample_builtin_plugin_loads() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("plugins")
        .join("rust-toolkit");

    assert!(dir.exists(), "sample plugin dir should exist");
    assert!(dir.join("plugin.toml").exists(), "plugin.toml should exist");
    assert!(
        dir.join("skills").join("rust-check").join("SKILL.md").exists(),
        "rust-check SKILL.md should exist"
    );

    let manifest = load_manifest(&dir).expect("sample plugin manifest should parse");
    assert_eq!(manifest.plugin.name, "rust-toolkit");
    assert_eq!(manifest.plugin.version.as_deref(), Some("1.0.0"));
    assert!(manifest.plugin.default_enabled);

    let mcp = load_plugin_mcp(&manifest);
    assert!(mcp.is_empty(), "sample plugin has no MCP servers");
}

/// Verify skill loading from the sample plugin.
#[test]
fn sample_plugin_skills_load() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("plugins")
        .join("rust-toolkit");

    let manifest = load_manifest(&dir).expect("manifest should parse");
    let skills = load_plugin_skills(&dir, &manifest);

    assert_eq!(skills.len(), 1, "should load 1 skill");
    assert_eq!(skills[0].name, "rust-check");
}

/// Verify discover_all finds the built-in plugin.
#[test]
fn discover_all_finds_builtin_plugins() {
    let registry = discover_all(&[]);
    let list = registry.list();

    let rust_toolkit = list.iter().find(|s| s.name == "rust-toolkit");
    assert!(rust_toolkit.is_some(), "rust-toolkit should be discovered");
    let plugin = rust_toolkit.unwrap();
    assert_eq!(plugin.source, "builtin");
    assert!(plugin.enabled, "sample plugin should be enabled by default");
    assert_eq!(plugin.skill_count, 1, "should report 1 skill");
}

/// Verify enable/disable cycle works on the sample plugin.
#[test]
fn plugin_enable_disable_cycle() {
    let mut registry = discover_all(&[]);

    assert!(registry.is_enabled("rust-toolkit"), "should start enabled");

    registry.disable("rust-toolkit").unwrap();
    assert!(!registry.is_enabled("rust-toolkit"), "should be disabled after disable");

    registry.enable("rust-toolkit").unwrap();
    assert!(registry.is_enabled("rust-toolkit"), "should be enabled after re-enable");
}

/// Verify render_plugin_block includes the sample plugin.
#[test]
fn render_block_includes_enabled_plugin() {
    let registry = discover_all(&[]);
    let block = render_plugin_block(&registry);

    assert!(block.is_some(), "should render a block when plugins are enabled");
    let text = block.unwrap();
    assert!(text.contains("rust-toolkit"), "block should mention rust-toolkit");
    assert!(text.contains("1 skill(s)"), "block should mention skill count");
}

/// Verify disable then re-enable removes and restores skills.
#[test]
fn disabling_plugin_removes_skills() {
    let mut registry = discover_all(&[]);

    let skills_before = registry.enabled_skills();
    assert!(skills_before.iter().any(|s| s.name == "rust-check"),
        "rust-check should be in enabled skills before disable");

    registry.disable("rust-toolkit").unwrap();
    let skills_after = registry.enabled_skills();
    assert!(!skills_after.iter().any(|s| s.name == "rust-check"),
        "rust-check should NOT be in enabled skills after disable");

    registry.enable("rust-toolkit").unwrap();
    let skills_again = registry.enabled_skills();
    assert!(skills_again.iter().any(|s| s.name == "rust-check"),
        "rust-check should be back after re-enable");
}
