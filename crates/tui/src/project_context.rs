//! Project context loading for DeepSeek TUI.
//!
//! This module handles loading project-specific context files that provide
//! instructions and context to the AI agent. These include:
//!
//! - `AGENTS.md` - Project-level agent instructions (primary)
//! - `.claude/instructions.md` - Claude-style hidden instructions
//! - `CLAUDE.md` - Claude-style instructions
//! - `.deepseek/instructions.md` - Hidden instructions file (legacy)
//!
//! The loaded content is injected into the system prompt to give the agent
//! context about the project's conventions, structure, and requirements.

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use thiserror::Error;

/// Names of project context files to look for, in priority order.
const PROJECT_CONTEXT_FILES: &[&str] = &[
    "AGENTS.md",
    ".claude/instructions.md",
    "CLAUDE.md",
    ".deepseek/instructions.md",
];

/// User-level project instructions loaded as a fallback when the workspace and
/// its parents do not define project context.
const GLOBAL_AGENTS_RELATIVE_PATH: &[&str] = &[".deepseek", "AGENTS.md"];

/// Maximum size for project context files (to prevent loading huge files)
const MAX_CONTEXT_SIZE: usize = 100 * 1024; // 100KB
const PACK_README_MAX_CHARS: usize = 4_000;
const PACK_MAX_ENTRIES: usize = 220;
const PACK_MAX_SOURCE_FILES: usize = 60;
const PACK_MAX_CONFIG_FILES: usize = 60;
const PACK_MAX_DEPTH: usize = 4;
const PACK_IGNORED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    "dist",
    "build",
    "target",
    ".idea",
    ".vscode",
    ".pytest_cache",
    ".DS_Store",
];
const PACK_ALLOWED_HIDDEN_DIRS: &[&str] = &[".github"];
const PACK_ALLOWED_HIDDEN_FILES: &[&str] = &[".editorconfig", ".gitattributes", ".gitignore"];
const PACK_IGNORED_FILE_NAMES: &[&str] = &[".DS_Store"];
const PACK_IGNORED_FILE_EXTENSIONS: &[&str] = &[
    "7z", "avif", "db", "gif", "gz", "ico", "jpeg", "jpg", "log", "mov", "mp3", "mp4", "pdf",
    "png", "sqlite", "tar", "tgz", "wav", "webp", "zip",
];

// === Errors ===

#[derive(Debug, Error)]
enum ProjectContextError {
    #[error("Failed to read context metadata for {path}: {source}")]
    Metadata {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("Context file {path} is too large ({size} bytes, max {max})")]
    TooLarge {
        path: PathBuf,
        size: u64,
        max: usize,
    },
    #[error("Failed to read context file {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("Context file {path} is empty")]
    Empty { path: PathBuf },
}

/// Result of loading project context
#[derive(Debug, Clone)]
pub struct ProjectContext {
    /// The loaded instructions content
    pub instructions: Option<String>,
    /// Path to the loaded file (for display)
    pub source_path: Option<PathBuf>,
    /// Any warnings during loading
    pub warnings: Vec<String>,
    /// Project root directory
    #[allow(dead_code)] // Part of ProjectContext public interface
    pub project_root: PathBuf,
    /// Whether this is a trusted project
    pub is_trusted: bool,
}

impl ProjectContext {
    /// Create an empty project context
    pub fn empty(project_root: PathBuf) -> Self {
        Self {
            instructions: None,
            source_path: None,
            warnings: Vec::new(),
            project_root,
            is_trusted: false,
        }
    }

    /// Check if any instructions were loaded
    pub fn has_instructions(&self) -> bool {
        self.instructions.is_some()
    }

    /// Get the instructions as a formatted block for system prompt
    pub fn as_system_block(&self) -> Option<String> {
        self.instructions.as_ref().map(|content| {
            let source = self
                .source_path
                .as_ref()
                .map_or_else(|| "project".to_string(), |p| p.display().to_string());

            format!(
                "<project_instructions source=\"{source}\">\n{content}\n</project_instructions>"
            )
        })
    }
}

#[derive(Debug, Serialize)]
struct ProjectContextPack {
    project_name: String,
    directory_structure: Vec<String>,
    readme: Option<ReadmePack>,
    config_files: Vec<String>,
    key_source_files: Vec<String>,
    counts: BTreeMap<String, usize>,
}

