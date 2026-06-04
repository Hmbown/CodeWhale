//! Project document discovery and loading
//!
//! Supports auto-discovery of project instructions like Claude Code.
//! Priority: AGENTS.md > WHALE.md (deprecated) > .claude/instructions.md > CLAUDE.md > .codewhale/instructions.md > .deepseek/instructions.md

use std::path::{Path, PathBuf};

/// Document filenames to search for (in priority order).
/// `AGENTS.md` is canonical. `WHALE.md` is **deprecated** (read-only legacy
/// fallback, now below `AGENTS.md`); CodeWhale-specific authority policy lives
/// in `.codewhale/constitution.json`. `CLAUDE.md` and the `*/instructions.md`
/// variants are read-only compatibility fallbacks.
pub const DOC_FILENAMES: &[&str] = &[
    "AGENTS.md",
    "WHALE.md", // deprecated: legacy CodeWhale-native, read-only fallback
    ".claude/instructions.md",
    "CLAUDE.md",
    ".codewhale/instructions.md",
    ".deepseek/instructions.md",
];

/// Maximum bytes to read from project docs (default: 32KB)
#[allow(dead_code)] // Used by read_project_docs
pub const DEFAULT_MAX_BYTES: usize = 32768;

/// A discovered project document
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ProjectDoc {
    pub path: PathBuf,
    pub content: String,
}

/// Walk from cwd up to git root, collecting all project docs
pub fn discover_paths(cwd: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let git_root = find_git_root(cwd);

    let mut current = cwd.to_path_buf();
    loop {
        for filename in DOC_FILENAMES {
            let doc_path = current.join(filename);
            if doc_path.exists() && doc_path.is_file() {
                paths.push(doc_path);
            }
        }

        // Stop at git root or filesystem root
        if let Some(ref root) = git_root
            && current == *root
        {
            break;
        }

        match current.parent() {
            Some(parent) if parent != current => {
                current = parent.to_path_buf();
            }
            _ => break,
        }
    }

    // Reverse so parent docs come first (will be overridden by child docs)
    paths.reverse();
    paths
}

/// Find the git root directory from cwd
pub(crate) fn find_git_root(cwd: &Path) -> Option<PathBuf> {
    let mut current = cwd.to_path_buf();
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        match current.parent() {
            Some(parent) if parent != current => {
                current = parent.to_path_buf();
            }
            _ => return None,
        }
    }
}

/// Read and concatenate project docs with byte limit
#[allow(dead_code)] // Public API; project_context.rs provides the active code path
pub fn read_project_docs(paths: &[PathBuf], max_bytes: usize) -> Option<String> {
    if paths.is_empty() {
        return None;
    }

    let mut combined = String::new();
    let mut total_bytes = 0;

    for path in paths {
        if total_bytes >= max_bytes {
            break;
        }

        if let Ok(content) = std::fs::read_to_string(path) {
            let remaining = max_bytes.saturating_sub(total_bytes);
            let content = if content.len() > remaining {
                // Truncate to remaining bytes at a word boundary if possible
                let truncated: String = content.chars().take(remaining).collect();
                format!("{truncated}\n\n[...truncated...]")
            } else {
                content
            };

            if !combined.is_empty() {
                combined.push_str("\n\n---\n\n");
            }
            combined.push_str(&format_instructions(path, &content));
            total_bytes += content.len();
        }
    }

    if combined.is_empty() {
        None
    } else {
        Some(combined)
    }
}

/// Format project instructions for injection into system prompt
#[allow(dead_code)] // Used by read_project_docs
pub fn format_instructions(path: &Path, content: &str) -> String {
    format!(
        "# Project instructions from {}\n\n<INSTRUCTIONS>\n{}\n</INSTRUCTIONS>",
        path.display(),
        content.trim()
    )
}

/// Load project docs from workspace with default settings
#[allow(dead_code)] // Convenience function; project_context.rs provides the active code path
pub fn load_from_workspace(workspace: &Path) -> Option<String> {
    let paths = discover_paths(workspace);
    read_project_docs(&paths, DEFAULT_MAX_BYTES)
}
