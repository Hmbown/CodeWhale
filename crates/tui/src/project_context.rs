//! Project context loading for CodeWhale.
//!
//! This module handles loading project-specific context files that provide
//! instructions and context to the AI agent. These include:
//!
//! - `AGENTS.md` - Cross-agent project instructions (canonical, highest priority)
//! - `.claude/instructions.md` - Claude-style hidden instructions (compat)
//! - `CLAUDE.md` - Claude-style instructions (compat)
//! - `.codewhale/instructions.md` - Hidden instructions file (compat)
//! - `.deepseek/instructions.md` - Hidden instructions file (legacy)
//!
//! CodeWhale-specific repo authority/prioritization policy lives separately in
//! `.codewhale/constitution.json` and is rendered as its own higher-authority
//! block. The loaded content is injected into the system prompt to give the
//! agent context about the project's conventions, structure, and requirements.

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Names of project context files to look for, in priority order.
///
/// `AGENTS.md` is the canonical cross-agent project-instructions file.
/// `WHALE.md` is no longer an active context surface; when present, CodeWhale
/// reports a migration warning but ignores it. CodeWhale-specific repo
/// authority now lives in `.codewhale/constitution.json`, not a bespoke
/// markdown file. `CLAUDE.md` and the `*/instructions.md` variants are
/// read-only compatibility fallbacks; CodeWhale never creates or recommends
/// them.
const PROJECT_CONTEXT_FILES: &[&str] = &[
    "AGENTS.md",
    ".claude/instructions.md",
    "CLAUDE.md",
    ".codewhale/instructions.md",
    ".deepseek/instructions.md",
];

/// Rules directories auto-discovered at workspace level, in priority order.
/// `.codewhale/rules/` is CodeWhale-native; `.claude/rules/` is Claude compatibility.
/// All `.md` files in these directories are loaded as project rules in filename order.
/// Security model: same trust class as AGENTS.md — workspace-contained content only,
/// no absolute-path escape. Does not require #417 project-config relaxation.
const RULES_DIRS: &[&str] = &[".codewhale/rules", ".claude/rules"];

/// File name of the deprecated CodeWhale-native instructions file.
const DEPRECATED_WHALE_FILENAME: &str = "WHALE.md";

/// Warning surfaced when an ignored `WHALE.md` is present.
const WHALE_IGNORED_WARNING: &str = "WHALE.md is ignored; move project instructions to AGENTS.md, or CodeWhale-specific authority policy to .codewhale/constitution.json.";

/// Relative path (within a workspace or one of its parents) to the
/// CodeWhale-specific repo authority/prioritization policy.
const REPO_CONSTITUTION_RELATIVE_PATH: &[&str] = &[".codewhale", "constitution.json"];

/// `schema_version` understood by this build of the constitution loader.
const SUPPORTED_CONSTITUTION_SCHEMA: u32 = 1;

/// User-level project instructions loaded as a fallback when the workspace and
/// its parents do not define project context. Any global AGENTS.md takes
/// priority over a global instructions.md (#3012). Within each file name,
/// `.codewhale/` takes priority over vendor-neutral `.agents/`, which takes
/// priority over legacy `.deepseek/`. Global `WHALE.md` files are ignored and
/// reported as migration-only diagnostics.
const GLOBAL_AGENTS_RELATIVE_PATH: &[&str] = &[".codewhale", "AGENTS.md"];
const GLOBAL_AGENTS_VENDOR_NEUTRAL_PATH: &[&str] = &[".agents", "AGENTS.md"];
const GLOBAL_AGENTS_LEGACY_PATH: &[&str] = &[".deepseek", "AGENTS.md"];
const GLOBAL_WHALE_RELATIVE_PATH: &[&str] = &[".codewhale", "WHALE.md"];
const GLOBAL_WHALE_VENDOR_NEUTRAL_PATH: &[&str] = &[".agents", "WHALE.md"];
const GLOBAL_WHALE_LEGACY_PATH: &[&str] = &[".deepseek", "WHALE.md"];
/// Global `instructions.md` (#3012): auto-loaded as a fallback context layer,
/// ranked below AGENTS.md, mirroring the project-level precedence.
const GLOBAL_INSTRUCTIONS_RELATIVE_PATH: &[&str] = &[".codewhale", "instructions.md"];
const GLOBAL_INSTRUCTIONS_VENDOR_NEUTRAL_PATH: &[&str] = &[".agents", "instructions.md"];
const GLOBAL_INSTRUCTIONS_LEGACY_PATH: &[&str] = &[".deepseek", "instructions.md"];

/// Maximum size for project context files (to prevent loading huge files)
const MAX_CONTEXT_SIZE: usize = 100 * 1024; // 100KB

/// Maximum number of rule files loaded per rules directory.
/// Prevents a project from silently injecting hundreds of rule files.
const MAX_RULES_FILES: usize = 50;

/// Maximum total bytes across the assembled rules_block.
/// 50 files × 100 KB per file could reach ~5 MB; this caps the
/// cumulative injected content so a large rules directory can't
/// dominate the context window. Exceeded bytes are truncated with
/// an explicit marker.
const MAX_RULES_BLOCK_BYTES: usize = 500 * 1024; // 500 KB
const PACK_README_MAX_CHARS: usize = 4_000;
const PACK_MAX_ENTRIES: usize = 220;
const PACK_MAX_SOURCE_FILES: usize = 60;
const PACK_MAX_CONFIG_FILES: usize = 60;
const PACK_MAX_DEPTH: usize = 4;
const PACK_IGNORED_DIRS: &[&str] = &[
    ".git",
    ".worktrees",
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
    #[error("Refusing symlinked context file {path}")]
    Symlink { path: PathBuf },
    #[error("Context path {path} is not a regular file")]
    NotFile { path: PathBuf },
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
    /// Auto-discovered rules from `.codewhale/rules/` / `.claude/rules/`.
    /// Kept separate from `instructions` so rules alone don't block
    /// parent-directory AGENTS.md discovery via `has_instructions()`.
    pub rules_block: Option<String>,
    /// Path to the loaded file (for display)
    pub source_path: Option<PathBuf>,
    /// Any warnings during loading
    pub warnings: Vec<String>,
    /// Rendered `.codewhale/constitution.json` authority block, if present.
    /// CodeWhale-specific repo authority/prioritization policy — distinct from
    /// the cross-agent prose in `instructions`.
    pub constitution_block: Option<String>,
    /// Path to the repo constitution file that produced `constitution_block`.
    pub constitution_source_path: Option<PathBuf>,
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
            rules_block: None,
            source_path: None,
            warnings: Vec::new(),
            constitution_block: None,
            constitution_source_path: None,
            project_root,
            is_trusted: false,
        }
    }

    /// Check if any instructions were loaded
    pub fn has_instructions(&self) -> bool {
        self.instructions.is_some()
    }

    /// Get the instructions as a formatted block for system prompt.
    ///
    /// The CodeWhale repo constitution (`.codewhale/constitution.json`), when
    /// present, is emitted first as a higher-authority block, followed by the
    /// cross-agent `<project_instructions>` prose. Either may be absent.
    pub fn as_system_block(&self) -> Option<String> {
        let instructions_block = self.instructions.as_ref().map(|content| {
            let source = self
                .source_path
                .as_ref()
                .map_or_else(|| "project".to_string(), |p| p.display().to_string());

            let mut block = format!(
                "<project_instructions source=\"{source}\">\n{content}\n</project_instructions>"
            );
            // Append rules after instructions, inside the same logical block.
            // Rules are kept separate from `instructions` so they don't block
            // parent-directory AGENTS.md discovery via `has_instructions()`.
            if let Some(rules) = &self.rules_block {
                block.push('\n');
                block.push_str(rules);
            }
            block
        });

        match (self.constitution_block.as_ref(), instructions_block) {
            (Some(constitution), Some(instructions)) => {
                Some(format!("{constitution}\n\n{instructions}"))
            }
            (Some(constitution), None) => {
                // Constitution present but no main instructions — still emit rules if any
                if let Some(rules) = &self.rules_block {
                    Some(format!("{constitution}\n\n{rules}"))
                } else {
                    Some(constitution.clone())
                }
            }
            (None, Some(instructions)) => Some(instructions),
            (None, None) => {
                // No main instructions, but rules may exist on their own
                self.rules_block.clone()
            }
        }
    }
}