#[derive(Debug, Serialize)]
struct ReadmePack {
    path: String,
    excerpt: String,
}

/// Generate a deterministic, cache-friendly project context pack.
///
/// The pack intentionally uses only stable workspace facts: relative paths,
/// sorted entries, bounded README text, and sorted JSON object fields. It does
/// not include timestamps, random ids, absolute temp paths, or live git state.
pub fn generate_project_context_pack(workspace: &Path) -> Option<String> {
    let mut entries = Vec::new();
    collect_pack_entries(workspace, workspace, 0, &mut entries);
    entries.sort();
    entries.truncate(PACK_MAX_ENTRIES);

    let mut config_files = entries
        .iter()
        .filter(|path| is_config_file(path))
        .take(PACK_MAX_CONFIG_FILES)
        .cloned()
        .collect::<Vec<_>>();
    config_files.sort();

    let mut key_source_files = entries
        .iter()
        .filter(|path| is_source_file(path))
        .take(PACK_MAX_SOURCE_FILES)
        .cloned()
        .collect::<Vec<_>>();
    key_source_files.sort();

    let readme = read_readme_excerpt(workspace, &entries);
    let mut counts = BTreeMap::new();
    counts.insert("config_files".to_string(), config_files.len());
    counts.insert("directory_entries".to_string(), entries.len());
    counts.insert("key_source_files".to_string(), key_source_files.len());

    let pack = ProjectContextPack {
        project_name: workspace
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("workspace")
            .to_string(),
        directory_structure: entries,
        readme,
        config_files,
        key_source_files,
        counts,
    };

    let json = serde_json::to_string_pretty(&pack).ok()?;
    Some(format!(
        "## Project Context Pack\n\n<project_context_pack>\n{json}\n</project_context_pack>"
    ))
}

fn collect_pack_entries(root: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
    if depth > PACK_MAX_DEPTH || out.len() >= PACK_MAX_ENTRIES {
        return;
    }

    let mut queue = VecDeque::new();
    queue.push_back((dir.to_path_buf(), depth));

    while let Some((current_dir, current_depth)) = queue.pop_front() {
        if current_depth > PACK_MAX_DEPTH || out.len() >= PACK_MAX_ENTRIES {
            continue;
        }

        let Ok(read_dir) = fs::read_dir(&current_dir) else {
            continue;
        };
        let mut children = read_dir.filter_map(Result::ok).collect::<Vec<_>>();
        children.sort_by_key(|entry| entry.path());

        for entry in children {
            if out.len() >= PACK_MAX_ENTRIES {
                break;
            }
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() && should_ignore_pack_dir(name) {
                continue;
            }
            if file_type.is_file() && should_ignore_pack_file(name) {
                continue;
            }

            if let Some(relative) = relative_slash_path(root, &path) {
                if file_type.is_dir() {
                    out.push(format!("{relative}/"));
                    if current_depth < PACK_MAX_DEPTH {
                        queue.push_back((path, current_depth + 1));
                    }
                } else if file_type.is_file() {
                    out.push(relative);
                }
            }
        }
    }
}

fn should_ignore_pack_dir(name: &str) -> bool {
    PACK_IGNORED_DIRS.contains(&name)
        || (name.starts_with('.') && !PACK_ALLOWED_HIDDEN_DIRS.contains(&name))
}

fn should_ignore_pack_file(name: &str) -> bool {
    if name.starts_with('.') && !PACK_ALLOWED_HIDDEN_FILES.contains(&name) {
        return true;
    }
    if PACK_IGNORED_FILE_NAMES.contains(&name) {
        return true;
    }
    let Some((_, ext)) = name.rsplit_once('.') else {
        return false;
    };
    PACK_IGNORED_FILE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
}

