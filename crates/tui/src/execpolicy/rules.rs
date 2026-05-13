//! Execpolicy rules loaded from TOML configuration.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use anyhow::{Context, Result};
use serde::Deserialize;

use super::matcher::pattern_matches;
use crate::command_safety::prefix_allow_matches;

static PATH_REGEX_CACHE: LazyLock<Mutex<BTreeMap<String, Option<regex::Regex>>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecPolicyDecision {
    Allow,
    Deny(String),
    AskUser(String),
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ExecPolicyConfig {
    #[serde(default)]
    pub rules: BTreeMap<String, RuleSet>,
    /// File access policy rules keyed by operation category (e.g. "read_file").
    #[serde(default, rename = "file")]
    pub file_rules: BTreeMap<String, FileRuleSet>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RuleSet {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

/// File-specific rule set using glob-style path patterns.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct FileRuleSet {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

impl ExecPolicyConfig {
    pub fn from_str(contents: &str) -> Result<Self> {
        toml::from_str(contents).context("failed to parse execpolicy.toml")
    }

    pub fn from_path(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read execpolicy file {}", path.display()))?;
        Self::from_str(&contents)
    }

    pub fn evaluate(&self, command: &str) -> ExecPolicyDecision {
        for (group, rules) in &self.rules {
            for pattern in &rules.deny {
                if pattern_matches(pattern, command) {
                    return ExecPolicyDecision::Deny(format!(
                        "execpolicy denied by {group}: {pattern}"
                    ));
                }
            }
        }

        for (group, rules) in &self.rules {
            for pattern in &rules.allow {
                // Allow rules use arity-aware prefix matching first so that
                // `allow = ["git status"]` matches `git status -s` but NOT
                // `git push origin main`.  Fall back to regex-style
                // `pattern_matches` for wildcard patterns (e.g. `cargo *`).
                if prefix_allow_matches(pattern, command) || pattern_matches(pattern, command) {
                    let _ = group;
                    return ExecPolicyDecision::Allow;
                }
            }
        }

        ExecPolicyDecision::AskUser("execpolicy: no matching allow rule".to_string())
    }

    /// Evaluate a file path against the configured file policy rules.
    pub fn evaluate_file(&self, category: &str, path: &str) -> ExecPolicyDecision {
        let mut rule_sets = Vec::new();
        if let Some(rules) = self.file_rules.get(category) {
            rule_sets.push(rules);
        }
        if category != "default"
            && let Some(rules) = self.file_rules.get("default")
        {
            rule_sets.push(rules);
        }

        if rule_sets.is_empty() {
            return ExecPolicyDecision::Allow;
        }

        for rules in &rule_sets {
            for pattern in &rules.deny {
                if pattern_matches_path(pattern, path) {
                    return ExecPolicyDecision::Deny(format!(
                        "file policy denied by {category}: {pattern}"
                    ));
                }
            }
        }

        for rules in &rule_sets {
            for pattern in &rules.allow {
                if pattern_matches_path(pattern, path) {
                    return ExecPolicyDecision::Allow;
                }
            }
        }

        ExecPolicyDecision::AskUser(format!(
            "file policy: no matching allow rule for {category}"
        ))
    }
}

pub fn default_execpolicy_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".deepseek").join("execpolicy.toml"))
}

fn pattern_matches_path(pattern: &str, path: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    if let Ok(cache) = PATH_REGEX_CACHE.lock()
        && let Some(re) = cache.get(pattern)
    {
        return re.as_ref().is_some_and(|re| re.is_match(path));
    }

    let mut escaped = regex::escape(pattern);
    escaped = escaped.replace("\\*\\*/", "(?:.*/)?");
    escaped = escaped.replace("\\*\\*", ".*");
    escaped = escaped.replace("\\*", "[^/\\\\]*");

    let re_str = format!("^{escaped}$");
    let compiled = regex::Regex::new(&re_str).ok();
    let matched = compiled.as_ref().is_some_and(|re| re.is_match(path));
    if let Ok(mut cache) = PATH_REGEX_CACHE.lock() {
        cache.insert(pattern.to_string(), compiled);
    }
    matched
}

