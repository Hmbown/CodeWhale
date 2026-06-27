use std::collections::HashMap;

use crate::skills::Skill;

use super::manifest::{LoadedPlugin, PluginSource};

/// Central registry for all discovered plugins.
#[derive(Debug, Clone)]
pub struct PluginRegistry {
    builtins: HashMap<String, LoadedPlugin>,
    users: HashMap<String, LoadedPlugin>,
    user_overrides: HashMap<String, bool>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        PluginRegistry {
            builtins: HashMap::new(),
            users: HashMap::new(),
            user_overrides: HashMap::new(),
        }
    }

    pub fn register(&mut self, plugin: LoadedPlugin) {
        let name = plugin.manifest.plugin.name.clone();
        match &plugin.source {
            PluginSource::Builtin { .. } => {
                self.builtins.insert(name, plugin);
            }
            PluginSource::User { .. } => {
                self.users.insert(name, plugin);
            }
        }
    }

    pub fn enable(&mut self, name: &str) -> Result<(), String> {
        if self.get(name).is_none() {
            return Err(format!("plugin \"{name}\" not found"));
        }
        self.user_overrides.insert(name.to_string(), true);
        Ok(())
    }

    pub fn disable(&mut self, name: &str) -> Result<(), String> {
        if self.get(name).is_none() {
            return Err(format!("plugin \"{name}\" not found"));
        }
        self.user_overrides.insert(name.to_string(), false);
        Ok(())
    }

    #[allow(dead_code)]
    pub fn is_enabled(&self, name: &str) -> bool {
        if let Some(&override_val) = self.user_overrides.get(name) {
            return override_val;
        }
        self.get(name)
            .map(|p| p.enabled)
            .unwrap_or(false)
    }

    pub fn list(&self) -> Vec<PluginSummary> {
        let mut summaries = Vec::new();
        for (name, plugin) in &self.builtins {
            summaries.push(PluginSummary {
                name: name.clone(),
                description: plugin.manifest.plugin.description.clone(),
                version: plugin.manifest.plugin.version.clone(),
                source: "builtin".to_string(),
                enabled: self.enabled_with_overrides(name, &plugin.enabled),
                skill_count: plugin.skills.len(),
            });
        }
        for (name, plugin) in &self.users {
            summaries.push(PluginSummary {
                name: name.clone(),
                description: plugin.manifest.plugin.description.clone(),
                version: plugin.manifest.plugin.version.clone(),
                source: "user".to_string(),
                enabled: self.enabled_with_overrides(name, &plugin.enabled),
                skill_count: plugin.skills.len(),
            });
        }
        summaries.sort_by(|a, b| a.name.cmp(&b.name));
        summaries
    }

    fn enabled_with_overrides(&self, name: &str, default: &bool) -> bool {
        if let Some(&override_val) = self.user_overrides.get(name) {
            return override_val;
        }
        *default
    }

    #[allow(dead_code)]
    pub fn enabled_skills(&self) -> Vec<&Skill> {
        let mut skills = Vec::new();
        for plugin in self.builtins.values() {
            if self.is_enabled(&plugin.manifest.plugin.name) {
                skills.extend(plugin.skills.iter());
            }
        }
        for plugin in self.users.values() {
            if self.is_enabled(&plugin.manifest.plugin.name) {
                skills.extend(plugin.skills.iter());
            }
        }
        skills
    }

    #[allow(dead_code)]
    pub fn enabled_mcp_servers(&self) -> HashMap<String, crate::mcp::McpServerConfig> {
        let mut servers = HashMap::new();
        for plugin in self.builtins.values() {
            if self.is_enabled(&plugin.manifest.plugin.name) {
                servers.extend(plugin.mcp_servers.clone());
            }
        }
        for plugin in self.users.values() {
            if self.is_enabled(&plugin.manifest.plugin.name) {
                servers.extend(plugin.mcp_servers.clone());
            }
        }
        servers
    }

    fn get(&self, name: &str) -> Option<&LoadedPlugin> {
        self.users
            .get(name)
            .or_else(|| self.builtins.get(name))
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct PluginSummary {
    pub name: String,
    pub description: String,
    pub version: Option<String>,
    pub source: String,
    pub enabled: bool,
    pub skill_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::manifest::{PluginManifest, PluginMeta};

    fn make_plugin(name: &str, default_enabled: bool, source: PluginSource) -> LoadedPlugin {
        LoadedPlugin {
            manifest: PluginManifest {
                plugin: PluginMeta {
                    name: name.to_string(),
                    description: format!("{name} description"),
                    version: Some("1.0.0".to_string()),
                    default_enabled,
                },
                skills: None,
                mcp_servers: None,
                when: None,
            },
            source,
            enabled: default_enabled,
            skills: Vec::new(),
            mcp_servers: HashMap::new(),
        }
    }

    #[test]
    fn empty_registry_lists_nothing() {
        let r = PluginRegistry::new();
        assert!(r.list().is_empty());
        assert!(!r.is_enabled("anything"));
    }

    #[test]
    fn register_builtin_and_list() {
        let mut r = PluginRegistry::new();
        let p = make_plugin("alpha", true, PluginSource::Builtin { path: ".".into() });
        r.register(p);
        let list = r.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "alpha");
        assert!(list[0].enabled);
        assert_eq!(list[0].source, "builtin");
    }

    #[test]
    fn register_user_overrides_builtin() {
        let mut r = PluginRegistry::new();
        let builtin = make_plugin("dup", false, PluginSource::Builtin { path: ".".into() });
        let user = make_plugin("dup", true, PluginSource::User { path: ".".into() });
        r.register(builtin);
        r.register(user);
        let list = r.list();
        assert_eq!(list.len(), 2);
        let dup_builtin = list.iter().find(|s| s.source == "builtin").unwrap();
        let dup_user = list.iter().find(|s| s.source == "user").unwrap();
        assert!(!dup_builtin.enabled);
        assert!(dup_user.enabled);
    }

    #[test]
    fn enable_disable_plugin() {
        let mut r = PluginRegistry::new();
        let p = make_plugin("toggle", true, PluginSource::Builtin { path: ".".into() });
        r.register(p);
        assert!(r.is_enabled("toggle"));
        r.disable("toggle").unwrap();
        assert!(!r.is_enabled("toggle"));
        r.enable("toggle").unwrap();
        assert!(r.is_enabled("toggle"));
    }

    #[test]
    fn enable_unknown_returns_err() {
        let mut r = PluginRegistry::new();
        assert!(r.enable("ghost").is_err());
        assert!(r.disable("ghost").is_err());
    }

    #[test]
    fn enabled_skills_empty_when_no_skills() {
        let mut r = PluginRegistry::new();
        let p = make_plugin("test", true, PluginSource::Builtin { path: ".".into() });
        r.register(p);
        assert!(r.enabled_skills().is_empty());
    }

    #[test]
    fn enabled_mcp_servers_empty_by_default() {
        let r = PluginRegistry::new();
        assert!(r.enabled_mcp_servers().is_empty());
    }
}