/// CodeWhale-specific repo authority/prioritization policy, loaded from
/// `.codewhale/constitution.json`. All fields are optional so a minimal file
/// (or a future schema) still parses; unknown fields are ignored.
#[derive(Debug, Clone, Default, Deserialize)]
struct RepoConstitution {
    #[serde(default)]
    schema_version: Option<u32>,
    /// Ordered list of sources to trust when local sources conflict
    /// (highest authority first).
    #[serde(default)]
    authority: Option<Vec<String>>,
    /// Repo invariants the agent must not break. Plain strings are advisory
    /// prose (rendered into the prompt only); object entries with `paths`
    /// are additionally compiled into mechanical write holds (see
    /// `crate::repo_law`). Law can only tighten — there is no allow shape.
    #[serde(default)]
    protected_invariants: Option<Vec<ProtectedInvariant>>,
    /// Branch / release policy in effect (e.g. "PRs target codex/v0.8.53").
    #[serde(default)]
    branch_policy: Option<String>,
    /// Conditions under which the agent should stop and escalate to the user.
    #[serde(default)]
    escalate_when: Option<Vec<String>>,
    #[serde(default)]
    verification_policy: Option<VerificationPolicy>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct VerificationPolicy {
    /// Steps to perform before claiming a task is done.
    #[serde(default)]
    before_claiming_done: Option<Vec<String>>,
}

/// One protected invariant: either advisory prose (the historical shape) or
/// an enforced entry carrying path globs. Untagged so existing files keep
/// parsing unchanged.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum ProtectedInvariant {
    Advisory(String),
    Enforced(EnforcedInvariant),
}

#[derive(Debug, Clone, Deserialize)]
struct EnforcedInvariant {
    text: String,
    /// Workspace-relative path globs this invariant protects (e.g.
    /// `crates/protocol/**`). Empty means advisory-only despite the shape.
    #[serde(default)]
    paths: Vec<String>,
    /// What the harness does when a write targets a protected path.
    #[serde(default)]
    action: RepoLawAction,
}

/// Enforcement level for a protected path. `Ask` force-prompts (in every
/// mode, including YOLO — law can add holds, never remove them); `Block`
/// denies outright.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RepoLawAction {
    #[default]
    Ask,
    Block,
}

/// A compiled, mechanically-enforceable repo-law rule.
pub(crate) struct RepoLawRule {
    pub(crate) text: String,
    pub(crate) patterns: Vec<String>,
    pub(crate) globs: globset::GlobSet,
    pub(crate) action: RepoLawAction,
}

/// Load and compile the enforceable rules from the workspace's repo
/// constitution. Any failure — missing file, parse error, invalid glob —
/// degrades to fewer (or zero) rules: enforcement can silently do less,
/// never more, and never poisons the tool gate. Parse warnings still reach
/// the user through the prompt-side load path, which reads the same file.
pub(crate) fn load_repo_law_rules(workspace: &Path) -> Vec<RepoLawRule> {
    let Some((_, constitution)) = discover_repo_constitution(workspace) else {
        return Vec::new();
    };
    let mut rules = Vec::new();
    for invariant in constitution.protected_invariants.into_iter().flatten() {
        let ProtectedInvariant::Enforced(enforced) = invariant else {
            continue;
        };
        if enforced.text.trim().is_empty() {
            continue;
        }
        let mut builder = globset::GlobSetBuilder::new();
        let mut patterns = Vec::new();
        for pattern in &enforced.paths {
            let trimmed = pattern.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(glob) = globset::Glob::new(trimmed) {
                builder.add(glob);
                patterns.push(trimmed.to_string());
            }
        }
        if patterns.is_empty() {
            continue;
        }
        let Ok(globs) = builder.build() else {
            continue;
        };
        rules.push(RepoLawRule {
            text: enforced.text.trim().to_string(),
            patterns,
            globs,
            action: enforced.action,
        });
    }
    rules
}

/// Walk from `workspace` toward the git root looking for the repo
/// constitution; parse best-effort. Shared by the enforcement loader; the
/// prompt-side loader keeps its richer warning handling.
fn discover_repo_constitution(workspace: &Path) -> Option<(PathBuf, RepoConstitution)> {
    let git_root = find_git_root(workspace);
    let mut current = workspace.to_path_buf();
    loop {
        let mut path = current.clone();
        for component in REPO_CONSTITUTION_RELATIVE_PATH {
            path.push(component);
        }
        if context_candidate_exists(&path) {
            let constitution = load_context_file(&path)
                .ok()
                .and_then(|raw| serde_json::from_str::<RepoConstitution>(&raw).ok())?;
            return Some((path, constitution));
        }
        if let Some(ref root) = git_root
            && current == *root
        {
            break;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => break,
        }
    }
    None
}

impl RepoConstitution {
    /// True when the file carried no usable policy (so we can skip emitting an
    /// empty block).
    fn is_empty(&self) -> bool {
        let list_empty = |l: &Option<Vec<String>>| l.as_ref().is_none_or(Vec::is_empty);
        list_empty(&self.authority)
            && self.protected_invariants.as_ref().is_none_or(Vec::is_empty)
            && list_empty(&self.escalate_when)
            && self
                .branch_policy
                .as_ref()
                .is_none_or(|s| s.trim().is_empty())
            && self
                .verification_policy
                .as_ref()
                .and_then(|p| p.before_claiming_done.as_ref())
                .is_none_or(Vec::is_empty)
    }

    /// Render a model-facing authority block (concise prose, per the layered
    /// model: base myth → global constitution → repo constitution = local law).
    fn render_block(&self, source: &Path) -> String {
        let mut body = String::new();
        if let Some(authority) = self.authority.as_ref().filter(|a| !a.is_empty()) {
            body.push_str(
                "When local sources conflict, trust them in this order (highest first):\n",
            );
            for (idx, item) in authority.iter().enumerate() {
                body.push_str(&format!("{}. {item}\n", idx + 1));
            }
        }
        if let Some(invariants) = self.protected_invariants.as_ref().filter(|i| !i.is_empty()) {
            body.push_str("\nProtected invariants — do not break:\n");
            for item in invariants {
                match item {
                    ProtectedInvariant::Advisory(text) => {
                        body.push_str(&format!("- {text}\n"));
                    }
                    ProtectedInvariant::Enforced(enforced) => {
                        let paths = enforced
                            .paths
                            .iter()
                            .map(String::as_str)
                            .collect::<Vec<_>>()
                            .join(", ");
                        if paths.is_empty() {
                            body.push_str(&format!("- {}\n", enforced.text));
                        } else {
                            body.push_str(&format!(
                                "- {} (mechanically enforced for: {paths})\n",
                                enforced.text
                            ));
                        }
                    }
                }
            }
        }
        if let Some(policy) = self.branch_policy.as_ref().filter(|s| !s.trim().is_empty()) {
            body.push_str(&format!("\nBranch / release policy: {}\n", policy.trim()));
        }
        if let Some(steps) = self
            .verification_policy
            .as_ref()
            .and_then(|p| p.before_claiming_done.as_ref())
            .filter(|s| !s.is_empty())
        {
            body.push_str("\nBefore claiming a task is done:\n");
            for step in steps {
                body.push_str(&format!("- {step}\n"));
            }
        }
        if let Some(conditions) = self.escalate_when.as_ref().filter(|c| !c.is_empty()) {
            body.push_str("\nStop and escalate to the user when:\n");
            for item in conditions {
                body.push_str(&format!("- {item}\n"));
            }
        }
        format!(
            "<codewhale_repo_constitution source=\"{}\">\nCodeWhale-specific repo authority policy (local law: subordinate to the global Constitution and the current user request, but above memory and old handoffs; WHALE.md is ignored and should be migrated, not treated as law).\n\n{}</codewhale_repo_constitution>",
            source.display(),
            body.trim_end()
        )
    }

    fn policy_warnings(&self, source: &Path) -> Vec<String> {
        let mut warnings = Vec::new();
        if let Some(policy) = self.branch_policy.as_deref()
            && branch_policy_looks_stale(policy)
        {
            warnings.push(format!(
                "{} branch_policy appears stale: hard-coded release branch guidance (`{}`). Use live branch/handoff truth and AGENTS.md instead of versioned integration-lane text.",
                source.display(),
                policy.trim()
            ));
        }
        warnings
    }
}

fn branch_policy_looks_stale(policy: &str) -> bool {
    let lower = policy.to_ascii_lowercase();
    lower.contains("codex/v")
        || ((lower.contains("integration branch") || lower.contains("not main"))
            && contains_release_version_token(policy))
}

fn contains_release_version_token(value: &str) -> bool {
    value
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '.'))
        .any(|token| {
            let token = token.trim_start_matches(['v', 'V']);
            let mut parts = token.split('.');
            matches!(
                (parts.next(), parts.next(), parts.next(), parts.next()),
                (Some(major), Some(minor), Some(patch), None)
                    if major.chars().all(|ch| ch.is_ascii_digit())
                        && minor.chars().all(|ch| ch.is_ascii_digit())
                        && patch.chars().all(|ch| ch.is_ascii_digit())
            )
        })
}

