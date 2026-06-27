use super::registry::PluginRegistry;

/// Render a system-prompt block listing all enabled plugins.
///
/// Follows the same pattern as `crate::skills::render_skills_block`.
pub fn render_plugin_block(registry: &PluginRegistry) -> Option<String> {
    let enabled: Vec<_> = registry
        .list()
        .into_iter()
        .filter(|s| s.enabled)
        .collect();

    if enabled.is_empty() {
        return None;
    }

    let mut out = String::new();
    out.push_str("## Plugins\n");
    out.push_str(
        "A plugin bundles skills and optional MCP servers into a toggleable \
         unit. Below are the plugins currently enabled in this session.\n\n",
    );
    out.push_str("### Enabled plugins\n");

    for summary in &enabled {
        let skills_note = if summary.skill_count > 0 {
            format!(" ({} skill(s))", summary.skill_count)
        } else {
            String::new()
        };
        out.push_str(&format!(
            "- **{}**{} — {}\n",
            summary.name, skills_note, summary.description
        ));
    }

    out.push('\n');
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::manifest::{PluginManifest, PluginMeta, PluginSource, LoadedPlugin};

    fn make_enabled_plugin(name: &str, desc: &str, enabled: bool) -> LoadedPlugin {
        LoadedPlugin {
            manifest: PluginManifest {
                plugin: PluginMeta {
                    name: name.to_string(),
                    description: desc.to_string(),
                    version: None,
                    default_enabled: enabled,
                },
                skills: None,
                mcp_servers: None,
                when: None,
            },
            source: PluginSource::Builtin { path: ".".into() },
            enabled,
            skills: Vec::new(),
            mcp_servers: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn render_no_enabled_plugins_returns_none() {
        let registry = PluginRegistry::new();
        assert!(render_plugin_block(&registry).is_none());
    }

    #[test]
    fn render_skips_disabled_plugins() {
        let mut registry = PluginRegistry::new();
        registry.register(make_enabled_plugin("off", "disabled plugin", false));
        assert!(render_plugin_block(&registry).is_none());
    }

    #[test]
    fn render_includes_enabled_plugins() {
        let mut registry = PluginRegistry::new();
        registry.register(make_enabled_plugin("alpha", "first plugin", true));
        registry.register(make_enabled_plugin("beta", "second plugin", false));
        let block = render_plugin_block(&registry).expect("should render");
        assert!(block.contains("alpha"));
        assert!(block.contains("first plugin"));
        assert!(!block.contains("beta"));
    }
}
