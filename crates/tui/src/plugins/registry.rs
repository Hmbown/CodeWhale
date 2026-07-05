use std::collections::HashMap;
use std::path::PathBuf;

use super::manifest::LoadedPlugin;

#[derive(Debug, Clone, Default)]
pub struct PluginRegistry {
    plugins: HashMap<String, LoadedPlugin>,
    user_overrides: HashMap<String, bool>,
    /// Where `user_overrides` is persisted. Discovery always sets this via
    /// [`set_overrides_store`](Self::set_overrides_store); it is `None` only
    /// when a registry is built without a persistence store (e.g. a direct
    /// `PluginRegistry::new()` in unit tests), in which case enable/disable
    /// stays in-memory.
    overrides_path: Option<PathBuf>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed the persisted enable/disable overrides and remember where to write
    /// them back. Called by discovery before plugins are registered; the
    /// overrides are then applied with [`apply_overrides`](Self::apply_overrides).
    pub fn set_overrides_store(&mut self, path: PathBuf, overrides: HashMap<String, bool>) {
        self.overrides_path = Some(path);
        self.user_overrides = overrides;
    }

    /// Apply every persisted override onto the currently-registered plugins.
    /// Discovery recomputes `enabled` from scratch (`!builtin`) on each launch,
    /// so this is what makes a prior `/plugin enable|disable` actually stick.
    pub fn apply_overrides(&mut self) {
        for (name, &enabled) in &self.user_overrides {
            if let Some(plugin) = self.plugins.get_mut(name) {
                plugin.enabled = enabled;
            }
        }
    }

    pub fn register(&mut self, name: String, plugin: LoadedPlugin) {
        self.plugins.insert(name, plugin);
    }

    pub fn enable(&mut self, name: &str) -> bool {
        if let Some(plugin) = self.plugins.get_mut(name) {
            plugin.enabled = true;
            self.user_overrides.insert(name.to_string(), true);
            self.persist_overrides();
            true
        } else {
            false
        }
    }

    pub fn disable(&mut self, name: &str) -> bool {
        if let Some(plugin) = self.plugins.get_mut(name) {
            plugin.enabled = false;
            self.user_overrides.insert(name.to_string(), false);
            self.persist_overrides();
            true
        } else {
            false
        }
    }

    /// Write the current override map to disk (best-effort). A failure here is
    /// logged but never fails the command — the in-memory toggle still applies
    /// for the current session.
    fn persist_overrides(&self) {
        if let Some(path) = &self.overrides_path
            && let Err(e) = super::discovery::save_overrides(path, &self.user_overrides)
        {
            tracing::warn!(
                "failed to persist plugin overrides to {}: {e}",
                path.display()
            );
        }
    }

    pub fn list(&self) -> Vec<(&String, &LoadedPlugin)> {
        self.plugins.iter().collect()
    }

    pub fn get(&self, name: &str) -> Option<&LoadedPlugin> {
        self.plugins.get(name)
    }

    pub fn enabled_plugins(&self) -> Vec<(&String, &LoadedPlugin)> {
        self.list_enabled()
    }

    pub fn list_enabled(&self) -> Vec<(&String, &LoadedPlugin)> {
        self.plugins.iter().filter(|(_, p)| p.enabled).collect()
    }

    pub fn is_enabled(&self, name: &str) -> bool {
        self.plugins.get(name).is_some_and(|p| p.enabled)
    }

    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }
}