fn relative_slash_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for component in relative.components() {
        parts.push(component.as_os_str().to_string_lossy().to_string());
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

fn read_readme_excerpt(workspace: &Path, entries: &[String]) -> Option<ReadmePack> {
    let path = entries
        .iter()
        .find(|path| {
            let lower = path.to_ascii_lowercase();
            lower == "readme.md" || lower == "readme.txt" || lower == "readme"
        })?
        .clone();
    let raw = fs::read_to_string(workspace.join(&path)).ok()?;
    let excerpt = truncate_chars(raw.trim(), PACK_README_MAX_CHARS);
    if excerpt.is_empty() {
        None
    } else {
        Some(ReadmePack { path, excerpt })
    }
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    value.chars().take(max_chars).collect::<String>()
}

fn is_config_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(lower.as_str());
    matches!(
        name,
        "cargo.toml"
            | "package.json"
            | "tsconfig.json"
            | "pyproject.toml"
            | "requirements.txt"
            | "go.mod"
            | "config.toml"
            | "deepseek.toml"
            | "dockerfile"
            | "compose.yaml"
            | "compose.yml"
            | "docker-compose.yaml"
            | "docker-compose.yml"
            | "makefile"
    ) || lower.ends_with(".config.js")
        || lower.ends_with(".config.ts")
        || lower.ends_with(".toml")
        || lower.ends_with(".yaml")
        || lower.ends_with(".yml")
}

fn is_source_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    matches!(
        lower.rsplit('.').next(),
        Some(
            "rs" | "py"
                | "js"
                | "jsx"
                | "ts"
                | "tsx"
                | "go"
                | "java"
                | "kt"
                | "c"
                | "cc"
                | "cpp"
                | "h"
                | "hpp"
                | "cs"
                | "rb"
                | "php"
                | "swift"
                | "sql"
                | "sh"
                | "bash"
        )
    )
}

/// Load project context from the workspace directory.
///
/// This searches for known project context files and loads the first one found.
pub fn load_project_context(workspace: &Path) -> ProjectContext {
    let mut ctx = ProjectContext::empty(workspace.to_path_buf());

    // Search for project context files
    for filename in PROJECT_CONTEXT_FILES {
        let file_path = workspace.join(filename);

        if file_path.exists() && file_path.is_file() {
            match load_context_file(&file_path) {
                Ok(content) => {
                    ctx.instructions = Some(content);
                    ctx.source_path = Some(file_path);
                    break;
                }
                Err(error) => {
                    ctx.warnings.push(error.to_string());
                }
            }
        }
    }

    // Check for trust file
    ctx.is_trusted = check_trust_status(workspace);

    ctx
}

/// Load project context from parent directories as well.
///
/// This allows for monorepo setups where a root AGENTS.md applies to all subdirectories.
pub fn load_project_context_with_parents(workspace: &Path) -> ProjectContext {
    load_project_context_with_parents_and_home(workspace, dirs::home_dir().as_deref())
}

fn load_project_context_with_parents_and_home(
    workspace: &Path,
    home_dir: Option<&Path>,
) -> ProjectContext {
    let mut ctx = load_project_context(workspace);

    // If no context found in workspace, check parent directories
    if !ctx.has_instructions() {
        let mut current = workspace.parent();

        while let Some(parent) = current {
            let parent_ctx = load_project_context(parent);
            ctx.warnings.extend(parent_ctx.warnings.iter().cloned());
            if parent_ctx.has_instructions() {
                ctx.instructions = parent_ctx.instructions;
                ctx.source_path = parent_ctx.source_path;
                break;
            }

            current = parent.parent();
        }
    }

    // Always check `~/.deepseek/AGENTS.md` so user-wide preferences
    // travel into every session (#1157). When both global and project
    // instructions exist, the global block prepends the project's so
    // workspace overrides win the last word; when only global exists,
    // it continues to serve as the fallback. `source_path` keeps
    // pointing at the more-specific source (project > global) for
    // display purposes.
    if let Some(global_ctx) = load_global_agents_context(workspace, home_dir) {
        ctx.warnings.extend(global_ctx.warnings.iter().cloned());
        if let Some(global_text) = global_ctx.instructions {
            match ctx.instructions.take() {
                Some(project_text) => {
                    ctx.instructions = Some(merge_global_and_project_instructions(
                        &global_text,
                        global_ctx.source_path.as_deref(),
                        &project_text,
                    ));
                    // Leave `ctx.source_path` pointing at the project /
                    // parent file — that's the location the user might
                    // want to edit when something looks wrong.
                }
                None => {
                    ctx.instructions = Some(global_text);
                    ctx.source_path = global_ctx.source_path;
                }
            }
        }
    }

    // Auto-generate .deepseek/instructions.md when no context file exists anywhere.
    // This avoids the per-turn filesystem scan fallback in prompts.rs that
    // breaks KV prefix cache stability.
    if !ctx.has_instructions()
        && let Some(generated) = auto_generate_context(workspace)
    {
        let mut warnings = std::mem::take(&mut ctx.warnings);
        ctx = load_project_context(workspace);
        warnings.extend(ctx.warnings.iter().cloned());
        ctx.warnings = warnings;
        if !ctx.has_instructions() {
            // Loaded from the file we just wrote — use the generated content
            // directly as a last resort (shouldn't normally happen).
            ctx.instructions = Some(generated);
            ctx.source_path = None;
        }
    }

    ctx
}