pub fn is_file_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "read_file" | "write_file" | "edit_file" | "apply_patch"
    )
}

pub fn extract_file_path<'a>(
    tool_input: &'a serde_json::Value,
    tool_name: &str,
) -> Option<&'a str> {
    match tool_name {
        "read_file" | "write_file" | "edit_file" | "apply_patch" => {
            tool_input.get("path")?.as_str()
        }
        _ => None,
    }
}

pub fn extract_file_paths(tool_input: &serde_json::Value, tool_name: &str) -> Vec<String> {
    match tool_name {
        "read_file" | "write_file" | "edit_file" => extract_file_path(tool_input, tool_name)
            .map(|path| vec![path.to_string()])
            .unwrap_or_default(),
        "apply_patch" => extract_apply_patch_paths(tool_input),
        _ => Vec::new(),
    }
}

fn extract_apply_patch_paths(tool_input: &serde_json::Value) -> Vec<String> {
    if let Some(path) = tool_input.get("path").and_then(serde_json::Value::as_str) {
        return vec![path.to_string()];
    }

    let Some(patch) = tool_input.get("patch").and_then(serde_json::Value::as_str) else {
        return Vec::new();
    };

    let mut paths = Vec::new();
    for line in patch.lines() {
        let candidate = line
            .strip_prefix("+++ ")
            .or_else(|| line.strip_prefix("--- "));
        let Some(candidate) = candidate else {
            continue;
        };
        if let Some(path) = normalize_diff_path(candidate) {
            push_unique_path(&mut paths, path);
        }
    }
    paths
}

fn normalize_diff_path(raw: &str) -> Option<String> {
    let path = raw.split_whitespace().next()?.trim_matches('"');
    if path == "/dev/null" {
        return None;
    }
    let normalized = path
        .strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path);
    if normalized.is_empty() {
        None
    } else {
        Some(normalized.to_string())
    }
}

fn push_unique_path(paths: &mut Vec<String>, path: String) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

