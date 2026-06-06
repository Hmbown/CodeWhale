//! File-defined sub-agent role definitions.
//!
//! Claude Code lets users define reusable sub-agent *types* as Markdown files
//! with YAML-ish frontmatter. This module brings the same to DeepSeek TUI:
//! drop a `<name>.md` into `~/.deepseek/agents/` (path mirrors `skills/`) and
//! its body becomes the sub-agent's system prompt when that name is spawned —
//! either as the `subagent_type` or via the `role` field. A matching file
//! overrides the built-in role intro; the shared output contract is still
//! appended by the caller, so file-defined agents stay well-behaved.
//!
//! Format:
//! ```text
//! ---
//! name: code-review
//! description: Reviews a diff for correctness and risk.
//! model: deepseek-v4-pro        # optional
//! tools: read_file, grep_files  # optional, comma-separated
//! ---
//! <the agent's system prompt body>
//! ```

use std::path::{Path, PathBuf};

/// A parsed agent definition from `<name>.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDefinition {
    pub name: String,
    pub description: String,
    pub model: Option<String>,
    pub tools: Option<Vec<String>>,
    /// The system-prompt body (everything after the frontmatter).
    pub body: String,
}

/// Resolve the default agents directory: `~/.deepseek/agents`.
///
/// Honors `HOME` / `USERPROFILE` first (matching the rest of the config
/// resolution) before falling back to the platform home dir.
#[must_use]
pub fn default_agents_dir() -> Option<PathBuf> {
    home_dir().map(|home| home.join(".deepseek").join("agents"))
}

fn home_dir() -> Option<PathBuf> {
    for var in ["HOME", "USERPROFILE"] {
        if let Some(p) = std::env::var_os(var) {
            let p = PathBuf::from(p);
            if !p.as_os_str().is_empty() {
                return Some(p);
            }
        }
    }
    dirs::home_dir()
}

/// Parse a definition from raw file contents. `name_hint` (the file stem) is
/// used when the frontmatter omits an explicit `name`. Returns `None` when the
/// content has no usable body.
#[must_use]
pub fn parse_definition(name_hint: &str, raw: &str) -> Option<AgentDefinition> {
    let (frontmatter, body) = split_frontmatter(raw);

    let mut name = name_hint.trim().to_string();
    let mut description = String::new();
    let mut model = None;
    let mut tools = None;

    for line in frontmatter.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim().trim_matches('"').trim_matches('\'').trim();
        if value.is_empty() {
            continue;
        }
        match key.as_str() {
            "name" => name = value.to_string(),
            "description" => description = value.to_string(),
            "model" => model = Some(value.to_string()),
            "tools" | "allowed-tools" | "allowed_tools" => {
                let list: Vec<String> = value
                    .split(',')
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect();
                if !list.is_empty() {
                    tools = Some(list);
                }
            }
            _ => {}
        }
    }

    let body = body.trim();
    if body.is_empty() || name.is_empty() {
        return None;
    }

    Some(AgentDefinition {
        name,
        description,
        model,
        tools,
        body: body.to_string(),
    })
}

/// Split `raw` into `(frontmatter, body)`. When there is no `---`-delimited
/// frontmatter, the whole input is the body.
fn split_frontmatter(raw: &str) -> (&str, &str) {
    let trimmed = raw.trim_start_matches('\u{feff}'); // tolerate a BOM
    let rest = match trimmed
        .strip_prefix("---\n")
        .or_else(|| trimmed.strip_prefix("---\r\n"))
    {
        Some(rest) => rest,
        None => return ("", trimmed),
    };
    // Find the closing `---` on its own line.
    if let Some((fm_end, line_end)) = find_closing_fence(rest) {
        (&rest[..fm_end], &rest[line_end..])
    } else {
        ("", trimmed)
    }
}

/// Returns `(start_of_fence, end_of_fence_line)` byte offsets for the closing
/// `---` line within `s`, or `None`.
fn find_closing_fence(s: &str) -> Option<(usize, usize)> {
    let mut offset = 0;
    for line in s.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']).trim();
        if trimmed == "---" {
            return Some((offset, offset + line.len()));
        }
        offset += line.len();
    }
    None
}