/// Combine `~/.deepseek/AGENTS.md` (global, user-wide preferences) with a
/// project-local AGENTS.md/CLAUDE.md/instructions.md. Global comes first
/// so workspace-specific rules can override it — the model reads in
/// declared order. Each block is wrapped in a labelled fence so the
/// model can tell which level any rule comes from when the two sets
/// disagree (#1157).
fn merge_global_and_project_instructions(
    global: &str,
    global_source: Option<&Path>,
    project: &str,
) -> String {
    let global_label = global_source
        .map(|p| format!("<!-- global: {} -->", p.display()))
        .unwrap_or_else(|| "<!-- global -->".to_string());
    format!(
        "{global_label}\n{}\n\n<!-- project (overrides global where they conflict) -->\n{}",
        global.trim_end(),
        project.trim_start(),
    )
}

fn load_global_agents_context(workspace: &Path, home_dir: Option<&Path>) -> Option<ProjectContext> {
    let home = home_dir?;
    let mut path = home.to_path_buf();
    for component in GLOBAL_AGENTS_RELATIVE_PATH {
        path.push(component);
    }

    if !(path.exists() && path.is_file()) {
        return None;
    }

    let mut ctx = ProjectContext::empty(workspace.to_path_buf());
    match load_context_file(&path) {
        Ok(content) => {
            ctx.instructions = Some(content);
            ctx.source_path = Some(path);
        }
        Err(error) => ctx.warnings.push(error.to_string()),
    }
    Some(ctx)
}

/// Generate a context file from project tree + summary and write it to
/// `.deepseek/instructions.md`. Returns the generated content on success.
fn auto_generate_context(workspace: &Path) -> Option<String> {
    let deepseek_dir = workspace.join(".deepseek");
    let instructions_path = deepseek_dir.join("instructions.md");

    // Don't overwrite an existing file
    if instructions_path.exists() {
        return None;
    }

    let summary = crate::utils::summarize_project(workspace);
    let tree = crate::utils::project_tree(workspace, 2);

    let content = format!(
        "# Project Structure (Auto-generated)\n\n\
         > This file was automatically generated by DeepSeek TUI.\n\
         > You can edit or delete it at any time.\n\n\
         **Summary:** {summary}\n\n\
         **Tree:**\n```\n{tree}\n```"
    );

    // Create .deepseek/ directory if needed
    if let Err(e) = std::fs::create_dir_all(&deepseek_dir) {
        tracing::warn!("Failed to create .deepseek/ directory: {e}");
        return None;
    }

    match std::fs::write(&instructions_path, &content) {
        Ok(()) => {
            tracing::info!("Auto-generated {}", instructions_path.display());
            Some(content)
        }
        Err(e) => {
            tracing::warn!("Failed to write {}: {e}", instructions_path.display());
            None
        }
    }
}

/// Load a context file with size checking
fn load_context_file(path: &Path) -> Result<String, ProjectContextError> {
    // Check file size first
    let metadata = fs::metadata(path).map_err(|source| ProjectContextError::Metadata {
        path: path.to_path_buf(),
        source,
    })?;

    if metadata.len() > MAX_CONTEXT_SIZE as u64 {
        return Err(ProjectContextError::TooLarge {
            path: path.to_path_buf(),
            size: metadata.len(),
            max: MAX_CONTEXT_SIZE,
        });
    }

    // Read the file
    let content = fs::read_to_string(path).map_err(|source| ProjectContextError::Read {
        path: path.to_path_buf(),
        source,
    })?;

    // Basic validation
    if content.trim().is_empty() {
        return Err(ProjectContextError::Empty {
            path: path.to_path_buf(),
        });
    }

    Ok(content)
}