/// Discover and render `.codewhale/constitution.json` from `workspace` or, if
/// absent, its parent directories up to the git root. Returns the rendered
/// authority block plus any parse warnings.
fn load_repo_constitution_block(
    workspace: &Path,
) -> (Option<String>, Option<PathBuf>, Vec<String>) {
    let mut warnings = Vec::new();
    let git_root = find_git_root(workspace);
    let mut current = workspace.to_path_buf();
    loop {
        let mut path = current.clone();
        for component in REPO_CONSTITUTION_RELATIVE_PATH {
            path.push(component);
        }
        if context_candidate_exists(&path) {
            match load_context_file(&path) {
                Ok(raw) => match serde_json::from_str::<RepoConstitution>(&raw) {
                    Ok(constitution) if !constitution.is_empty() => {
                        if let Some(version) = constitution.schema_version
                            && version != SUPPORTED_CONSTITUTION_SCHEMA
                        {
                            warnings.push(format!(
                                "{} declares schema_version {version}; this build supports {SUPPORTED_CONSTITUTION_SCHEMA}. Reading it on a best-effort basis.",
                                path.display()
                            ));
                        }
                        warnings.extend(constitution.policy_warnings(&path));
                        return (Some(constitution.render_block(&path)), Some(path), warnings);
                    }
                    Ok(_) => {
                        warnings.push(format!(
                            "{} has no authority/verification policy; ignoring.",
                            path.display()
                        ));
                        return (None, None, warnings);
                    }
                    Err(e) => {
                        warnings.push(format!("Failed to parse {}: {e}", path.display()));
                        return (None, None, warnings);
                    }
                },
                Err(e) => {
                    warnings.push(format!("Failed to read {}: {e}", path.display()));
                    return (None, None, warnings);
                }
            }
        }
        if let Some(ref root) = git_root
            && current == *root
        {
            break;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => break,
        }
    }
    (None, None, warnings)
}

/// Discover `.codewhale/constitution.json` for read-only status display.
///
/// Uses the same walk and parse rules as prompt assembly; returns the source
/// path only when the file carries usable policy.
#[must_use]
pub fn discover_repo_constitution_path(workspace: &Path) -> Option<PathBuf> {
    let (_block, source_path, _warnings) = load_repo_constitution_block(workspace);
    source_path
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
    let pack = build_project_context_pack(workspace)?;
    let json = serde_json::to_string_pretty(&pack).ok()?;
    Some(format!(
        "## Project Context Pack\n\n<project_context_pack>\n{json}\n</project_context_pack>"
    ))
}

fn generate_bounded_project_overview(workspace: &Path) -> Option<String> {
    let pack = build_project_context_pack(workspace)?;
    let json = serde_json::to_string_pretty(&pack).ok()?;
    Some(format!(
        "## Bounded Project Overview\n\n```json\n{json}\n```"
    ))
}

fn build_project_context_pack(workspace: &Path) -> Option<ProjectContextPack> {
    let mut entries = Vec::new();
    collect_pack_entries(workspace, workspace, 0, &mut entries);
    sort_pack_paths(&mut entries);
    entries.truncate(PACK_MAX_ENTRIES);

    let mut config_files = entries
        .iter()
        .filter(|path| is_config_file(path))
        .take(PACK_MAX_CONFIG_FILES)
        .cloned()
        .collect::<Vec<_>>();
    sort_pack_paths(&mut config_files);

    let mut key_source_files = entries
        .iter()
        .filter(|path| is_source_file(path))
        .take(PACK_MAX_SOURCE_FILES)
        .cloned()
        .collect::<Vec<_>>();
    sort_pack_paths(&mut key_source_files);

    let readme = read_readme_excerpt(workspace, &entries);
    let mut counts = BTreeMap::new();
    counts.insert("config_files".to_string(), config_files.len());
    counts.insert("directory_entries".to_string(), entries.len());
    counts.insert("key_source_files".to_string(), key_source_files.len());

    Some(ProjectContextPack {
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
    })
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
    normalize_pack_relative_path(&parts.join("/"))
}

fn normalize_pack_relative_path(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    let mut parts = Vec::new();
    for part in normalized.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return None;
        }
        parts.push(part);
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn sort_pack_paths(paths: &mut [String]) {
    paths.sort_by(|a, b| {
        pack_path_priority(a)
            .cmp(&pack_path_priority(b))
            .then_with(|| pack_path_sort_key(a).cmp(&pack_path_sort_key(b)))
            .then_with(|| a.cmp(b))
    });
}

fn pack_path_sort_key(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}

