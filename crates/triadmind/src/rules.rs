//! # Rules — Always-On architecture rules injection
//!
//! Ported from triadmind-core/rules.ts
//!
//! Injects TriadMind architecture guard rules into AGENTS.md and Cursor rules files.
//! The rules are enclosed in `<!-- TRIADMIND_RULES_START -->` / `<!-- TRIADMIND_RULES_END -->`
//! markers so they can be safely updated or removed without affecting existing content.
//!
//! @LeftBranch: install_always_on_rules, strip_existing_rules

use std::path::{Path, PathBuf};

const START_MARKER: &str = "<!-- TRIADMIND_RULES_START -->";
const END_MARKER: &str = "<!-- TRIADMIND_RULES_END -->";

/// Configuration paths for a TriadMind workspace.
pub struct RulesPaths {
    /// Project root directory.
    pub project_root: PathBuf,
    /// Path to triad-map.json (relative to project root or absolute).
    pub map_file: PathBuf,
    /// Path to triadmind config.json.
    pub config_file: PathBuf,
    /// Path to master-prompt.md.
    pub master_prompt_file: PathBuf,
    /// Path to .triadmind directory.
    pub triad_dir: PathBuf,
}

impl RulesPaths {
    pub fn new(project_root: PathBuf) -> Self {
        let triad_dir = project_root.join(".triadmind");
        Self {
            map_file: triad_dir.join("triad-map.json"),
            config_file: triad_dir.join("config.json"),
            master_prompt_file: triad_dir.join("master-prompt.md"),
            triad_dir,
            project_root,
        }
    }
}

/// Install always-on rules into AGENTS.md and .cursor/rules/triadmind.mdc.
///
/// Returns the paths that were written.
pub fn install_always_on_rules(paths: &RulesPaths) -> std::io::Result<Vec<PathBuf>> {
    let mut written = Vec::new();

    // Ensure directories exist
    std::fs::create_dir_all(&paths.triad_dir)?;
    let cursor_rules_dir = paths.project_root.join(".cursor").join("rules");
    std::fs::create_dir_all(&cursor_rules_dir)?;

    // Build the rules content
    let agent_rules = build_agent_rules(paths);

    // Write to AGENTS.md in project root
    let agents_path = paths.project_root.join("AGENTS.md");
    upsert_agents_md(&agents_path, &agent_rules)?;
    written.push(agents_path);

    // Write to .cursor/rules/triadmind.mdc (Cursor IDE integration)
    let cursor_rule_path = cursor_rules_dir.join("triadmind.mdc");
    let cursor_rule = build_cursor_rule(paths);
    std::fs::write(&cursor_rule_path, &cursor_rule)?;
    written.push(cursor_rule_path);

    Ok(written)
}

/// Remove TriadMind rules from AGENTS.md.
///
/// Safe: only removes content between START_MARKER and END_MARKER.
pub fn remove_rules(paths: &RulesPaths) -> std::io::Result<()> {
    let agents_path = paths.project_root.join("AGENTS.md");
    if !agents_path.exists() {
        return Ok(());
    }

    let content = std::fs::read_to_string(&agents_path)?;
    let cleaned = strip_existing_rules(&content);

    if cleaned.is_empty() {
        let _ = std::fs::remove_file(&agents_path);
    } else {
        std::fs::write(&agents_path, cleaned.trim_end())?;
    }

    Ok(())
}

/// Check if AGENTS.md already contains TriadMind rules.
pub fn has_rules(paths: &RulesPaths) -> bool {
    let agents_path = paths.project_root.join("AGENTS.md");
    if !agents_path.exists() {
        return false;
    }

    std::fs::read_to_string(&agents_path)
        .map(|content| content.contains(START_MARKER))
        .unwrap_or(false)
}

fn build_agent_rules(paths: &RulesPaths) -> String {
    let map_rel = relativize(&paths.project_root, &paths.map_file);
    let config_rel = relativize(&paths.project_root, &paths.config_file);
    let master_rel = relativize(&paths.project_root, &paths.master_prompt_file);

    let rules = vec![
        format!("{START_MARKER}"),
        "# TriadMind Always-On Rules".to_string(),
        String::new(),
        format!("- Before answering architecture questions, prefer `{map_rel}`; if it is missing or stale, regenerate it with `triadmind sync --force`."),
        format!("- Before generating or modifying code, read `{config_rel}`; use `{master_rel}` only when it already exists or when the task explicitly needs a regenerated prompt artifact."),
        "- Do not jump straight into implementation when a topology upgrade is required.".to_string(),
        "- Prefer the TriadMind sequence: Macro -> Meso -> Micro -> draft-protocol -> visualizer -> apply -> handoff.".to_string(),
        "- If the user message starts with `@triadmind`, treat it as a TriadMind directive.".to_string(),
        "- If the body is a control command like `init`, `macro`, `meso`, `micro`, `finalize`, `plan`, `apply`, `renormalize`, `heal`, or `handoff`, route to the matching TriadMind lifecycle action.".to_string(),
        "- Otherwise, treat it as a silent topology-upgrade demand: run the full protocol workflow first, then continue to apply and handoff.".to_string(),
        "- Use `reuse` first, then `modify`, and only use `create_child` when the current leaf node cannot safely absorb the new responsibility.".to_string(),
        "- If a runtime error occurs, prefer generating a repair protocol via `.triadmind/healing-prompt.md` when present instead of ad-hoc code edits.".to_string(),
        format!("{END_MARKER}"),
        String::new(),
    ];

    rules.join("\n")
}