/// Load a named definition from `dir`. Matching is case-insensitive on the
/// file stem (`code-review` matches `Code-Review.md`).
#[must_use]
pub fn load_agent_definition(dir: &Path, name: &str) -> Option<AgentDefinition> {
    let want = name.trim().to_ascii_lowercase();
    if want.is_empty() {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if stem == want {
            let raw = std::fs::read_to_string(&path).ok()?;
            return parse_definition(&stem, &raw);
        }
    }
    None
}

/// List every definition in `dir`, sorted by name. Unreadable or malformed
/// files are skipped.
#[must_use]
pub fn list_agent_definitions(dir: &Path) -> Vec<AgentDefinition> {
    let mut defs = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return defs;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        if let Ok(raw) = std::fs::read_to_string(&path)
            && let Some(def) = parse_definition(&stem, &raw)
        {
            defs.push(def);
        }
    }
    defs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    defs
}

/// Render an "Available Agents" context block for the system prompt, or `None`
/// when no definitions exist. Mirrors the skills-context shape.
#[must_use]
pub fn render_available_agents_context(dir: &Path) -> Option<String> {
    let defs = list_agent_definitions(dir);
    if defs.is_empty() {
        return None;
    }
    let mut out = String::from(
        "## Available Agents\n\nUser-defined sub-agent roles (spawn via `agent_spawn` with the matching `subagent_type`/`role`):\n",
    );
    for def in &defs {
        let desc = if def.description.is_empty() {
            String::new()
        } else {
            format!(" — {}", def.description)
        };
        out.push_str(&format!("- `{}`{desc}\n", def.name));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    const SAMPLE: &str = "---\nname: code-review\ndescription: Reviews a diff.\nmodel: deepseek-v4-pro\ntools: read_file, grep_files\n---\nYou are a meticulous code reviewer. Focus on correctness and risk.";

    #[test]
    fn parses_frontmatter_and_body() {
        let def = parse_definition("code-review", SAMPLE).expect("parse");
        assert_eq!(def.name, "code-review");
        assert_eq!(def.description, "Reviews a diff.");
        assert_eq!(def.model.as_deref(), Some("deepseek-v4-pro"));
        assert_eq!(
            def.tools,
            Some(vec!["read_file".to_string(), "grep_files".to_string()])
        );
        assert!(def.body.starts_with("You are a meticulous code reviewer"));
    }

    #[test]
    fn body_without_frontmatter_is_kept() {
        let def = parse_definition("plain", "Just a body, no frontmatter.").expect("parse");
        assert_eq!(def.name, "plain");
        assert_eq!(def.body, "Just a body, no frontmatter.");
        assert!(def.description.is_empty());
    }

    #[test]
    fn empty_body_is_rejected() {
        assert!(parse_definition("x", "---\nname: x\n---\n   ").is_none());
    }

    #[test]
    fn loads_by_case_insensitive_stem() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("Code-Review.md"), SAMPLE).unwrap();
        let def = load_agent_definition(dir.path(), "code-review").expect("load");
        assert_eq!(def.name, "code-review");
        assert!(load_agent_definition(dir.path(), "missing").is_none());
    }

    #[test]
    fn lists_and_renders_context_sorted() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("code-review.md"), SAMPLE).unwrap();
        fs::write(
            dir.path().join("explore.md"),
            "---\nname: explore\ndescription: Read-only scout.\n---\nExplore the codebase.",
        )
        .unwrap();
        let defs = list_agent_definitions(dir.path());
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].name, "code-review"); // sorted
        assert_eq!(defs[1].name, "explore");

        let ctx = render_available_agents_context(dir.path()).expect("context");
        assert!(ctx.contains("`code-review` — Reviews a diff."));
        assert!(ctx.contains("`explore` — Read-only scout."));
    }

    #[test]
    fn empty_dir_renders_no_context() {
        let dir = tempdir().unwrap();
        assert!(render_available_agents_context(dir.path()).is_none());
    }
}