fn pack_path_priority(path: &str) -> u8 {
    let lower = pack_path_sort_key(path);
    let name = lower.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    if matches!(name, "readme.md" | "readme.txt" | "readme") {
        0
    } else if is_config_file(&lower) {
        1
    } else if is_source_file(&lower) {
        2
    } else if lower.ends_with('/') {
        3
    } else {
        4
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

    // Search for active project context files.
    for filename in PROJECT_CONTEXT_FILES {
        let file_path = workspace.join(filename);

        if context_candidate_exists(&file_path) {
            match load_context_file(&file_path) {
                Ok(content) => {
                    tracing::info!(
                        "Loaded project context from {} ({} bytes)",
                        file_path.display(),
                        content.len()
                    );
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

    ctx.warnings
        .extend(ignored_project_whale_warnings(workspace));

    // Load rules from auto-discovered directories (.codewhale/rules/, .claude/rules/)
    // Each rule file is wrapped in a <project_rule> block and appended after
    // the main instructions content. Security model: same as AGENTS.md —
    // workspace-contained content only, no absolute-path escape.
    let mut rules_content = String::new();
    for rules_dir in RULES_DIRS {
        let rules = load_rules_from_dir(workspace, rules_dir);
        for (path, content) in rules {
            if !rules_content.is_empty() {
                rules_content.push('\n');
            }
            rules_content.push_str(&format!(
                "<project_rule source=\"{}\">\n{}\n</project_rule>",
                path.display(),
                content.trim()
            ));
        }
    }

    if !rules_content.is_empty() {
        // Cap total rules bytes so a large rules dir can't dominate the context window
        if rules_content.len() > MAX_RULES_BLOCK_BYTES {
            let mut end = MAX_RULES_BLOCK_BYTES;
            while !rules_content.is_char_boundary(end) {
                end -= 1;
            }
            rules_content.truncate(end);
            rules_content.push_str("\n\n[…rules block truncated at 500 KB…]");
            tracing::warn!(
                target: "project_context",
                total_bytes = rules_content.len(),
                cap = MAX_RULES_BLOCK_BYTES,
                "Truncating rules block to total byte budget"
            );
        }
        ctx.rules_block = Some(rules_content);
    }

    // Check for trust file
    ctx.is_trusted = check_trust_status(workspace);

    ctx
}

/// Load project context from parent directories as well.
///
/// This allows for monorepo setups where a root AGENTS.md applies to all subdirectories.
pub fn load_project_context_with_parents(workspace: &Path) -> ProjectContext {
    load_project_context_with_parents_cached_and_home(workspace, dirs::home_dir().as_deref())
}

fn load_project_context_with_parents_cached_and_home(
    workspace: &Path,
    home_dir: Option<&Path>,
) -> ProjectContext {
    let workspace = canonicalize_workspace_or_keep(workspace);
    let pre_load_key = crate::project_context_cache::compute_cache_key(&workspace, home_dir);
    if let Some(ctx) = crate::project_context_cache::lookup(&pre_load_key) {
        return ctx;
    }

    let ctx = load_project_context_with_parents_and_home(&workspace, home_dir);
    let post_load_key = crate::project_context_cache::compute_cache_key(&workspace, home_dir);
    crate::project_context_cache::store(post_load_key, ctx.clone());
    ctx
}

fn load_project_context_with_parents_and_home(
    workspace: &Path,
    home_dir: Option<&Path>,
) -> ProjectContext {
    let workspace_canonical = canonicalize_workspace_or_keep(workspace);
    let mut ctx = load_project_context(workspace);
    let parent_search_stop = project_context_parent_search_stop_dir();

    // If no context found in workspace, check parent directories
    if !ctx.has_instructions() {
        let mut current = workspace_canonical.parent();

        while let Some(parent) = current {
            if parent_search_stop
                .as_deref()
                .is_some_and(|stop| parent == stop)
            {
                break;
            }

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

    // Always check global instruction files so user-wide preferences
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

    // Generate a bounded in-memory fallback when no context file exists
    // anywhere. This keeps prompt shape stable without creating project-local
    // `.codewhale/` files merely because CodeWhale was opened in a directory.
    if !ctx.has_instructions()
        && let Some(generated) = generate_ephemeral_context(workspace)
    {
        ctx.instructions = Some(generated);
        ctx.source_path = None;
    }

    // Load the CodeWhale-specific repo authority policy
    // (.codewhale/constitution.json) independently of the prose instructions —
    // it is a distinct, higher-authority artifact and may exist with or without
    // an AGENTS.md. Legacy WHALE.md files are ignored and reported as
    // migration-only diagnostics.
    // Loaded last so the auto-generate fallback above (which rebuilds `ctx`)
    // cannot clobber it.
    let (constitution_block, constitution_source_path, constitution_warnings) =
        load_repo_constitution_block(workspace);
    ctx.warnings.extend(constitution_warnings);
    ctx.constitution_block = constitution_block;
    ctx.constitution_source_path = constitution_source_path;

    ctx
}

pub(crate) fn project_context_cache_candidate_paths(
    workspace: &Path,
    home_dir: Option<&Path>,
) -> Vec<PathBuf> {
    let workspace = canonicalize_workspace_or_keep(workspace);
    let mut paths = Vec::new();
    let parent_search_stop = project_context_parent_search_stop_dir();

    let mut current = Some(workspace.as_path());
    while let Some(dir) = current {
        if parent_search_stop
            .as_deref()
            .is_some_and(|stop| dir == stop)
        {
            break;
        }

        for filename in PROJECT_CONTEXT_FILES {
            paths.push(dir.join(filename));
        }
        paths.push(dir.join(DEPRECATED_WHALE_FILENAME));
        current = dir.parent();
    }

    if let Some(home) = home_dir {
        for candidate in global_context_relative_paths() {
            paths.push(join_relative_components(home, candidate));
        }
        for candidate in legacy_global_whale_relative_paths() {
            paths.push(join_relative_components(home, candidate));
        }
    }

    paths.extend(repo_constitution_candidate_paths(&workspace));
    paths.push(workspace.join(".deepseek").join("trusted"));
    paths.push(workspace.join(".deepseek").join("trust.json"));
    paths.extend(crate::config::workspace_trust_config_candidate_paths());

    // Include auto-discovered rules directory files so cache invalidates
    // when rules change (not just when AGENTS.md changes).
    for rules_dir in RULES_DIRS {
        let dir_path = workspace.join(rules_dir);
        // Skip symlinked rules directories (same guard as load_rules_from_dir)
        if fs::symlink_metadata(&dir_path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&dir_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "md") {
                    paths.push(path);
                }
            }
        }
    }

    paths
}

fn repo_constitution_candidate_paths(workspace: &Path) -> Vec<PathBuf> {
    let git_root = find_git_root(workspace);
    let mut current = workspace.to_path_buf();
    let mut paths = Vec::new();
    loop {
        paths.push(join_relative_components(
            &current,
            REPO_CONSTITUTION_RELATIVE_PATH,
        ));
        if let Some(ref root) = git_root
            && current == *root
        {
            break;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => break,
        }
    }
    paths
}

fn global_context_relative_paths() -> [&'static [&'static str]; 6] {
    [
        GLOBAL_AGENTS_RELATIVE_PATH,
        GLOBAL_AGENTS_VENDOR_NEUTRAL_PATH,
        GLOBAL_AGENTS_LEGACY_PATH,
        GLOBAL_INSTRUCTIONS_RELATIVE_PATH,
        GLOBAL_INSTRUCTIONS_VENDOR_NEUTRAL_PATH,
        GLOBAL_INSTRUCTIONS_LEGACY_PATH,
    ]
}

fn legacy_global_whale_relative_paths() -> [&'static [&'static str]; 3] {
    [
        GLOBAL_WHALE_RELATIVE_PATH,
        GLOBAL_WHALE_VENDOR_NEUTRAL_PATH,
        GLOBAL_WHALE_LEGACY_PATH,
    ]
}

fn join_relative_components(base: &Path, relative: &[&str]) -> PathBuf {
    let mut path = base.to_path_buf();
    for component in relative {
        path.push(component);
    }
    path
}

fn ignored_project_whale_warnings(dir: &Path) -> Vec<String> {
    let path = dir.join(DEPRECATED_WHALE_FILENAME);
    ignored_whale_warning_for_path(&path).into_iter().collect()
}

fn ignored_global_whale_warnings(home: &Path) -> Vec<String> {
    legacy_global_whale_relative_paths()
        .iter()
        .filter_map(|candidate| {
            let path = join_relative_components(home, candidate);
            ignored_whale_warning_for_path(&path)
        })
        .collect()
}

fn ignored_whale_warning_for_path(path: &Path) -> Option<String> {
    context_candidate_exists(path)
        .then(|| format!("{WHALE_IGNORED_WARNING} Ignored file: {}", path.display()))
}

fn canonicalize_workspace_or_keep(workspace: &Path) -> PathBuf {
    fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf())
}

fn find_git_root(cwd: &Path) -> Option<PathBuf> {
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

fn project_context_parent_search_stop_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| canonicalize_workspace_or_keep(&home))
}

/// Combine global user-wide preferences with a project-local
/// AGENTS.md/CLAUDE.md/instructions.md. Global comes first so
/// workspace-specific rules can override it — the model reads in declared
/// order. Each block is wrapped in a labelled fence so the model can tell
/// which level any rule comes from when the two sets disagree (#1157).
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

    // Priority order (AGENTS.md preferred; instructions.md next, #3012):
    // 1. ~/.codewhale/AGENTS.md       (canonical)
    // 2. ~/.agents/AGENTS.md          (vendor-neutral fallback)
    // 3. ~/.deepseek/AGENTS.md        (legacy fallback)
    // 4. ~/.codewhale/instructions.md (canonical)
    // 5. ~/.agents/instructions.md    (vendor-neutral fallback)
    // 6. ~/.deepseek/instructions.md  (legacy fallback)
    // Global WHALE.md files are ignored and reported as migration-only
    // diagnostics, never loaded as fallback law.
    let mut warnings = ignored_global_whale_warnings(home);

    for candidate in global_context_relative_paths() {
        let path = join_relative_components(home, candidate);

        if context_candidate_exists(&path) {
            match load_context_file(&path) {
                Ok(content) => {
                    let mut ctx = ProjectContext::empty(workspace.to_path_buf());
                    ctx.instructions = Some(content);
                    ctx.source_path = Some(path);
                    ctx.warnings = warnings;
                    return Some(ctx);
                }
                Err(error) => warnings.push(error.to_string()),
            }
        }
    }

    if !warnings.is_empty() {
        let mut ctx = ProjectContext::empty(workspace.to_path_buf());
        ctx.warnings = warnings;
        return Some(ctx);
    }

    None
}

/// Generate ephemeral context from the project tree. Returns the generated
/// content on success without writing workspace files.
fn generate_ephemeral_context(workspace: &Path) -> Option<String> {
    let overview = generate_bounded_project_overview(workspace)?;

    Some(format!(
        "# Project Context (Auto-generated, ephemeral)\n\n\
         > This context was generated in memory by CodeWhale.\n\
         > No .codewhale/instructions.md file was written.\n\n\
         {overview}"
    ))
}

/// Load a context file with size checking
fn load_context_file(path: &Path) -> Result<String, ProjectContextError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| ProjectContextError::Metadata {
        path: path.to_path_buf(),
        source,
    })?;

    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return Err(ProjectContextError::Symlink {
            path: path.to_path_buf(),
        });
    }

    if !file_type.is_file() {
        return Err(ProjectContextError::NotFile {
            path: path.to_path_buf(),
        });
    }

    let mut file = open_context_file(path)?;
    let metadata = file
        .metadata()
        .map_err(|source| ProjectContextError::Metadata {
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

    let mut content = String::new();
    file.read_to_string(&mut content)
        .map_err(|source| ProjectContextError::Read {
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

fn context_candidate_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        let file_type = metadata.file_type();
        file_type.is_file() || file_type.is_symlink()
    })
}

/// Scan a rules directory for `.md` files and load them in filename order.
/// Missing or unreadable directories return an empty vec (no error).
/// Each file is verified through `load_context_file` (size check, symlink safety).
fn load_rules_from_dir(workspace: &Path, rules_dir_name: &str) -> Vec<(PathBuf, String)> {
    let rules_dir = workspace.join(rules_dir_name);
    let mut entries: Vec<(PathBuf, String)> = Vec::new();

    // Refuse a symlinked rules directory: the real .md files behind it
    // would pass per-file is_symlink checks and be read from outside the
    // workspace subtree — same escape class as #417.
    if fs::symlink_metadata(&rules_dir)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        tracing::warn!(
            target: "project_context",
            dir = %rules_dir.display(),
            "Refusing symlinked rules directory"
        );
        return entries;
    }

    let dir_iter = match fs::read_dir(&rules_dir) {
        Ok(iter) => iter,
        Err(_) => return entries,
    };

    let mut file_paths: Vec<PathBuf> = Vec::new();
    for entry in dir_iter.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "md") && context_candidate_exists(&path) {
            file_paths.push(path);
        }
    }

    // Sort by filename for deterministic order
    file_paths.sort_by(|a, b| {
        a.file_name()
            .unwrap_or_default()
            .cmp(b.file_name().unwrap_or_default())
    });

    // Enforce per-directory cap
    let total = file_paths.len();
    if total > MAX_RULES_FILES {
        tracing::warn!(
            target: "project_context",
            dir = %rules_dir.display(),
            total,
            cap = MAX_RULES_FILES,
            "Truncating rules directory to cap"
        );
        file_paths.truncate(MAX_RULES_FILES);
    }

    for path in file_paths {
        match load_context_file(&path) {
            Ok(content) => {
                tracing::info!(
                    "Loaded project rule from {} ({} bytes)",
                    path.display(),
                    content.len()
                );
                entries.push((path, content));
            }
            Err(error) => {
                tracing::warn!(
                    target: "project_context",
                    ?error,
                    ?path,
                    "Skipping unreadable rules file"
                );
            }
        }
    }

    entries
}