pub fn load_default_policy() -> Result<Option<ExecPolicyConfig>> {
    let Some(path) = default_execpolicy_path() else {
        return Ok(None);
    };
    if !path.exists() {
        return Ok(None);
    }
    ExecPolicyConfig::from_path(&path).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execpolicy_evaluate() {
        let config = ExecPolicyConfig {
            rules: BTreeMap::from([
                (
                    "git".to_string(),
                    RuleSet {
                        allow: vec!["git status".to_string(), "git log *".to_string()],
                        deny: vec!["git push --force".to_string()],
                    },
                ),
                (
                    "danger".to_string(),
                    RuleSet {
                        allow: vec![],
                        deny: vec!["rm -rf /".to_string()],
                    },
                ),
            ]),
            ..Default::default()
        };

        assert!(matches!(
            config.evaluate("git status"),
            ExecPolicyDecision::Allow
        ));
        assert!(matches!(
            config.evaluate("git log --oneline"),
            ExecPolicyDecision::Allow
        ));
        assert!(matches!(
            config.evaluate("git push --force"),
            ExecPolicyDecision::Deny(_)
        ));
        assert!(matches!(
            config.evaluate("unknown command"),
            ExecPolicyDecision::AskUser(_)
        ));
    }

    #[test]
    fn test_prefix_rule_allows_git_status_with_flags() {
        // Arity-aware: `allow = ["git status"]` must match `git status -s`.
        let config = ExecPolicyConfig {
            rules: BTreeMap::from([(
                "git".to_string(),
                RuleSet {
                    allow: vec!["git status".to_string()],
                    deny: vec![],
                },
            )]),
            ..Default::default()
        };

        assert!(matches!(
            config.evaluate("git status -s"),
            ExecPolicyDecision::Allow
        ));
        assert!(matches!(
            config.evaluate("git status --porcelain"),
            ExecPolicyDecision::Allow
        ));
        // Push must NOT match the "git status" allow rule.
        assert!(matches!(
            config.evaluate("git push origin main"),
            ExecPolicyDecision::AskUser(_)
        ));
    }

    #[test]
    fn test_prefix_rule_allows_cargo_check_variants() {
        let config = ExecPolicyConfig {
            rules: BTreeMap::from([(
                "cargo".to_string(),
                RuleSet {
                    allow: vec!["cargo check".to_string()],
                    deny: vec![],
                },
            )]),
            ..Default::default()
        };

        assert!(matches!(
            config.evaluate("cargo check"),
            ExecPolicyDecision::Allow
        ));
        assert!(matches!(
            config.evaluate("cargo check --workspace"),
            ExecPolicyDecision::Allow
        ));
        assert!(matches!(
            config.evaluate("cargo build --release"),
            ExecPolicyDecision::AskUser(_)
        ));
    }

    #[test]
    fn test_file_policy_allows_and_denies() {
        let config = ExecPolicyConfig {
            rules: BTreeMap::new(),
            file_rules: BTreeMap::from([(
                "write_file".to_string(),
                FileRuleSet {
                    allow: vec!["src/**/*.rs".to_string(), "*.md".to_string()],
                    deny: vec!["src/secrets.rs".to_string(), ".env".to_string()],
                },
            )]),
        };

        assert!(matches!(
            config.evaluate_file("write_file", "src/main.rs"),
            ExecPolicyDecision::Allow
        ));
        assert!(matches!(
            config.evaluate_file("write_file", "README.md"),
            ExecPolicyDecision::Allow
        ));
        assert!(matches!(
            config.evaluate_file("write_file", "src/secrets.rs"),
            ExecPolicyDecision::Deny(_)
        ));
        assert!(matches!(
            config.evaluate_file("write_file", ".env"),
            ExecPolicyDecision::Deny(_)
        ));
        assert!(matches!(
            config.evaluate_file("write_file", "Cargo.lock"),
            ExecPolicyDecision::AskUser(_)
        ));
    }

    #[test]
    fn test_file_policy_fallback_to_default_category() {
        let config = ExecPolicyConfig {
            rules: BTreeMap::new(),
            file_rules: BTreeMap::from([(
                "default".to_string(),
                FileRuleSet {
                    allow: vec!["*".to_string()],
                    deny: vec![".env".to_string()],
                },
            )]),
        };

        assert!(matches!(
            config.evaluate_file("read_file", "README.md"),
            ExecPolicyDecision::Allow
        ));
        assert!(matches!(
            config.evaluate_file("edit_file", ".env"),
            ExecPolicyDecision::Deny(_)
        ));
    }

    #[test]
    fn test_file_policy_no_rules_configured() {
        let config: ExecPolicyConfig = Default::default();
        assert!(matches!(
            config.evaluate_file("write_file", "anything.txt"),
            ExecPolicyDecision::Allow
        ));
    }

    #[test]
    fn test_pattern_matches_path_directly() {
        assert!(pattern_matches_path("*.md", "README.md"));
        assert!(pattern_matches_path("src/**/*.rs", "src/main.rs"));
        assert!(pattern_matches_path("src/**/*.rs", "src/a/b.rs"));
        assert!(!pattern_matches_path("*.md", "src/README.txt"));
    }

    #[test]
    fn test_extract_apply_patch_path_override() {
        let input = serde_json::json!({
            "path": "src/main.rs",
            "patch": "--- a/other.rs\n+++ b/other.rs\n@@ -1 +1 @@\n-old\n+new\n"
        });

        assert_eq!(
            extract_file_paths(&input, "apply_patch"),
            vec!["src/main.rs".to_string()]
        );
    }

    #[test]
    fn test_extract_apply_patch_paths_from_diff_headers() {
        let input = serde_json::json!({
            "patch": "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-old\n+new\ndiff --git a/.env b/.env\n--- a/.env\n+++ b/.env\n@@ -1 +1 @@\n-old\n+new\n"
        });

        assert_eq!(
            extract_file_paths(&input, "apply_patch"),
            vec!["src/a.rs".to_string(), ".env".to_string()]
        );
    }
}