fn build_cursor_rule(paths: &RulesPaths) -> String {
    let map_rel = relativize(&paths.project_root, &paths.map_file);
    let config_rel = relativize(&paths.project_root, &paths.config_file);
    let master_rel = relativize(&paths.project_root, &paths.master_prompt_file);

    format!(
        "---\ndescription: TriadMind always-on architecture guard\nalwaysApply: true\n---\n\n\
         Before answering architecture questions, read `{map_rel}`.\n\
         Before generating or changing code, read `{config_rel}` and `{master_rel}`.\n\
         When a feature changes topology, do not skip protocol design. Follow:\n\
         Macro -> Meso -> Micro -> draft-protocol -> visualizer -> apply -> handoff.\n\
         If the user message starts with `@triadmind`, treat it as a TriadMind directive.\n\
         If it is a control command like `init`, `macro`, `meso`, `micro`, `finalize`, `plan`, \
         `apply`, `renormalize`, `heal`, or `handoff`, route to that lifecycle action.\n\
         Otherwise, treat it as a silent topology-upgrade demand, complete the protocol workflow \
         first, then continue to apply and handoff.\n\
         Prefer `reuse`, then `modify`, and only then `create_child`.\n"
    )
}

/// Insert or update TriadMind rules in an AGENTS.md file.
fn upsert_agents_md(agents_path: &Path, triad_rules: &str) -> std::io::Result<()> {
    let existing = if agents_path.exists() {
        std::fs::read_to_string(agents_path)?
    } else {
        String::new()
    };

    let normalized = strip_existing_rules(&existing).trim_end().to_string();
    let next = if normalized.is_empty() {
        triad_rules.to_string()
    } else {
        format!("{normalized}\n\n{triad_rules}")
    };

    std::fs::write(agents_path, next)?;
    Ok(())
}

/// Strip existing TriadMind rules (between START_MARKER and END_MARKER) from content.
pub fn strip_existing_rules(content: &str) -> String {
    let start = match content.find(START_MARKER) {
        Some(idx) => idx,
        None => return content.to_string(),
    };

    let prefix = &content[..start];
    let after_start = &content[start + START_MARKER.len()..];

    let end = match after_start.find(END_MARKER) {
        Some(idx) => idx + END_MARKER.len(),
        None => {
            // No end marker found — only strip the start marker line
            return format!("{}{}", prefix, after_start);
        }
    };

    let suffix = &after_start[end..];
    // Remove any trailing newline left by the marker block
    let suffix = suffix.trim_start_matches('\n');

    format!("{}{}", prefix, suffix)
}

/// Compute a relative path from `base` to `target`, for display in rules.
fn relativize(base: &Path, target: &Path) -> String {
    target
        .strip_prefix(base)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| target.to_string_lossy().to_string())
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_existing_rules_removes_block() {
        let content = "# Project\n\nSome text.\n\n<!-- TRIADMIND_RULES_START -->\nrule 1\nrule 2\n<!-- TRIADMIND_RULES_END -->\n\nMore text.";
        let stripped = strip_existing_rules(content);
        assert_eq!(stripped, "# Project\n\nSome text.\n\nMore text.");
    }

    #[test]
    fn test_strip_existing_rules_no_marker() {
        let content = "# Just a normal file";
        assert_eq!(strip_existing_rules(content), content);
    }

    #[test]
    fn test_strip_existing_rules_only_marker() {
        let content = "<!-- TRIADMIND_RULES_START -->\nrules\n<!-- TRIADMIND_RULES_END -->";
        let stripped = strip_existing_rules(content);
        assert_eq!(stripped, "");
    }

    #[test]
    fn test_has_rules_detects_marker() {
        let content = "text\n<!-- TRIADMIND_RULES_START -->\n<!-- TRIADMIND_RULES_END -->";
        assert!(content.contains(START_MARKER));
    }

    #[test]
    fn test_relativize_same_path() {
        let base = Path::new("/project");
        let target = Path::new("/project/.triadmind/triad-map.json");
        assert_eq!(relativize(base, target), ".triadmind/triad-map.json");
    }
}
