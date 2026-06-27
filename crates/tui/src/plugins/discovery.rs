use std::path::{Path, PathBuf};
use super::manifest::{LoadedPlugin, PluginSource, check_plugin_when, load_manifest, load_plugin_skills, load_plugin_mcp};
use super::registry::PluginRegistry;

pub fn builtin_plugins_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets").join("plugins")
}

pub fn user_plugins_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|p| p.join(".codewhale").join("plugins"))
}

pub fn discover_all(_config_disabled: &[String]) -> PluginRegistry {
    let mut registry = PluginRegistry::new();
    let builtin_dir = builtin_plugins_dir();
    if builtin_dir.exists() {
        scan_plugin_dir(&builtin_dir, |dir| {
            let manifest = load_manifest(dir).ok()?;
            let compatible = check_plugin_when(&manifest.when);
            let skills = load_plugin_skills(dir, &manifest);
            let mcp = load_plugin_mcp(&manifest);
            let enabled = compatible && manifest.plugin.default_enabled;
            Some(LoadedPlugin {
                manifest,
                source: PluginSource::Builtin { path: dir.to_path_buf() },
                enabled,
                skills,
                mcp_servers: mcp,
            })
        }).into_iter().for_each(|p| { registry.register(p); });
    }
    if let Some(user_dir) = user_plugins_dir() {
        if user_dir.exists() {
            scan_plugin_dir(&user_dir, |dir| {
                let manifest = load_manifest(dir).ok()?;
                let compatible = check_plugin_when(&manifest.when);
                let skills = load_plugin_skills(dir, &manifest);
                let mcp = load_plugin_mcp(&manifest);
                let enabled = compatible && manifest.plugin.default_enabled;
                Some(LoadedPlugin {
                    manifest,
                    source: PluginSource::User { path: dir.to_path_buf() },
                    enabled,
                    skills,
                    mcp_servers: mcp,
                })
            }).into_iter().for_each(|p| { registry.register(p); });
        }
    }
    registry
}

fn scan_plugin_dir<F>(dir: &Path, f: F) -> Vec<LoadedPlugin>
where
    F: Fn(&Path) -> Option<LoadedPlugin>,
{
    let mut plugins = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(plugin) = f(&path) {
                    plugins.push(plugin);
                }
            }
        }
    }
    plugins
}