/// Check if this project is marked as trusted
fn check_trust_status(workspace: &Path) -> bool {
    if crate::config::is_workspace_trusted(workspace) {
        return true;
    }

    // Check for trust markers
    let trust_markers = [
        workspace.join(".deepseek").join("trusted"),
        workspace.join(".deepseek").join("trust.json"),
    ];

    for marker in &trust_markers {
        if marker.exists() {
            return true;
        }
    }

    false
}

/// Create a default AGENTS.md file for a project
pub fn create_default_agents_md(workspace: &Path) -> std::io::Result<PathBuf> {
    let agents_path = workspace.join("AGENTS.md");

    let default_content = r#"# Project Agent Instructions

This file provides guidance to AI agents (DeepSeek TUI, Claude Code, etc.) when working with code in this repository.

## File Location

Save this file as `AGENTS.md` in your project root so the CLI can load it automatically.

## Build and Development Commands

```bash
# Build
# cargo build              # Rust projects
# npm run build            # Node.js projects
# python -m build          # Python projects

# Test
# cargo test               # Rust
# npm test                 # Node.js
# pytest                   # Python

# Lint and Format
# cargo fmt && cargo clippy  # Rust
# npm run lint               # Node.js
# ruff check .               # Python
```

## Architecture Overview

<!-- Describe your project's high-level architecture here -->
<!-- Focus on the "big picture" that requires reading multiple files to understand -->

### Key Components

<!-- List and describe the main components/modules -->

### Data Flow

<!-- Describe how data flows through the system -->

## Configuration Files

<!-- List important configuration files and their purposes -->

## Extension Points

<!-- Describe how to extend the codebase (add new features, tools, etc.) -->

## Commit Messages

Use conventional commits: `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`
"#;

    fs::write(&agents_path, default_content)?;
    Ok(agents_path)
}

/// Merge multiple project contexts (e.g., from nested directories)
#[allow(dead_code)] // Public API for monorepo context merging
pub fn merge_contexts(contexts: &[ProjectContext]) -> Option<String> {
    let non_empty: Vec<_> = contexts
        .iter()
        .filter_map(ProjectContext::as_system_block)
        .collect();

    if non_empty.is_empty() {
        None
    } else {
        Some(non_empty.join("\n\n"))
    }
}

