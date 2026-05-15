#![allow(dead_code)]

//! Feature flags and metadata for DeepSeek TUI.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};

use serde::{Deserialize, Serialize};

/// Lifecycle stage for a feature flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Experimental,
    Beta,
    Stable,
    Deprecated,
    Removed,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Experimental => "experimental",
            Self::Beta => "beta",
            Self::Stable => "stable",
            Self::Deprecated => "deprecated",
            Self::Removed => "removed",
        }
    }
}

/// Unique features toggled via configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Feature {
    /// Enable the default shell tool.
    ShellTool,
    /// Enable background sub-agent tooling.
    Subagents,
    /// Enable web search tool.
    WebSearch,
    /// Enable apply_patch tool.
    ApplyPatch,
    /// Enable MCP tools.
    Mcp,
    /// Enable execpolicy integration/tooling.
    ExecPolicy,
    /// Enable vision model for image analysis.
    VisionModel,
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Feature {
    pub fn key(self) -> &'static str {
        self.info().key
    }

    pub fn stage(self) -> Stage {
        self.info().stage
    }

    pub fn default_enabled(self) -> bool {
        self.info().default_enabled
    }

    fn info(self) -> &'static FeatureSpec {
        FEATURES
            .iter()
            .find(|spec| spec.id == self)
            .unwrap_or_else(|| unreachable!("missing FeatureSpec for {:?}", self))
    }
}

/// Holds the effective set of enabled features.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Features {
    enabled: BTreeSet<Feature>,
}

impl Features {
    /// Starts with built-in defaults.
    pub fn with_defaults() -> Self {
        let mut set = BTreeSet::new();
        for spec in FEATURES {
            if spec.default_enabled {
                set.insert(spec.id);
            }
        }
        Self { enabled: set }
    }

    pub fn enabled(&self, feature: Feature) -> bool {
        self.enabled.contains(&feature)
    }

    pub fn enable(&mut self, feature: Feature) -> &mut Self {
        self.enabled.insert(feature);
        self
    }

    pub fn disable(&mut self, feature: Feature) -> &mut Self {
        self.enabled.remove(&feature);
        self
    }

    pub fn apply_map(&mut self, entries: &BTreeMap<String, bool>) {
        for (key, enabled) in entries {
            if let Some(feature) = feature_from_key(key) {
                if *enabled {
                    self.enable(feature);
                } else {
                    self.disable(feature);
                }
            }
        }
    }

    pub fn enabled_features(&self) -> Vec<Feature> {
        let mut list: Vec<_> = self.enabled.iter().copied().collect();
        list.sort();
        list
    }
}

/// Keys accepted in `[features]` tables.
pub fn is_known_feature_key(key: &str) -> bool {
    FEATURES.iter().any(|spec| spec.key == key)
}

pub fn feature_from_key(key: &str) -> Option<Feature> {
    FEATURES
        .iter()
        .find(|spec| spec.key == key)
        .map(|spec| spec.id)
}

pub fn feature_spec_by_key(key: &str) -> Option<&'static FeatureSpec> {
    FEATURES.iter().find(|spec| spec.key == key)
}

pub fn render_feature_table(features: &Features) -> String {
    let mut output = String::from("feature\tstage\tenabled\n");
    for spec in FEATURES {
        let _ = writeln!(
            output,
            "{}\t{}\t{}",
            spec.key,
            spec.stage,
            features.enabled(spec.id)
        );
    }
    output
}

/// Deserializable features table for TOML.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct FeaturesToml {
    #[serde(flatten)]
    pub entries: BTreeMap<String, bool>,
}

/// Single registry of all feature definitions.
#[derive(Debug, Clone, Copy)]
pub struct FeatureSpec {
    pub id: Feature,
    pub key: &'static str,
    pub stage: Stage,
    pub default_enabled: bool,
}

pub const FEATURES: &[FeatureSpec] = &[
    FeatureSpec {
        id: Feature::ShellTool,
        key: "shell_tool",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::Subagents,
        key: "subagents",
        stage: Stage::Experimental,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::WebSearch,
        key: "web_search",
        stage: Stage::Experimental,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::ApplyPatch,
        key: "apply_patch",
        stage: Stage::Experimental,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::Mcp,
        key: "mcp",
        stage: Stage::Experimental,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::ExecPolicy,
        key: "exec_policy",
        stage: Stage::Experimental,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::VisionModel,
        key: "vision_model",
        stage: Stage::Experimental,
        default_enabled: false,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_map_toggles_known_features_and_ignores_unknown_keys() {
        let mut features = Features::with_defaults();
        let entries = BTreeMap::from([
            ("mcp".to_string(), false),
            ("shell_tool".to_string(), false),
            ("not_real".to_string(), false),
        ]);

        features.apply_map(&entries);

        assert!(!features.enabled(Feature::Mcp));
        assert!(!features.enabled(Feature::ShellTool));
        assert_eq!(feature_from_key("not_real"), None);
    }

    #[test]
    fn render_feature_table_uses_registry_order_and_effective_state() {
        let mut features = Features::with_defaults();
        features.disable(Feature::Mcp);

        let table = render_feature_table(&features);
        let lines = table.lines().collect::<Vec<_>>();

        assert_eq!(lines.first(), Some(&"feature\tstage\tenabled"));
        assert!(lines.contains(&"shell_tool\tstable\ttrue"));
        assert!(lines.contains(&"mcp\texperimental\tfalse"));
    }
}
