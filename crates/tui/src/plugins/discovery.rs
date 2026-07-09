use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::manifest::{LoadedPlugin, PluginManifest};
use super::registry::PluginRegistry;

const PLUGIN_MANIFEST: &str = "plugin.toml";
const OVERRIDES_FILE: &str = "overrides.json";

pub fn default_user_plugins_dir() -> PathBuf {
    codewhale_config::codewhale_home()
        .map(|p| p.join("plugins"))
        .unwrap_or_else(|_| PathBuf::from("/tmp/codewhale/plugins"))
}

/// Path of the JSON file that records `/plugin enable|disable` choices so they
/// survive restarts.
pub fn default_overrides_path() -> PathBuf {
    default_user_plugins_dir().join(OVERRIDES_FILE)
}

/// Read the persisted enable/disable overrides. Missing or malformed files
/// yield an empty map — the user simply gets the default enablement.
pub fn load_overrides(path: &Path) -> HashMap<String, bool> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default()
}

/// Persist the enable/disable overrides, creating the parent directory if
/// needed.
pub fn save_overrides(path: &Path, overrides: &HashMap<String, bool>) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(overrides)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, json)
}

pub fn discover_all(builtin_dirs: &[&str]) -> PluginRegistry {
    let mut registry = PluginRegistry::new();

    let overrides_path = default_overrides_path();
    let overrides = load_overrides(&overrides_path);
    registry.set_overrides_store(overrides_path, overrides);

    for dir in builtin_dirs {
        let path = PathBuf::from(dir);
        if path.exists() {
            discover_from_dir(&path, &mut registry, true);
        }
    }

    let user_dir = default_user_plugins_dir();
    if user_dir.exists() {
        discover_from_dir(&user_dir, &mut registry, false);
    }

    // Discovery recomputes `enabled` from `!builtin`; re-apply the user's
    // persisted choices so a prior enable/disable actually sticks (#3918).
    registry.apply_overrides();

    registry
}

fn discover_from_dir(dir: &Path, registry: &mut PluginRegistry, builtin: bool) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let manifest_path = path.join(PLUGIN_MANIFEST);
        if !manifest_path.exists() {
            continue;
        }

        match PluginManifest::from_path(&manifest_path) {
            Ok(manifest) => {
                if !manifest.check_when() {
                    continue;
                }

                let name = manifest.plugin.name.clone();
                let plugin = LoadedPlugin {
                    manifest,
                    base_path: path,
                    enabled: !builtin,
                };

                registry.register(name, plugin);
            }
            Err(e) => {
                tracing::warn!("Failed to load plugin from {}: {}", path.display(), e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn default_user_plugins_dir_uses_explicit_codewhale_home() {
        let _env_lock = crate::test_support::lock_test_env();
        let tmp = TempDir::new().expect("tempdir");
        let home = tmp.path().join("codewhale-home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.as_os_str());

        assert_eq!(default_user_plugins_dir(), home.join("plugins"));
        assert_eq!(
            default_overrides_path(),
            home.join("plugins").join(OVERRIDES_FILE)
        );
    }
}