#[cfg(unix)]
fn open_context_file(path: &Path) -> Result<fs::File, ProjectContextError> {
    use std::os::unix::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|source| ProjectContextError::Read {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(not(unix))]
fn open_context_file(path: &Path) -> Result<fs::File, ProjectContextError> {
    fs::File::open(path).map_err(|source| ProjectContextError::Read {
        path: path.to_path_buf(),
        source,
    })
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

This file provides guidance to AI agents (CodeWhale, Claude Code, etc.) when working with code in this repository.

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
    fn mixed_advisory_and_enforced_invariants_render_and_back_compat_holds() {
        let tmp = tempdir().expect("tempdir");
        let dir = tmp.path().join(".codewhale");
        fs::create_dir_all(&dir).expect("law dir");
        fs::write(
            dir.join("constitution.json"),
            r#"{
                "protected_invariants": [
                    "Plain advisory prose.",
                    { "text": "The wire format is frozen", "paths": ["crates/protocol/**"], "action": "block" }
                ]
            }"#,
        )
        .expect("write law");

        let (block, path, warnings) = load_repo_constitution_block(tmp.path());
        let block = block.expect("law renders");
        assert!(path.is_some());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(block.contains("- Plain advisory prose."), "{block}");
        assert!(
            block.contains(
                "- The wire format is frozen (mechanically enforced for: crates/protocol/**)"
            ),
            "{block}"
        );

        // The enforcement loader compiles only the enforced entry.
        let rules = load_repo_law_rules(tmp.path());
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].text, "The wire format is frozen");
        assert_eq!(rules[0].action, RepoLawAction::Block);
        assert!(rules[0].globs.is_match("crates/protocol/wire.rs"));
    }

    #[test]
    fn legacy_string_only_invariants_render_unchanged_and_compile_nothing() {
        let tmp = tempdir().expect("tempdir");
        let dir = tmp.path().join(".codewhale");
        fs::create_dir_all(&dir).expect("law dir");
        fs::write(
            dir.join("constitution.json"),
            r#"{"protected_invariants": ["Keep DeepSeek support first-class."]}"#,
        )
        .expect("write law");

        let (block, _, warnings) = load_repo_constitution_block(tmp.path());
        let block = block.expect("law renders");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(
            block.contains("- Keep DeepSeek support first-class."),
            "{block}"
        );
        assert!(!block.contains("mechanically enforced"), "{block}");
        assert!(load_repo_law_rules(tmp.path()).is_empty());
    }

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

    #[cfg(unix)]
    #[test]
    fn project_context_rejects_symlinked_agents_md() {
        let workspace = tempdir().expect("workspace tempdir");
        let outside = tempdir().expect("outside tempdir");
        let outside_agents = outside.path().join("AGENTS.md");
        fs::write(&outside_agents, "outside instructions").expect("write outside agents");
        std::os::unix::fs::symlink(&outside_agents, workspace.path().join("AGENTS.md"))
            .expect("symlink agents");

        let ctx = load_project_context(workspace.path());

        assert!(
            !ctx.has_instructions(),
            "symlinked project instructions must not be loaded: {:?}",
            ctx.instructions
        );
        assert!(
            ctx.warnings.iter().any(|w| w.contains("symlinked")),
            "expected symlink warning, got {:?}",
            ctx.warnings
        );
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
    fn agents_md_used_while_whale_md_is_ignored() {
        let tmp = tempdir().expect("tempdir");
        fs::write(tmp.path().join("AGENTS.md"), "AGENTS canonical").expect("write agents");
        fs::write(tmp.path().join("WHALE.md"), "WHALE legacy").expect("write whale");

        let ctx = load_project_context(tmp.path());
        let instructions = ctx.instructions.expect("instructions loaded");
        assert!(instructions.contains("AGENTS canonical"), "{instructions}");
        assert!(!instructions.contains("WHALE legacy"), "{instructions}");
        assert!(
            ctx.warnings
                .iter()
                .any(|w| w.contains("WHALE.md is ignored")),
            "{:?}",
            ctx.warnings
        );
    }

    #[test]
    fn whale_md_alone_is_ignored_with_migration_warning() {
        let tmp = tempdir().expect("tempdir");
        fs::write(tmp.path().join("WHALE.md"), "WHALE legacy body").expect("write whale");

        let ctx = load_project_context(tmp.path());
        assert!(
            ctx.instructions.is_none(),
            "legacy WHALE.md must not be read"
        );
        assert!(
            ctx.warnings
                .iter()
                .any(|w| w.contains("WHALE.md is ignored")),
            "expected ignored-file warning, got {:?}",
            ctx.warnings
        );
    }

    #[test]
    fn constitution_json_renders_authority_block() {
        let tmp = tempdir().expect("tempdir");
        fs::create_dir(tmp.path().join(".git")).expect("mkdir .git");
        fs::create_dir(tmp.path().join(".codewhale")).expect("mkdir .codewhale");
        fs::write(
            tmp.path().join(".codewhale").join("constitution.json"),
            r#"{
                "schema_version": 1,
                "authority": ["current user request", "live code and tests", "AGENTS.md"],
                "protected_invariants": ["keep the tool-catalog head byte-stable"],
                "branch_policy": "Start from live branch truth; open PRs into main",
                "verification_policy": { "before_claiming_done": ["run focused tests"] },
                "escalate_when": ["a destructive action was not authorized"]
            }"#,
        )
        .expect("write constitution");

        let ctx = load_project_context_with_parents(tmp.path());
        let block = ctx
            .constitution_block
            .as_deref()
            .expect("constitution block rendered");
        assert!(block.contains("<codewhale_repo_constitution"));
        assert!(block.contains("current user request"));
        assert!(block.contains("run focused tests"));
        assert!(block.contains("keep the tool-catalog head byte-stable"));
        assert!(block.contains("Start from live branch truth"));
        assert!(block.contains("a destructive action was not authorized"));
        assert!(block.contains("WHALE.md is ignored and should be migrated"));
        assert!(
            ctx.constitution_source_path
                .as_ref()
                .is_some_and(|path| path.ends_with(".codewhale/constitution.json")),
            "constitution source path should be visible: {:?}",
            ctx.constitution_source_path
        );
        // It also surfaces through the system block.
        assert!(
            ctx.as_system_block()
                .expect("system block")
                .contains("codewhale_repo_constitution")
        );
    }

    #[test]
    fn stale_constitution_branch_policy_warns() {
        let tmp = tempdir().expect("tempdir");
        fs::create_dir(tmp.path().join(".git")).expect("mkdir .git");
        fs::create_dir(tmp.path().join(".codewhale")).expect("mkdir .codewhale");
        fs::write(
            tmp.path().join(".codewhale").join("constitution.json"),
            r#"{
                "schema_version": 1,
                "authority": ["current user request"],
                "branch_policy": "v0.8.53 work targets the codex/v0.8.53 integration branch, not main"
            }"#,
        )
        .expect("write constitution");

        let ctx = load_project_context_with_parents(tmp.path());
        assert!(
            ctx.constitution_block.is_some(),
            "stale policy should warn but still render"
        );
        assert!(
            ctx.warnings
                .iter()
                .any(|warning| warning.contains("branch_policy appears stale")),
            "expected stale branch_policy warning, got {:?}",
            ctx.warnings
        );
    }

    #[test]
    fn repository_constitution_avoids_hard_coded_release_lane_policy() {
        let repo_constitution = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(".codewhale")
            .join("constitution.json");
        let raw = fs::read_to_string(&repo_constitution).expect("read repo constitution");
        let constitution: RepoConstitution =
            serde_json::from_str(&raw).expect("parse repo constitution");
        let warnings = constitution.policy_warnings(&repo_constitution);
        assert!(
            warnings.is_empty(),
            "repo constitution should not carry stale release-lane policy: {:?}",
            warnings
        );
    }

    #[test]
    fn malformed_constitution_warns_without_crashing() {
        let tmp = tempdir().expect("tempdir");
        fs::create_dir(tmp.path().join(".git")).expect("mkdir .git");
        fs::create_dir(tmp.path().join(".codewhale")).expect("mkdir .codewhale");
        fs::write(
            tmp.path().join(".codewhale").join("constitution.json"),
            "{ not valid json",
        )
        .expect("write bad constitution");

        let ctx = load_project_context_with_parents(tmp.path());
        assert!(
            ctx.constitution_block.is_none(),
            "no block for invalid JSON"
        );
        assert!(
            ctx.warnings.iter().any(|w| w.contains("Failed to parse")),
            "expected parse warning, got {:?}",
            ctx.warnings
        );
    }

    #[cfg(unix)]
    #[test]
    fn constitution_json_rejects_symlinked_file() {
        let workspace = tempdir().expect("workspace tempdir");
        let outside = tempdir().expect("outside tempdir");
        fs::create_dir(workspace.path().join(".git")).expect("mkdir .git");
        fs::create_dir(workspace.path().join(".codewhale")).expect("mkdir .codewhale");
        let outside_constitution = outside.path().join("constitution.json");
        fs::write(
            &outside_constitution,
            r#"{"schema_version":1,"authority":["outside authority"]}"#,
        )
        .expect("write outside constitution");
        std::os::unix::fs::symlink(
            &outside_constitution,
            workspace
                .path()
                .join(".codewhale")
                .join("constitution.json"),
        )
        .expect("symlink constitution");

        let ctx =
            load_project_context_with_parents_and_home(workspace.path(), Some(outside.path()));

        assert!(
            ctx.constitution_block.is_none(),
            "symlinked constitution must not be loaded: {:?}",
            ctx.constitution_block
        );
        assert!(
            !ctx.as_system_block()
                .unwrap_or_default()
                .contains("outside authority"),
            "symlink target content must not reach the system block"
        );
        assert!(
            ctx.warnings.iter().any(|w| w.contains("symlinked")),
            "expected symlink warning, got {:?}",
            ctx.warnings
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
        fs::create_dir_all(tmp.path().join(".codewhale").join("state")).expect("mkdir state");
        fs::write(
            tmp.path()
                .join(".codewhale")
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
    fn generated_context_is_bounded_and_ephemeral_for_many_file_workspace() {
        let workspace = tempdir().expect("workspace tempdir");
        let home = tempdir().expect("home tempdir");
        let noisy = workspace.path().join("aaa-many-files");
        fs::create_dir_all(&noisy).expect("mkdir noisy");
        for i in 0..1000 {
            fs::write(noisy.join(format!("file-{i:04}.rs")), "fn noisy() {}").expect("write noisy");
        }
        fs::create_dir_all(workspace.path().join("zzz-important")).expect("mkdir important");
        fs::write(
            workspace.path().join("zzz-important").join("main.rs"),
            "fn important() {}",
        )
        .expect("write important");

        let start = std::time::Instant::now();
        let ctx = load_project_context_with_parents_and_home(workspace.path(), Some(home.path()));
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "auto-generated context should stay bounded, took {elapsed:?}"
        );
        assert!(ctx.has_instructions());

        let generated_path = workspace.path().join(".codewhale").join("instructions.md");
        assert_eq!(ctx.source_path, None);
        assert!(
            !generated_path.exists(),
            "generated project context should stay ephemeral"
        );
        assert!(
            !workspace.path().join(".codewhale").exists(),
            "loading context should not create a .codewhale directory"
        );
        let generated = ctx.instructions.as_ref().expect("generated instructions");
        assert!(generated.contains("Project Context (Auto-generated, ephemeral)"));
        assert!(generated.contains("Bounded Project Overview"));
        assert!(!generated.contains("<project_context_pack>"));
        assert!(
            generated.contains("\"zzz-important/\""),
            "later top-level project areas should remain visible:\n{generated}"
        );
        let noisy_count = generated.matches("aaa-many-files/file-").count();
        assert!(
            noisy_count < 300,
            "generated context should not list the whole noisy directory; saw {noisy_count}"
        );
        assert!(
            !generated.contains("file-0999.rs"),
            "bounded context should omit the tail of the noisy directory"
        );
    }

    #[test]
    fn cached_context_reflects_overwritten_agents_md() {
        crate::project_context_cache::clear();
        let workspace = tempdir().expect("workspace tempdir");
        let home = tempdir().expect("home tempdir");
        let agents = workspace.path().join("AGENTS.md");
        fs::write(&agents, "alpha").expect("write alpha");

        let first =
            load_project_context_with_parents_cached_and_home(workspace.path(), Some(home.path()));
        assert!(
            first
                .instructions
                .as_deref()
                .is_some_and(|s| s.contains("alpha")),
            "expected alpha instructions: {:?}",
            first.instructions
        );

        fs::write(&agents, "bravo").expect("write bravo");
        let second =
            load_project_context_with_parents_cached_and_home(workspace.path(), Some(home.path()));

        assert!(
            second
                .instructions
                .as_deref()
                .is_some_and(|s| s.contains("bravo")),
            "cache must invalidate on same-length content overwrite: {:?}",
            second.instructions
        );
    }

    #[test]
    fn cached_context_reflects_constitution_json_change() {
        crate::project_context_cache::clear();
        let workspace = tempdir().expect("workspace tempdir");
        let home = tempdir().expect("home tempdir");
        fs::create_dir(workspace.path().join(".git")).expect("mkdir git");
        fs::create_dir(workspace.path().join(".codewhale")).expect("mkdir codewhale");
        let constitution = workspace
            .path()
            .join(".codewhale")
            .join("constitution.json");
        fs::write(
            &constitution,
            r#"{"schema_version":1,"authority":["alpha authority"]}"#,
        )
        .expect("write alpha constitution");

        let first =
            load_project_context_with_parents_cached_and_home(workspace.path(), Some(home.path()));
        assert!(
            first
                .constitution_block
                .as_deref()
                .is_some_and(|s| s.contains("alpha authority")),
            "expected alpha constitution block: {:?}",
            first.constitution_block
        );

        fs::write(
            &constitution,
            r#"{"schema_version":1,"authority":["bravo authority"]}"#,
        )
        .expect("write bravo constitution");
        let second =
            load_project_context_with_parents_cached_and_home(workspace.path(), Some(home.path()));

        assert!(
            second
                .constitution_block
                .as_deref()
                .is_some_and(|s| s.contains("bravo authority")),
            "cache must invalidate when constitution changes: {:?}",
            second.constitution_block
        );
    }

    #[test]
    fn cached_generated_context_stays_ephemeral() {
        crate::project_context_cache::clear();
        let workspace = tempdir().expect("workspace tempdir");
        let home = tempdir().expect("home tempdir");

        let first =
            load_project_context_with_parents_cached_and_home(workspace.path(), Some(home.path()));
        assert!(first.has_instructions());
        let generated_path = workspace.path().join(".codewhale").join("instructions.md");
        assert!(
            !generated_path.exists(),
            "first load should not write generated instructions"
        );

        let second =
            load_project_context_with_parents_cached_and_home(workspace.path(), Some(home.path()));
        assert!(second.has_instructions());
        assert!(
            !generated_path.exists(),
            "cached generated context should remain in memory-only state"
        );
    }

    #[test]
    fn cached_context_reflects_trust_marker_created() {
        crate::project_context_cache::clear();
        let workspace = tempdir().expect("workspace tempdir");
        let home = tempdir().expect("home tempdir");
        fs::write(workspace.path().join("AGENTS.md"), "instructions").expect("write agents");

        let first =
            load_project_context_with_parents_cached_and_home(workspace.path(), Some(home.path()));
        assert!(!first.is_trusted);

        let trust_dir = workspace.path().join(".deepseek");
        fs::create_dir(&trust_dir).expect("mkdir trust dir");
        fs::write(trust_dir.join("trusted"), "").expect("write trust marker");

        let second =
            load_project_context_with_parents_cached_and_home(workspace.path(), Some(home.path()));
        assert!(
            second.is_trusted,
            "cache must invalidate when trust marker appears"
        );
    }

    #[test]
    fn project_context_pack_sort_is_cross_platform_and_priority_aware() {
        let mut unix_paths = vec![
            "src/z.rs".to_string(),
            "docs/".to_string(),
            "README.md".to_string(),
            "Cargo.toml".to_string(),
            "src/a.rs".to_string(),
            "notes.txt".to_string(),
        ];
        let mut windows_paths = vec![
            "src\\z.rs".to_string(),
            "docs\\".to_string(),
            "README.md".to_string(),
            "Cargo.toml".to_string(),
            "src\\a.rs".to_string(),
            "notes.txt".to_string(),
        ];

        sort_pack_paths(&mut unix_paths);
        sort_pack_paths(&mut windows_paths);

        let normalized_windows = windows_paths
            .iter()
            .map(|path| path.replace('\\', "/"))
            .collect::<Vec<_>>();
        assert_eq!(unix_paths, normalized_windows);
        assert_eq!(
            unix_paths,
            vec![
                "README.md",
                "Cargo.toml",
                "src/a.rs",
                "src/z.rs",
                "docs/",
                "notes.txt",
            ]
        );
    }

    #[test]
    fn normalize_pack_relative_path_rejects_parent_segments() {
        assert_eq!(
            normalize_pack_relative_path(".\\src\\main.rs"),
            Some("src/main.rs".to_string())
        );
        assert_eq!(normalize_pack_relative_path("../secret.txt"), None);
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
    fn test_load_global_agents_falls_back_to_vendor_neutral_path() {
        let workspace = tempdir().expect("workspace tempdir");
        let home = tempdir().expect("home tempdir");
        let global_dir = home.path().join(".agents");
        fs::create_dir(&global_dir).expect("mkdir .agents");
        let global_agents = global_dir.join("AGENTS.md");
        fs::write(&global_agents, "Vendor-neutral instructions").expect("write global agents");

        let ctx = load_project_context_with_parents_and_home(workspace.path(), Some(home.path()));

        assert!(ctx.has_instructions());
        assert!(
            ctx.instructions
                .as_ref()
                .unwrap()
                .contains("Vendor-neutral instructions")
        );
        assert_eq!(ctx.source_path, Some(global_agents));
    }

    #[test]
    fn test_codewhale_specific_path_wins_over_agents_path() {
        let workspace = tempdir().expect("workspace tempdir");
        let home = tempdir().expect("home tempdir");

        let codewhale_dir = home.path().join(".codewhale");
        fs::create_dir(&codewhale_dir).expect("mkdir .codewhale");
        let codewhale_agents = codewhale_dir.join("AGENTS.md");
        fs::write(&codewhale_agents, "CodeWhale-specific instructions")
            .expect("write codewhale agents");

        let agents_dir = home.path().join(".agents");
        fs::create_dir(&agents_dir).expect("mkdir .agents");
        fs::write(agents_dir.join("AGENTS.md"), "Vendor-neutral instructions")
            .expect("write vendor-neutral agents");

        let ctx = load_project_context_with_parents_and_home(workspace.path(), Some(home.path()));

        assert!(ctx.has_instructions());
        let instructions = ctx.instructions.as_ref().unwrap();
        assert!(
            instructions.contains("CodeWhale-specific instructions"),
            "CodeWhale-specific global file should win:\n{instructions}"
        );
        assert!(
            !instructions.contains("Vendor-neutral instructions"),
            "lower-priority .agents file should be skipped:\n{instructions}"
        );
        assert_eq!(ctx.source_path, Some(codewhale_agents));
    }

    #[test]
    fn test_global_agents_wins_over_global_whale_across_paths() {
        let workspace = tempdir().expect("workspace tempdir");
        let home = tempdir().expect("home tempdir");

        let codewhale_dir = home.path().join(".codewhale");
        fs::create_dir(&codewhale_dir).expect("mkdir .codewhale");
        fs::write(codewhale_dir.join("WHALE.md"), "Global WHALE legacy")
            .expect("write codewhale whale");

        let agents_dir = home.path().join(".agents");
        fs::create_dir(&agents_dir).expect("mkdir .agents");
        let global_agents = agents_dir.join("AGENTS.md");
        fs::write(&global_agents, "Global AGENTS canonical").expect("write global agents");

        let ctx = load_project_context_with_parents_and_home(workspace.path(), Some(home.path()));

        assert!(ctx.has_instructions());
        let instructions = ctx.instructions.as_ref().unwrap();
        assert!(
            instructions.contains("Global AGENTS canonical"),
            "global AGENTS.md should win:\n{instructions}"
        );
        assert!(
            !instructions.contains("Global WHALE legacy"),
            "global WHALE.md content should be skipped when any global AGENTS.md exists:\n{instructions}"
        );
        assert!(
            ctx.warnings
                .iter()
                .any(|warning| warning.contains("WHALE.md is ignored")),
            "ignored WHALE.md should emit migration warning: {:?}",
            ctx.warnings
        );
        assert_eq!(ctx.source_path, Some(global_agents));
    }

    #[test]
    fn test_global_whale_is_ignored_when_no_global_agents_exists() {
        let workspace = tempdir().expect("workspace tempdir");
        let home = tempdir().expect("home tempdir");

        let codewhale_dir = home.path().join(".codewhale");
        fs::create_dir(&codewhale_dir).expect("mkdir .codewhale");
        let global_whale = codewhale_dir.join("WHALE.md");
        fs::write(&global_whale, "Global WHALE legacy").expect("write codewhale whale");

        let ctx = load_project_context_with_parents_and_home(workspace.path(), Some(home.path()));

        let instructions = ctx.instructions.as_deref().unwrap_or("");
        assert!(
            !instructions.contains("Global WHALE legacy"),
            "legacy WHALE.md must not be read when no global AGENTS.md exists:\n{instructions}"
        );
        assert!(
            ctx.warnings
                .iter()
                .any(|warning| warning.contains("WHALE.md is ignored")),
            "expected global WHALE.md ignored warning, got {:?}",
            ctx.warnings
        );
        assert_ne!(ctx.source_path, Some(global_whale));
    }

    #[test]
    fn test_global_instructions_md_is_autoloaded_while_whale_is_ignored() {
        // #3012: a global ~/.codewhale/instructions.md should be auto-loaded as
        // a fallback context layer while legacy WHALE.md remains ignored.
        let workspace = tempdir().expect("workspace tempdir");
        let home = tempdir().expect("home tempdir");

        let codewhale_dir = home.path().join(".codewhale");
        fs::create_dir(&codewhale_dir).expect("mkdir .codewhale");
        fs::write(codewhale_dir.join("WHALE.md"), "Global WHALE legacy")
            .expect("write codewhale whale");
        let global_instructions = codewhale_dir.join("instructions.md");
        fs::write(&global_instructions, "Global instructions body")
            .expect("write global instructions");

        let ctx = load_project_context_with_parents_and_home(workspace.path(), Some(home.path()));

        assert!(ctx.has_instructions());
        let instructions = ctx.instructions.as_ref().unwrap();
        assert!(
            instructions.contains("Global instructions body"),
            "global instructions.md should be auto-loaded:\n{instructions}"
        );
        assert!(
            !instructions.contains("Global WHALE legacy"),
            "instructions.md should load without reading ignored WHALE.md:\n{instructions}"
        );
        assert!(
            ctx.warnings
                .iter()
                .any(|warning| warning.contains("WHALE.md is ignored")),
            "ignored WHALE.md should emit migration warning: {:?}",
            ctx.warnings
        );
        assert_eq!(ctx.source_path, Some(global_instructions));
    }

    #[test]
    fn test_global_agents_outranks_global_instructions() {
        // #3012 precedence: AGENTS.md > instructions.md.
        let workspace = tempdir().expect("workspace tempdir");
        let home = tempdir().expect("home tempdir");

        let codewhale_dir = home.path().join(".codewhale");
        fs::create_dir(&codewhale_dir).expect("mkdir .codewhale");
        let global_agents = codewhale_dir.join("AGENTS.md");
        fs::write(&global_agents, "Global AGENTS canonical").expect("write global agents");
        fs::write(
            codewhale_dir.join("instructions.md"),
            "Global instructions body",
        )
        .expect("write global instructions");

        let ctx = load_project_context_with_parents_and_home(workspace.path(), Some(home.path()));

        assert!(ctx.has_instructions());
        let instructions = ctx.instructions.as_ref().unwrap();
        assert!(
            instructions.contains("Global AGENTS canonical"),
            "global AGENTS.md should outrank instructions.md:\n{instructions}"
        );
        assert!(
            !instructions.contains("Global instructions body"),
            "instructions.md should be skipped when a global AGENTS.md exists:\n{instructions}"
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
                .contains("Project Context (Auto-generated, ephemeral)")
        );
    }

    // ── Rules directory auto-discovery tests ──

    #[test]
    fn rules_from_codewhale_dir_are_loaded_as_project_context() {
        let tmp = tempdir().expect("tempdir");
        let rules_dir = tmp.path().join(".codewhale/rules");
        fs::create_dir_all(&rules_dir).expect("mkdir rules");
        fs::write(
            rules_dir.join("security.md"),
            "# Security\nNo hardcoded secrets.",
        )
        .expect("write");

        let ctx = load_project_context(tmp.path());

        let rules = ctx.rules_block.as_ref().expect("rules_block should be set");
        assert!(
            rules.contains("Security"),
            "expected rules content, got: {rules}"
        );
        assert!(
            rules.contains("<project_rule source="),
            "expected <project_rule> wrapper, got: {rules}"
        );
    }

    #[test]
    fn rules_are_loaded_in_filename_order() {
        let tmp = tempdir().expect("tempdir");
        let rules_dir = tmp.path().join(".codewhale/rules");
        fs::create_dir_all(&rules_dir).expect("mkdir rules");
        fs::write(rules_dir.join("zzz.md"), "last").expect("write");
        fs::write(rules_dir.join("aaa.md"), "first").expect("write");
        fs::write(rules_dir.join("mmm.md"), "middle").expect("write");

        let ctx = load_project_context(tmp.path());
        let rules = ctx.rules_block.as_ref().unwrap();

        let pos_aaa = rules.find("first").unwrap();
        let pos_mmm = rules.find("middle").unwrap();
        let pos_zzz = rules.find("last").unwrap();
        assert!(pos_aaa < pos_mmm, "aaa should come before mmm");
        assert!(pos_mmm < pos_zzz, "mmm should come before zzz");
    }

    #[test]
    fn rules_from_claude_dir_are_compat_loaded() {
        let tmp = tempdir().expect("tempdir");
        let rules_dir = tmp.path().join(".claude/rules");
        fs::create_dir_all(&rules_dir).expect("mkdir rules");
        fs::write(rules_dir.join("style.md"), "Use tabs").expect("write");

        let ctx = load_project_context(tmp.path());

        let rules = ctx.rules_block.as_ref().expect("rules should be loaded");
        assert!(
            rules.contains("Use tabs"),
            "expected .claude/rules/ compat loading"
        );
    }

    #[test]
    fn rules_directory_missing_does_not_crash() {
        let tmp = tempdir().expect("tempdir");
        // No .codewhale/rules/ or .claude/rules/ directories exist
        let ctx = load_project_context(tmp.path());
        // Rules block should be None when no rules directories exist
        assert!(
            ctx.rules_block.is_none(),
            "rules_block should be None when no rules exist"
        );
    }

    #[test]
    fn rules_coexist_with_agents_md() {
        let tmp = tempdir().expect("tempdir");
        fs::write(tmp.path().join("AGENTS.md"), "Main project instructions").expect("write");
        let rules_dir = tmp.path().join(".codewhale/rules");
        fs::create_dir_all(&rules_dir).expect("mkdir rules");
        fs::write(rules_dir.join("extra.md"), "Extra rule").expect("write");

        let ctx = load_project_context(tmp.path());
        let instructions = ctx.instructions.as_ref().unwrap();
        let rules = ctx.rules_block.as_ref().unwrap();

        assert!(
            instructions.contains("Main project instructions"),
            "AGENTS.md content missing"
        );
        assert!(rules.contains("Extra rule"), "rules content missing");
        // AGENTS.md should come first in system block
        let block = ctx.as_system_block().unwrap();
        let pos_agents = block.find("Main project instructions").unwrap();
        let pos_rule = block.find("Extra rule").unwrap();
        assert!(pos_agents < pos_rule, "AGENTS.md should precede rules");
    }

    #[test]
    fn non_md_files_in_rules_dir_are_ignored() {
        let tmp = tempdir().expect("tempdir");
        let rules_dir = tmp.path().join(".codewhale/rules");
        fs::create_dir_all(&rules_dir).expect("mkdir rules");
        fs::write(rules_dir.join("notes.txt"), "should be ignored").expect("write");
        fs::write(rules_dir.join("valid.md"), "loaded").expect("write");

        let ctx = load_project_context(tmp.path());
        let rules = ctx.rules_block.as_ref().unwrap();

        assert!(rules.contains("loaded"), "valid .md should be loaded");
        assert!(
            !rules.contains("should be ignored"),
            ".txt should be ignored"
        );
    }

    #[test]
    fn rules_cap_truncates_excess_files() {
        let tmp = tempdir().expect("tempdir");
        let rules_dir = tmp.path().join(".codewhale/rules");
        fs::create_dir_all(&rules_dir).expect("mkdir rules");

        // Create more files than the cap
        for i in 0..60 {
            fs::write(
                rules_dir.join(format!("rule_{i:04}.md")),
                format!("content {i}"),
            )
            .expect("write");
        }

        let ctx = load_project_context(tmp.path());
        let rules = ctx.rules_block.as_ref().unwrap();

        // The last file (by sorted name) should NOT be present
        assert!(
            !rules.contains("content 59"),
            "rule_0059 should be above cap"
        );
        // The first file should be present
        assert!(
            rules.contains("content 0"),
            "rule_0000 should be within cap"
        );
        // Count <project_rule> blocks
        let count = rules.matches("<project_rule source=").count();
        assert_eq!(
            count, MAX_RULES_FILES,
            "exactly {MAX_RULES_FILES} rules should be loaded"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rules_rejects_symlinked_files() {
        let workspace = tempdir().expect("workspace tempdir");
        let outside = tempdir().expect("outside tempdir");
        let rules_dir = workspace.path().join(".codewhale/rules");
        fs::create_dir_all(&rules_dir).expect("mkdir rules");

        let outside_rule = outside.path().join("outside.md");
        fs::write(&outside_rule, "outside content").expect("write outside");
        std::os::unix::fs::symlink(&outside_rule, rules_dir.join("outside.md"))
            .expect("symlink rule");

        let ctx = load_project_context(workspace.path());

        // Symlinked rules must not be loaded
        assert!(
            ctx.rules_block.is_none()
                || !ctx
                    .rules_block
                    .as_ref()
                    .unwrap()
                    .contains("outside content"),
            "symlinked rules must not be loaded"
        );
    }

    #[cfg(unix)]
    #[test]
    fn rules_rejects_symlinked_directory() {
        let workspace = tempdir().expect("workspace tempdir");
        let outside = tempdir().expect("outside tempdir");
        let outside_dir = outside.path().join("real_rules");
        fs::create_dir_all(&outside_dir).expect("mkdir outside dir");
        fs::write(outside_dir.join("secret.md"), "outside content").expect("write outside");
        fs::create_dir_all(workspace.path().join(".codewhale")).expect("mkdir codewhale");

        // Symlink the directory itself, not individual files
        std::os::unix::fs::symlink(&outside_dir, workspace.path().join(".codewhale/rules"))
            .expect("symlink rules dir");

        let ctx = load_project_context(workspace.path());

        // Symlinked rules directory must be refused at the directory level
        assert!(
            ctx.rules_block.is_none()
                || !ctx
                    .rules_block
                    .as_ref()
                    .unwrap()
                    .contains("outside content"),
            "symlinked rules directory must be refused"
        );
    }

    #[test]
    fn rules_from_both_dirs_are_loaded_together() {
        let tmp = tempdir().expect("tempdir");
        let codewhale_rules = tmp.path().join(".codewhale/rules");
        let claude_rules = tmp.path().join(".claude/rules");
        fs::create_dir_all(&codewhale_rules).expect("mkdir codewhale rules");
        fs::create_dir_all(&claude_rules).expect("mkdir claude rules");
        fs::write(codewhale_rules.join("cw.md"), "codewhale-rule").expect("write");
        fs::write(claude_rules.join("claude.md"), "claude-rule").expect("write");

        let ctx = load_project_context(tmp.path());
        let rules = ctx.rules_block.as_ref().unwrap();

        assert!(
            rules.contains("codewhale-rule"),
            ".codewhale/rules/ should be loaded"
        );
        assert!(
            rules.contains("claude-rule"),
            ".claude/rules/ should be loaded"
        );
        // .codewhale/rules/ content should appear before .claude/rules/ (RULES_DIRS order)
        let pos_cw = rules.find("codewhale-rule").unwrap();
        let pos_claude = rules.find("claude-rule").unwrap();
        assert!(
            pos_cw < pos_claude,
            ".codewhale/rules/ should precede .claude/rules/"
        );
    }

    #[test]
    fn rules_block_truncated_at_total_byte_budget() {
        let tmp = tempdir().expect("tempdir");
        let rules_dir = tmp.path().join(".codewhale/rules");
        fs::create_dir_all(&rules_dir).expect("mkdir rules");

        // Create files whose combined content exceeds MAX_RULES_BLOCK_BYTES
        let per_file = "X".repeat(20 * 1024); // 20 KB each
        for i in 0..30 {
            fs::write(rules_dir.join(format!("rule_{i:04}.md")), &per_file).expect("write");
        }

        let ctx = load_project_context(tmp.path());
        let rules = ctx.rules_block.as_ref().unwrap();

        assert!(
            rules.len() <= MAX_RULES_BLOCK_BYTES + 200, // + marker overhead
            "rules block should be truncated to budget: {} > {}",
            rules.len(),
            MAX_RULES_BLOCK_BYTES
        );
        assert!(
            rules.contains("truncated at 500 KB"),
            "truncation marker missing"
        );
    }
}
