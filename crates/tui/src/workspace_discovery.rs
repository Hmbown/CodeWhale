//! Shared workspace discovery filters for UI path pickers and mentions.

use std::path::{Component, Path};

/// Directories that must remain discoverable for `@`-mention completion and
/// fuzzy file resolution even when excluded by `.gitignore`.
pub(crate) const DISCOVERY_ALWAYS_DIRS: &[&str] = &[".deepseek", ".cursor", ".claude", ".agents"];

/// Root-relative directories that are too large or generated to discover
/// with gitignore disabled. Exact user-specified paths may still resolve.
const DISCOVERY_EXCLUDED_SUBDIRS: &[&str] =
    &[".deepseek/snapshots", ".worktrees", ".claude/worktrees"];

/// Directory basenames that should not be traversed by fallback discovery
/// walks that deliberately disable gitignore.
const DISCOVERY_EXCLUDED_DIR_NAMES: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".venv",
    "venv",
    "env",
    "dist",
    "build",
    ".next",
    ".turbo",
    "coverage",
    "__pycache__",
    ".pytest_cache",
    ".ruff_cache",
];

/// Check whether `path` is under a root-relative excluded discovery subtree.
pub(crate) fn path_is_excluded_from_discovery(walk_root: &Path, path: &Path) -> bool {
    DISCOVERY_EXCLUDED_SUBDIRS
        .iter()
        .any(|excluded| path.starts_with(walk_root.join(excluded)))
}

/// Filter for walks that turn off gitignore to surface explicit hidden paths.
pub(crate) fn should_skip_unignored_discovery_entry(walk_root: &Path, path: &Path) -> bool {
    if path == walk_root {
        return false;
    }

    if path_is_excluded_from_discovery(walk_root, path) {
        return true;
    }

    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| DISCOVERY_EXCLUDED_DIR_NAMES.contains(&name))
}

/// Leading `!` on a picker or `@`-completion query includes hidden files
/// (#5550). The remainder is the actual name/path needle.
pub(crate) fn parse_hidden_file_prefix(query: &str) -> (bool, &str) {
    let trimmed = query.trim();
    match trimmed.strip_prefix('!') {
        Some(rest) => (true, rest.trim_start()),
        None => (false, trimmed),
    }
}

/// True when any path component is a dotfile/dotdir (not `.` / `..`).
pub(crate) fn path_has_hidden_component(path: &str) -> bool {
    Path::new(path)
        .components()
        .any(|component| match component {
            Component::Normal(name) => {
                let name = name.to_string_lossy();
                name.starts_with('.') && name != "."
            }
            _ => false,
        })
}

#[cfg(test)]
mod tests {
    use super::{parse_hidden_file_prefix, path_has_hidden_component};

    #[test]
    fn bang_prefix_is_an_explicit_hidden_toggle() {
        assert_eq!(parse_hidden_file_prefix("lib.rs"), (false, "lib.rs"));
        assert_eq!(parse_hidden_file_prefix("!"), (true, ""));
        assert_eq!(parse_hidden_file_prefix("!env"), (true, "env"));
        assert_eq!(parse_hidden_file_prefix("! .env"), (true, ".env"));
        assert_eq!(parse_hidden_file_prefix("!src/.env"), (true, "src/.env"));
        assert_eq!(
            parse_hidden_file_prefix("  !.gitignore  "),
            (true, ".gitignore")
        );
    }

    #[test]
    fn hidden_component_detects_dotfiles_anywhere_in_the_relative_path() {
        assert!(path_has_hidden_component(".env"));
        assert!(path_has_hidden_component("src/.hidden.rs"));
        assert!(path_has_hidden_component(".agents/skill.md"));
        assert!(!path_has_hidden_component("src/lib.rs"));
        assert!(!path_has_hidden_component("README.md"));
    }
}
