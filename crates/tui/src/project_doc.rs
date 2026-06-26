//! Project document discovery and loading
//!
//! Supports auto-discovery of project instructions like Claude Code.
//! Priority: AGENTS.md > WHALE.md (deprecated) > .claude/instructions.md > CLAUDE.md > .codewhale/instructions.md > .deepseek/instructions.md

use std::fs;
use std::io::{self, Read};
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
#[allow(dead_code)] // DEFAULT_MAX_BYTES; used by read_project_docs (see #3490) // Used by read_project_docs (see #3490)
pub const DEFAULT_MAX_BYTES: usize = 32768;

/// A discovered project document
#[derive(Debug, Clone)]
#[allow(dead_code)] // ProjectDoc struct; part of project context API (see #3490)
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
            if is_regular_file_path(&doc_path) {
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
#[allow(dead_code)] // Public API; project_context.rs provides the active code path (see #3490)
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

        if let Ok(content) = read_regular_file_to_string(path) {
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
#[allow(dead_code)] // Used by read_project_docs (see #3490)
pub fn format_instructions(path: &Path, content: &str) -> String {
    format!(
        "# Project instructions from {}\n\n<INSTRUCTIONS>\n{}\n</INSTRUCTIONS>",
        path.display(),
        content.trim()
    )
}

/// Load project docs from workspace with default settings
#[allow(dead_code)] // Convenience function; project_context.rs provides the active code path (see #3490)
pub fn load_from_workspace(workspace: &Path) -> Option<String> {
    let paths = discover_paths(workspace);
    read_project_docs(&paths, DEFAULT_MAX_BYTES)
}

fn is_regular_file_path(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        let file_type = metadata.file_type();
        file_type.is_file() && !file_type.is_symlink()
    })
}

fn read_regular_file_to_string(path: &Path) -> io::Result<String> {
    let metadata = fs::symlink_metadata(path)?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() || !file_type.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing non-regular project doc {}", path.display()),
        ));
    }

    let mut file = open_regular_file(path)?;
    let mut content = String::new();
    file.read_to_string(&mut content)?;
    Ok(content)
}

#[cfg(unix)]
fn open_regular_file(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(not(unix))]
fn open_regular_file(path: &Path) -> io::Result<fs::File> {
    fs::File::open(path)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[cfg(unix)]
    #[test]
    fn discover_paths_ignores_symlinked_project_docs() {
        let workspace = tempdir().expect("workspace tempdir");
        let outside = tempdir().expect("outside tempdir");
        let outside_agents = outside.path().join("AGENTS.md");
        fs::write(&outside_agents, "outside instructions").expect("write outside agents");
        std::os::unix::fs::symlink(&outside_agents, workspace.path().join("AGENTS.md"))
            .expect("symlink agents");

        let paths = discover_paths(workspace.path());

        assert!(
            paths.is_empty(),
            "symlinked project docs must not be discovered: {paths:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn read_project_docs_rejects_symlinked_paths() {
        let workspace = tempdir().expect("workspace tempdir");
        let outside = tempdir().expect("outside tempdir");
        let outside_agents = outside.path().join("AGENTS.md");
        let linked_agents = workspace.path().join("AGENTS.md");
        fs::write(&outside_agents, "outside instructions").expect("write outside agents");
        std::os::unix::fs::symlink(&outside_agents, &linked_agents).expect("symlink agents");

        let docs = read_project_docs(&[linked_agents], DEFAULT_MAX_BYTES);

        assert!(
            docs.is_none(),
            "symlinked project docs must not be read: {docs:?}"
        );
    }
}
