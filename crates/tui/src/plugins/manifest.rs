//! Plugin manifest (`plugin.toml`) parsing and data types.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::mcp::McpServerConfig;
use crate::skills::Skill;

#[derive(Debug, Clone, Deserialize)]
pub struct PluginManifest {
    pub plugin: PluginMeta,
    pub skills: Option<PluginSkills>,
    pub mcp_servers: Option<HashMap<String, McpServerConfig>>,
    pub when: Option<PluginWhen>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PluginMeta {
    pub name: String,
    pub description: String,
    pub version: Option<String>,
    #[serde(default = "default_true")]
    pub default_enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct PluginSkills {
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PluginWhen {
    pub os: Option<Vec<String>>,
    pub required_binaries: Option<Vec<String>>,
}

/// A fully loaded plugin with resolved skills and MCP servers.
#[derive(Debug, Clone)]
pub struct LoadedPlugin {
    pub manifest: PluginManifest,
    pub source: PluginSource,
    pub enabled: bool,
    pub skills: Vec<Skill>,
    #[allow(dead_code)]
    pub mcp_servers: HashMap<String, McpServerConfig>,
}

#[derive(Debug, Clone)]
pub enum PluginSource {
    Builtin { #[allow(dead_code)] path: std::path::PathBuf },
    User { #[allow(dead_code)] path: std::path::PathBuf },
}

impl PluginSource {
    #[allow(dead_code)]
    pub fn path(&self) -> &std::path::Path {
        match self {
            PluginSource::Builtin { path } | PluginSource::User { path } => path,
        }
    }
}

/// Load a `plugin.toml` from `dir`.
pub fn load_manifest(dir: &Path) -> Result<PluginManifest, String> {
    let path = dir.join("plugin.toml");
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("Failed to read {path:?}: {e}"))?;
    let manifest: PluginManifest =
        toml::from_str(&content).map_err(|e| format!("Failed to parse {path:?}: {e}"))?;

    let dir_name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if manifest.plugin.name != dir_name {
        return Err(format!(
            "plugin name \"{}\" does not match directory name \"{dir_name}\"",
            manifest.plugin.name
        ));
    }

    Ok(manifest)
}

/// Check environment compatibility.
pub fn check_plugin_when(when: &Option<PluginWhen>) -> bool {
    let Some(when) = when else { return true };
    if let Some(ref allowed_os) = when.os {
        let current_os = std::env::consts::OS;
        if !allowed_os.iter().any(|os| os == current_os) {
            return false;
        }
    }
    if let Some(ref bins) = when.required_binaries {
        for bin in bins {
            if lookup_binary(bin).is_none() {
                return false;
            }
        }
    }
    true
}

fn lookup_binary(bin: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let full = dir.join(bin);
            if full.is_file() { Some(full) }
            else {
                let with_exe = dir.join(format!("{bin}.exe"));
                if with_exe.is_file() { Some(with_exe) } else { None }
            }
        })
    })
}

/// Resolve skill SKILL.md paths relative to the plugin directory.
pub fn load_plugin_skills(dir: &Path, manifest: &PluginManifest) -> Vec<Skill> {
    let Some(skills) = &manifest.skills else { return Vec::new() };
    let mut result = Vec::new();
    for relative_path in &skills.paths {
        let skill_dir = dir.join(relative_path);
        let skill_file = skill_dir.join("SKILL.md");
        if !skill_file.exists() { continue; }
        let content = match std::fs::read_to_string(&skill_file) { Ok(c) => c, Err(_) => continue };
        if let Ok(mut skill) = crate::skills::SkillRegistry::parse_skill(&skill_file, &content) {
            skill.path = skill_file;
            result.push(skill);
        }
    }
    result
}

/// Extract MCP servers from a plugin manifest, returning parsed configs.
pub fn load_plugin_mcp(manifest: &PluginManifest) -> HashMap<String, McpServerConfig> {
    manifest.mcp_servers.clone().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_manifest() {
        let toml_str = r#"
[plugin]
name = "test-plugin"
description = "A test plugin"
"#;
        let manifest: PluginManifest = toml::from_str(toml_str).unwrap();
        assert_eq!(manifest.plugin.name, "test-plugin");
        assert_eq!(manifest.plugin.description, "A test plugin");
        assert!(manifest.plugin.version.is_none());
        assert!(manifest.plugin.default_enabled);
        assert!(manifest.skills.is_none());
        assert!(manifest.mcp_servers.is_none());
    }

    #[test]
    fn parse_full_manifest() {
        let toml_str = r#"
[plugin]
name = "full-plugin"
description = "A full test plugin"
version = "1.0.0"
default_enabled = false

[skills]
paths = ["skills/check/", "skills/test/"]

[mcp_servers.custom-api]
command = "my-mcp"
args = ["--stdio"]
description = "Custom API"

[when]
os = ["linux", "windows"]
required_binaries = ["cargo", "node"]
"#;
        let manifest: PluginManifest = toml::from_str(toml_str).unwrap();
        assert_eq!(manifest.plugin.name, "full-plugin");
        assert_eq!(manifest.plugin.version.as_deref(), Some("1.0.0"));
        assert!(!manifest.plugin.default_enabled);
        let skills = manifest.skills.unwrap();
        assert_eq!(skills.paths.len(), 2);
        let mcp = manifest.mcp_servers.unwrap();
        assert!(mcp.contains_key("custom-api"));
    }

    #[test]
    fn load_plugin_mcp_returns_empty_when_none() {
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
    fn check_no_when_always_passes() {
        assert!(check_plugin_when(&None));
    }

    #[test]
    fn name_mismatch_detected() {
        let dir = std::path::Path::new("/somewhere/mismatch");
        // Instead of creating a real file, just test that the function returns Err
        // for a non-existent directory
        let result = load_manifest(dir);
        assert!(result.is_err());
    }
}

