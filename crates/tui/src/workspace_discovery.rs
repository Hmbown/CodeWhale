//! Shared workspace discovery filters for UI path pickers and mentions.

use std::path::Path;

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