// === Unit Tests ===

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_load_project_context_empty() {
        let tmp = tempdir().expect("tempdir");
        let ctx = load_project_context(tmp.path());

        assert!(!ctx.has_instructions());
        assert!(ctx.source_path.is_none());
    }

    #[test]
    fn test_load_project_context_agents_md() {
        let tmp = tempdir().expect("tempdir");
        let agents_path = tmp.path().join("AGENTS.md");
        fs::write(&agents_path, "# Test Instructions\n\nFollow these rules.").expect("write");

        let ctx = load_project_context(tmp.path());

        assert!(ctx.has_instructions());
        assert!(
            ctx.instructions
                .as_ref()
                .unwrap()
                .contains("Test Instructions")
        );
        assert_eq!(ctx.source_path, Some(agents_path));
    }

    #[test]
    fn test_load_project_context_priority() {
        let tmp = tempdir().expect("tempdir");

        // Create both files - AGENTS.md should take priority
        fs::write(tmp.path().join("AGENTS.md"), "AGENTS content").expect("write");
        let claude_dir = tmp.path().join(".claude");
        fs::create_dir(&claude_dir).expect("mkdir");
        fs::write(claude_dir.join("instructions.md"), "CLAUDE content").expect("write");

        let ctx = load_project_context(tmp.path());

        assert!(ctx.has_instructions());
        assert!(
            ctx.instructions
                .as_ref()
                .unwrap()
                .contains("AGENTS content")
        );
    }

    #[test]
    fn test_load_project_context_hidden_dir() {
        let tmp = tempdir().expect("tempdir");
        let hidden_dir = tmp.path().join(".deepseek");
        fs::create_dir(&hidden_dir).expect("mkdir");
        fs::write(hidden_dir.join("instructions.md"), "Hidden instructions").expect("write");

        let ctx = load_project_context(tmp.path());

        assert!(ctx.has_instructions());
        assert!(
            ctx.instructions
                .as_ref()
                .unwrap()
                .contains("Hidden instructions")
        );
    }

    #[test]
    fn test_as_system_block() {
        let tmp = tempdir().expect("tempdir");
        let agents_path = tmp.path().join("AGENTS.md");
        fs::write(&agents_path, "Test content").expect("write");

        let ctx = load_project_context(tmp.path());
        let block = ctx.as_system_block().expect("block");

        assert!(block.contains("<project_instructions"));
        assert!(block.contains("Test content"));
        assert!(block.contains("</project_instructions>"));
    }

    #[test]
    fn test_empty_file_warning() {
        let tmp = tempdir().expect("tempdir");
        let agents_path = tmp.path().join("AGENTS.md");
        fs::write(&agents_path, "   \n  \n  ").expect("write"); // Only whitespace

        let ctx = load_project_context(tmp.path());

        assert!(!ctx.has_instructions());
        assert!(!ctx.warnings.is_empty());
    }

    #[test]
    fn test_check_trust_status() {
        let tmp = tempdir().expect("tempdir");

        // Not trusted by default
        assert!(!check_trust_status(tmp.path()));

        // Create trust marker
        let deepseek_dir = tmp.path().join(".deepseek");
        fs::create_dir(&deepseek_dir).expect("mkdir");
        fs::write(deepseek_dir.join("trusted"), "").expect("write");

        assert!(check_trust_status(tmp.path()));
    }

    #[test]
    fn test_create_default_agents_md() {
        let tmp = tempdir().expect("tempdir");
        let path = create_default_agents_md(tmp.path()).expect("create");

        assert!(path.exists());
        let content = fs::read_to_string(&path).expect("read");
        assert!(content.contains("Project Agent Instructions"));
    }

    #[test]
    fn test_load_with_parents() {
        let tmp = tempdir().expect("tempdir");

        // Create a nested structure
        let subdir = tmp.path().join("subproject");
        fs::create_dir(&subdir).expect("mkdir");

        // Put AGENTS.md in parent
        fs::write(tmp.path().join("AGENTS.md"), "Parent instructions").expect("write");
        // Also create .git to mark as repo root
        fs::create_dir(tmp.path().join(".git")).expect("mkdir .git");

        // Load from subdir should find parent's AGENTS.md
        let ctx = load_project_context_with_parents(&subdir);

        assert!(ctx.has_instructions());
        assert!(
            ctx.instructions
                .as_ref()
                .unwrap()
                .contains("Parent instructions")
        );
    }

    #[test]
    fn test_merge_contexts() {
        let mut ctx1 = ProjectContext::empty(PathBuf::from("/a"));
        ctx1.instructions = Some("Instructions A".to_string());
        ctx1.source_path = Some(PathBuf::from("/a/AGENTS.md"));

        let mut ctx2 = ProjectContext::empty(PathBuf::from("/b"));
        ctx2.instructions = Some("Instructions B".to_string());
        ctx2.source_path = Some(PathBuf::from("/b/AGENTS.md"));

        let merged = merge_contexts(&[ctx1, ctx2]).expect("merge");

        assert!(merged.contains("Instructions A"));
        assert!(merged.contains("Instructions B"));
    }

    #[test]
    fn test_load_with_parents_searches_above_git_root_when_needed() {
        let tmp = tempdir().expect("tempdir");

        // AGENTS.md exists above repository root.
        fs::write(tmp.path().join("AGENTS.md"), "Organization instructions").expect("write");

        // Mark repository root one level below.
        let repo_root = tmp.path().join("repo");
        fs::create_dir(&repo_root).expect("mkdir repo");
        fs::create_dir(repo_root.join(".git")).expect("mkdir .git");

        let workspace = repo_root.join("apps").join("client");
        fs::create_dir_all(&workspace).expect("mkdir workspace");

        let ctx = load_project_context_with_parents(&workspace);
        assert!(ctx.has_instructions());
        assert!(
            ctx.instructions
                .as_ref()
                .unwrap()
                .contains("Organization instructions")
        );
    }

    #[test]
    fn project_context_pack_is_stable_and_sorted() {
        let tmp = tempdir().expect("tempdir");
        fs::write(tmp.path().join("README.md"), "# Demo\n\nReadme body").expect("write");
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"demo\"").expect("write");
        fs::create_dir_all(tmp.path().join("src")).expect("mkdir src");
        fs::write(tmp.path().join("src").join("z.rs"), "mod z;").expect("write z");
        fs::write(tmp.path().join("src").join("a.rs"), "mod a;").expect("write a");
        fs::create_dir_all(tmp.path().join("node_modules").join("pkg")).expect("mkdir ignored");
        fs::write(
            tmp.path().join("node_modules").join("pkg").join("index.js"),
            "ignored",
        )
        .expect("write ignored");

        let first = generate_project_context_pack(tmp.path()).expect("pack");
        let second = generate_project_context_pack(tmp.path()).expect("pack again");

        assert_eq!(first, second);
        assert!(first.contains("\"project_name\""));
        assert!(first.contains("\"directory_structure\""));
        assert!(first.contains("\"README.md\""));
        assert!(first.contains("\"Cargo.toml\""));
        assert!(first.contains("\"src/a.rs\""));
        assert!(first.contains("\"src/z.rs\""));
        assert!(!first.contains("node_modules"));
        assert!(
            first.find("\"src/a.rs\"").expect("a before z")
                < first.find("\"src/z.rs\"").expect("z")
        );
    }

    #[test]
    fn project_context_pack_ignores_agent_state_and_binary_noise() {
        let tmp = tempdir().expect("tempdir");
        fs::create_dir_all(tmp.path().join("src")).expect("mkdir src");
        fs::write(tmp.path().join("src").join("main.rs"), "fn main() {}").expect("write src");
        fs::write(tmp.path().join(".DS_Store"), "noise").expect("write ds store");
        fs::write(tmp.path().join("paper.pdf"), "not a real pdf").expect("write pdf");
        fs::create_dir_all(tmp.path().join(".deepseek").join("state")).expect("mkdir state");
        fs::write(
            tmp.path()
                .join(".deepseek")
                .join("state")
                .join("subagents.v1.json"),
            "{}",
        )
        .expect("write state");
        fs::create_dir_all(tmp.path().join(".playwright-mcp")).expect("mkdir playwright");
        fs::write(
            tmp.path().join(".playwright-mcp").join("trace.log"),
            "noise",
        )
        .expect("write log");
        fs::create_dir_all(tmp.path().join(".agents").join("skills").join("demo"))
            .expect("mkdir skills");
        fs::write(
            tmp.path()
                .join(".agents")
                .join("skills")
                .join("demo")
                .join("SKILL.md"),
            "skill body",
        )
        .expect("write skill");
        fs::create_dir_all(tmp.path().join(".github").join("workflows")).expect("mkdir workflows");
        fs::write(
            tmp.path().join(".github").join("workflows").join("ci.yml"),
            "name: ci",
        )
        .expect("write workflow");

        let pack = generate_project_context_pack(tmp.path()).expect("pack");

        assert!(pack.contains("\"src/main.rs\""), "{pack}");
        assert!(pack.contains("\".github/\""), "{pack}");
        assert!(pack.contains("\".github/workflows/ci.yml\""), "{pack}");
        assert!(!pack.contains(".deepseek"), "{pack}");
        assert!(!pack.contains(".playwright-mcp"), "{pack}");
        assert!(!pack.contains(".agents"), "{pack}");
        assert!(!pack.contains(".DS_Store"), "{pack}");
        assert!(!pack.contains("paper.pdf"), "{pack}");
        assert!(!pack.contains("trace.log"), "{pack}");
    }

    #[test]
    fn project_context_pack_keeps_later_top_level_dirs_under_budget() {
        let tmp = tempdir().expect("tempdir");
        let noisy = tmp.path().join("aaa-many-files");
        fs::create_dir_all(&noisy).expect("mkdir noisy");
        for i in 0..(PACK_MAX_ENTRIES + 20) {
            fs::write(noisy.join(format!("file-{i:03}.rs")), "fn f() {}").expect("write noisy");
        }
        fs::create_dir_all(tmp.path().join("zzz-important")).expect("mkdir important");
        fs::write(
            tmp.path().join("zzz-important").join("main.rs"),
            "fn important() {}",
        )
        .expect("write important");

        let pack = generate_project_context_pack(tmp.path()).expect("pack");

        assert!(
            pack.contains("\"zzz-important/\""),
            "breadth-first packing should keep later top-level directories visible:\n{pack}"
        );
    }

    #[test]
    fn test_load_global_agents_when_project_has_no_context() {
        let workspace = tempdir().expect("workspace tempdir");
        let home = tempdir().expect("home tempdir");
        let global_dir = home.path().join(".deepseek");
        fs::create_dir(&global_dir).expect("mkdir .deepseek");
        let global_agents = global_dir.join("AGENTS.md");
        fs::write(&global_agents, "Global instructions").expect("write global agents");

        let ctx = load_project_context_with_parents_and_home(workspace.path(), Some(home.path()));

        assert!(ctx.has_instructions());
        assert!(
            ctx.instructions
                .as_ref()
                .unwrap()
                .contains("Global instructions")
        );
        assert_eq!(ctx.source_path, Some(global_agents));
    }

    #[test]
    fn test_local_and_global_agents_merge_when_both_exist() {
        // #1157: when both `~/.deepseek/AGENTS.md` and a project AGENTS.md
        // exist, the prompt should carry user-wide preferences AND the
        // project's overrides — not silently drop the global file.
        let workspace = tempdir().expect("workspace tempdir");
        fs::write(workspace.path().join("AGENTS.md"), "Local instructions")
            .expect("write local agents");

        let home = tempdir().expect("home tempdir");
        let global_dir = home.path().join(".deepseek");
        fs::create_dir(&global_dir).expect("mkdir .deepseek");
        fs::write(global_dir.join("AGENTS.md"), "Global instructions")
            .expect("write global agents");

        let ctx = load_project_context_with_parents_and_home(workspace.path(), Some(home.path()));

        assert!(ctx.has_instructions());
        let instructions = ctx.instructions.as_ref().unwrap();
        assert!(
            instructions.contains("Global instructions"),
            "global block missing from merged instructions:\n{instructions}"
        );
        assert!(
            instructions.contains("Local instructions"),
            "project block missing from merged instructions:\n{instructions}"
        );
        // Global block precedes the project block so project rules read
        // last and win "last word" precedence with the model.
        let global_at = instructions.find("Global instructions").unwrap();
        let local_at = instructions.find("Local instructions").unwrap();
        assert!(
            global_at < local_at,
            "global block must come before project block, got global={global_at} local={local_at}"
        );
        // The merged block is labelled so the model can tell the layers
        // apart when it needs to explain which rule it followed.
        assert!(
            instructions.contains("project (overrides global where they conflict)"),
            "expected labelled separator between global and project blocks"
        );
        // `source_path` keeps pointing at the more-specific file so the
        // user knows where to edit the workspace-level override.
        assert_eq!(ctx.source_path, Some(workspace.path().join("AGENTS.md")));
    }

    #[test]
    fn test_global_agents_only_no_project_unchanged_fallback() {
        // Sanity: when only the global file exists, the historical
        // fallback behaviour is preserved — no merge framing leaks in.
        let workspace = tempdir().expect("workspace tempdir");
        let home = tempdir().expect("home tempdir");
        let global_dir = home.path().join(".deepseek");
        fs::create_dir(&global_dir).expect("mkdir .deepseek");
        let global_agents = global_dir.join("AGENTS.md");
        fs::write(&global_agents, "Just the global instructions").expect("write global agents");

        let ctx = load_project_context_with_parents_and_home(workspace.path(), Some(home.path()));

        assert!(ctx.has_instructions());
        let instructions = ctx.instructions.as_ref().unwrap();
        assert!(instructions.contains("Just the global instructions"));
        assert!(
            !instructions.contains("project (overrides global"),
            "merge-framing label should not appear when there's nothing to merge"
        );
        assert_eq!(ctx.source_path, Some(global_agents));
    }

    #[test]
    fn test_invalid_global_agents_warns_and_falls_back_to_generated_context() {
        let workspace = tempdir().expect("workspace tempdir");
        let home = tempdir().expect("home tempdir");
        let global_dir = home.path().join(".deepseek");
        fs::create_dir(&global_dir).expect("mkdir .deepseek");
        fs::write(global_dir.join("AGENTS.md"), "   \n  ").expect("write empty global agents");

        let ctx = load_project_context_with_parents_and_home(workspace.path(), Some(home.path()));

        assert!(
            ctx.warnings
                .iter()
                .any(|warning| warning.contains("Context file") && warning.contains("is empty")),
            "expected empty global AGENTS.md warning, got {:?}",
            ctx.warnings
        );
        assert!(ctx.has_instructions());
        assert!(
            ctx.instructions
                .as_ref()
                .unwrap()
                .contains("Project Structure (Auto-generated)")
        );
    }
}
