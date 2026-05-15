//! Repo-aware working set tracking and prompt context packing.
//!
//! The goal of this module is to keep a small, high-signal list of
//! "active" paths that the assistant should prioritize. It observes
//! user messages and tool calls, extracts likely paths, and produces:
//! - a compact working-set summary block for the system prompt
//! - pinned message indices that compaction should preserve

use crate::models::{ContentBlock, Message};
use ignore::WalkBuilder;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Repo-aware resolver for `@`-mentions and file pickers.
///
/// `cwd` is captured at construction; if the host's current directory changes
/// during a session, build a fresh `Workspace`. Fuzzy lookups are backed by a
/// lazy basename → paths index built once on first miss and reused for the
/// rest of the session — without it, every mis-typed mention triggered a full
/// `WalkBuilder` traversal up to depth 6 (Gemini code-review feedback).
#[derive(Debug)]
pub struct Workspace {
    pub root: PathBuf,
    cwd: Option<PathBuf>,
    file_index: OnceLock<HashMap<String, Vec<PathBuf>>>,
}

impl Workspace {
    /// Construct a workspace anchored at `root`, capturing the process CWD as
    /// the secondary resolution pass. Convenience entry point intended for
    /// callers that don't already have a CWD on hand; the App routes through
    /// [`Workspace::with_cwd`] with its own captured launch directory.
    #[allow(dead_code)] // Keeps the surface stable for #97 (Ctrl+P picker).
    pub fn new(root: PathBuf) -> Self {
        Self::with_cwd(root, std::env::current_dir().ok())
    }

    /// Construct with an explicit cwd. Used by tests that need deterministic
    /// resolution against a known directory without depending on (and
    /// mutating) the process's real working directory.
    pub fn with_cwd(root: PathBuf, cwd: Option<PathBuf>) -> Self {
        Self {
            root,
            cwd,
            file_index: OnceLock::new(),
        }
    }

    /// Two-pass resolution: workspace, then cwd, then fuzzy fallback.
    pub fn resolve(&self, raw_path: &str) -> Result<PathBuf, PathBuf> {
        let path = expand_mention_home(raw_path);
        if path.is_absolute() {
            if path.exists() {
                return Ok(path);
            }
            return Err(path);
        }

        let ws_path = self.root.join(&path);
        if ws_path.exists() {
            return Ok(ws_path);
        }

        if let Some(cwd) = self.cwd.as_ref() {
            let cwd_path = cwd.join(&path);
            if cwd_path.exists() {
                return Ok(cwd_path);
            }
        }

        if let Some(fuzzy) = self.fuzzy_resolve(&path) {
            return Ok(fuzzy);
        }

        Err(ws_path)
    }

    fn fuzzy_resolve(&self, path: &Path) -> Option<PathBuf> {
        let needle = path.file_name()?.to_string_lossy().to_lowercase();
        if needle.is_empty() {
            return None;
        }

        let index = self.file_index.get_or_init(|| self.build_file_index());
        index.get(&needle).and_then(|paths| paths.first()).cloned()
    }

    fn build_file_index(&self) -> HashMap<String, Vec<PathBuf>> {
        let mut index: HashMap<String, Vec<PathBuf>> = HashMap::new();
        let mut total: usize = 0;
        let builder = discovery_walk_builder(&self.root, Some(6));

        for entry in builder.build().flatten() {
            if total >= FILE_INDEX_MAX_ENTRIES {
                tracing::warn!(
                    target: "working_set",
                    limit = FILE_INDEX_MAX_ENTRIES,
                    "file-index discovery hit the entry cap; truncating to keep first-turn latency bounded (#697)"
                );
                return index;
            }
            if entry
                .file_type()
                .is_some_and(|ft| ft.is_file() || ft.is_dir())
            {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                index
                    .entry(name)
                    .or_default()
                    .push(entry.path().to_path_buf());
                total += 1;
            }
        }

        // Also index AI-tool dot-directories with gitignore disabled.
        for dir_name in DISCOVERY_ALWAYS_DIRS {
            if total >= FILE_INDEX_MAX_ENTRIES {
                break;
            }
            let dot_dir = self.root.join(dir_name);
            if !dot_dir.is_dir() {
                continue;
            }
            let mut dot_builder = WalkBuilder::new(&dot_dir);
            dot_builder
                .hidden(true)
                .follow_links(false)
                .git_ignore(false)
                .ignore(false)
                .max_depth(Some(5));
            for entry in dot_builder.build().flatten() {
                if total >= FILE_INDEX_MAX_ENTRIES {
                    break;
                }
                // Exclude machine-generated bulk (e.g. .deepseek/snapshots/).
                if path_is_excluded_from_discovery(&self.root, entry.path()) {
                    continue;
                }
                if entry
                    .file_type()
                    .is_some_and(|ft| ft.is_file() || ft.is_dir())
                {
                    let name = entry.file_name().to_string_lossy().to_lowercase();
                    index
                        .entry(name)
                        .or_default()
                        .push(entry.path().to_path_buf());
                    total += 1;
                }
            }
        }

        // Beyond the curated dot-dir whitelist above, also index any explicit
        // hidden/ignored path the user might `@`-mention (e.g. a project's
        // own `.generated/specs/`). `local_reference_paths` walks with
        // gitignore disabled but still honors `.deepseekignore`.
        for path in local_reference_paths(&self.root, LOCAL_REFERENCE_SCAN_LIMIT) {
            if total >= FILE_INDEX_MAX_ENTRIES {
                break;
            }
            let Some(name) = path
                .file_name()
                .map(|name| name.to_string_lossy().to_lowercase())
            else {
                continue;
            };
            index.entry(name).or_default().push(path);
            total += 1;
        }
        index
    }

    /// Walk the workspace (and the recorded `cwd` when it diverges) and
    /// return relative paths whose representation matches `partial`.
    ///
    /// Ranking: a candidate matches when its case-insensitive display string
    /// starts with `partial` (prefix hit) or contains it as a substring; prefix
    /// hits sort first so `docs/de` lands `docs/deepseek_v4.pdf` ahead of any
    /// path that merely shares those bytes.
    ///
    /// Display strings are workspace-relative for files under `root`, and
    /// cwd-relative for files only under the recorded `cwd` — so what the user
    /// Tab-completes matches what their shell would have shown them.
    ///
    /// Honors `.gitignore`, `.git/info/exclude`, `.ignore`, and
    /// `.deepseekignore`. Capped at `limit` results.
    #[must_use]
    pub fn completions(&self, partial: &str, limit: usize) -> Vec<String> {
        if limit == 0 {
            return Vec::new();
        }
        let needle = partial.to_lowercase();
        let mut prefix_hits: Vec<String> = Vec::new();
        let mut substring_hits: Vec<String> = Vec::new();
        let mut seen: HashSet<PathBuf> = HashSet::new();

        // Walk the recorded cwd first when it diverges from the workspace
        // root, so cwd-relative entries appear ahead of duplicates surfaced by
        // the workspace walk.
        let cwd_diverges = self
            .cwd
            .as_deref()
            .map(|c| c != self.root.as_path())
            .unwrap_or(false);
        if cwd_diverges && let Some(cwd) = self.cwd.as_deref() {
            walk_for_completions(
                cwd,
                cwd,
                &needle,
                limit,
                &mut prefix_hits,
                &mut substring_hits,
                &mut seen,
            );
            add_local_reference_completions(
                cwd,
                cwd,
                &needle,
                limit,
                &mut prefix_hits,
                &mut substring_hits,
                &mut seen,
            );
        }
        walk_for_completions(
            &self.root,
            &self.root,
            &needle,
            limit,
            &mut prefix_hits,
            &mut substring_hits,
            &mut seen,
        );
        add_local_reference_completions(
            &self.root,
            &self.root,
            &needle,
            limit,
            &mut prefix_hits,
            &mut substring_hits,
            &mut seen,
        );

        prefix_hits.sort();
        substring_hits.sort();
        prefix_hits.extend(substring_hits);
        prefix_hits.truncate(limit);
        prefix_hits
    }
}

/// Maximum directory depth walked when surfacing file-mention completions.
/// Mirrors the existing `project_tree` cutoff and keeps Tab snappy in deep
/// monorepos.
const COMPLETIONS_WALK_DEPTH: usize = 6;

/// Hard cap on the number of `(file or directory)` entries indexed by
/// [`Workspace::build_file_index`]. The fuzzy-resolve index is a
/// convenience for [`Workspace::fuzzy_resolve`]; missing entries fall
/// back to literal-path resolution. Capping here keeps the first
/// `fuzzy_resolve` call bounded on huge workspaces (#697 reported a
/// ~10s hang on the first turn). For typical projects 50K is well
/// above the actual entry count and the cap is a no-op.
const FILE_INDEX_MAX_ENTRIES: usize = 50_000;

/// Directories that must remain discoverable for `@`-mention completion and
/// fuzzy file resolution even when excluded by `.gitignore`. AI-tool
/// convention directories (`.deepseek/`, `.cursor/`, `.claude/`, `.agents/`)
/// are routinely gitignored, but users need to `@`-mention files inside them.
const DISCOVERY_ALWAYS_DIRS: &[&str] = &[".deepseek", ".cursor", ".claude", ".agents"];

/// Subdirectories under `DISCOVERY_ALWAYS_DIRS` that must NOT be indexed
/// even when the parent dir is walked with gitignore disabled. These are
/// large, machine-generated, or sensitive paths that would blow up the
/// walker (e.g. `.deepseek/snapshots/` — the snapshot side repo that
/// #1112 caps at 500 MB; indexing it would trigger the same OOM/hang
/// the cap was built to prevent).
const DISCOVERY_EXCLUDED_SUBDIRS: &[&str] = &[".deepseek/snapshots"];

/// Check whether a path resolved against `walk_root` falls inside any
/// `DISCOVERY_EXCLUDED_SUBDIRS` entry. Used to keep the snapshot side
/// repo (`.deepseek/snapshots/`) out of the completion/index walk.
fn path_is_excluded_from_discovery(walk_root: &Path, path: &Path) -> bool {
    for excluded in DISCOVERY_EXCLUDED_SUBDIRS {
        if path.starts_with(walk_root.join(excluded)) {
            return true;
        }
    }
    false
}

/// Configure a `WalkBuilder` for workspace discovery: hidden files, no
/// symlink following, depth-limited, custom `.deepseekignore` honored,
/// and gitignore overrides for AI-tool dot-directories so `@`-completion
/// finds them even when they're gitignored.
fn discovery_walk_builder(root: &Path, max_depth: Option<usize>) -> WalkBuilder {
    let mut builder = WalkBuilder::new(root);
    builder.hidden(true).follow_links(false);
    if let Some(depth) = max_depth {
        builder.max_depth(Some(depth));
    }
    let _ = builder.add_custom_ignore_filename(".deepseekignore");
    builder
}

/// Walk the AI-tool dot-directories (`.deepseek/`, `.cursor/`, `.claude/`,
/// `.agents/`) with gitignore disabled so their contents are discoverable
/// even when the project's `.gitignore` / `.ignore` excludes them.
#[allow(clippy::too_many_arguments)]
fn walk_always_discoverable_dirs(
    walk_root: &Path,
    display_root: &Path,
    needle: &str,
    limit: usize,
    prefix_hits: &mut Vec<String>,
    substring_hits: &mut Vec<String>,
    seen: &mut HashSet<PathBuf>,
    max_depth: Option<usize>,
) {
    for dir_name in DISCOVERY_ALWAYS_DIRS {
        let dot_dir = walk_root.join(dir_name);
        if !dot_dir.is_dir() {
            continue;
        }
        let mut builder = WalkBuilder::new(&dot_dir);
        builder
            .hidden(true)
            .follow_links(false)
            .git_ignore(false)
            .ignore(false);
        if let Some(depth) = max_depth {
            builder.max_depth(Some(depth.saturating_sub(1)));
        }
        for entry in builder.build().flatten() {
            if prefix_hits.len() + substring_hits.len() >= limit {
                break;
            }
            let path = entry.path();
            // Exclude machine-generated bulk (e.g. .deepseek/snapshots/)
            // even though gitignore is disabled for this walk.
            if path_is_excluded_from_discovery(walk_root, path) {
                continue;
            }
            let Ok(rel) = path.strip_prefix(display_root) else {
                continue;
            };
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if rel_str.is_empty() {
                continue;
            }
            let abs = path.to_path_buf();
            if !seen.insert(abs) {
                continue;
            }
            let is_dir = entry.file_type().is_some_and(|ft| ft.is_dir());
            let candidate = if is_dir {
                format!("{rel_str}/")
            } else {
                rel_str.clone()
            };
            let lower = candidate.to_lowercase();
            if needle.is_empty() || lower.starts_with(needle) {
                prefix_hits.push(candidate);
            } else if lower.contains(needle) {
                substring_hits.push(candidate);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn walk_for_completions(
    walk_root: &Path,
    display_root: &Path,
    needle: &str,
    limit: usize,
    prefix_hits: &mut Vec<String>,
    substring_hits: &mut Vec<String>,
    seen: &mut HashSet<PathBuf>,
) {
    let builder = discovery_walk_builder(walk_root, Some(COMPLETIONS_WALK_DEPTH));

    for entry in builder.build().flatten() {
        if prefix_hits.len() + substring_hits.len() >= limit {
            break;
        }
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(display_root) else {
            continue;
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if rel_str.is_empty() {
            continue;
        }
        // Dedup across the (cwd, workspace) double-walk by absolute path; we
        // want the cwd-relative display when both walks see the same file.
        let abs = path.to_path_buf();
        if !seen.insert(abs) {
            continue;
        }
        let is_dir = entry.file_type().is_some_and(|ft| ft.is_dir());
        let candidate = if is_dir {
            format!("{rel_str}/")
        } else {
            rel_str.clone()
        };
        let lower = candidate.to_lowercase();
        if needle.is_empty() || lower.starts_with(needle) {
            prefix_hits.push(candidate);
        } else if lower.contains(needle) {
            substring_hits.push(candidate);
        }
    }

    // Also walk the AI-tool dot-directories with gitignore disabled so
    // `.deepseek/`, `.cursor/`, etc. are always discoverable.
    walk_always_discoverable_dirs(
        walk_root,
        display_root,
        needle,
        limit,
        prefix_hits,
        substring_hits,
        seen,
        Some(COMPLETIONS_WALK_DEPTH),
    );
}

const LOCAL_REFERENCE_SCAN_LIMIT: usize = 4096;

#[allow(clippy::too_many_arguments)]
fn add_local_reference_completions(
    root: &Path,
    display_root: &Path,
    needle: &str,
    limit: usize,
    prefix_hits: &mut Vec<String>,
    substring_hits: &mut Vec<String>,
    seen: &mut HashSet<PathBuf>,
) {
    if !should_try_local_reference_completion(needle) {
        return;
    }

    for path in local_reference_paths(root, LOCAL_REFERENCE_SCAN_LIMIT) {
        if prefix_hits.len() + substring_hits.len() >= limit {
            break;
        }
        let Ok(rel) = path.strip_prefix(display_root) else {
            continue;
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if rel_str.is_empty() || !seen.insert(path.clone()) {
            continue;
        }
        let lower = rel_str.to_lowercase();
        if needle.is_empty() || lower.starts_with(needle) {
            prefix_hits.push(rel_str);
        } else if lower.contains(needle) {
            substring_hits.push(rel_str);
        }
    }
}

fn should_try_local_reference_completion(needle: &str) -> bool {
    !needle.is_empty() && (needle.starts_with('.') || needle.contains('/') || needle.contains('\\'))
}

fn local_reference_paths(root: &Path, limit: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .follow_links(false)
        .max_depth(Some(COMPLETIONS_WALK_DEPTH))
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false);
    let _ = builder.add_custom_ignore_filename(".deepseekignore");
    builder.filter_entry(|entry| !should_skip_local_reference_dir(entry.path()));

    for entry in builder.build().flatten() {
        if out.len() >= limit {
            break;
        }
        let path = entry.path();
        if path == root {
            continue;
        }
        if entry
            .file_type()
            .is_some_and(|ft| ft.is_file() || ft.is_dir())
        {
            out.push(path.to_path_buf());
        }
    }
    out
}

fn should_skip_local_reference_dir(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches!(
        name,
        ".git"
            | "target"
            | "node_modules"
            | ".venv"
            | "venv"
            | "env"
            | "dist"
            | "build"
            | "__pycache__"
            | ".ruff_cache"
    )
}

impl Clone for Workspace {
    fn clone(&self) -> Self {
        // Don't carry the cached file_index — clones get a fresh OnceLock so
        // they don't pin a stale snapshot of the previous owner's tree.
        Self {
            root: self.root.clone(),
            cwd: self.cwd.clone(),
            file_index: OnceLock::new(),
        }
    }
}

fn expand_mention_home(path: &str) -> PathBuf {
    if path == "~"
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home);
    }
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
}

/// Configuration for working-set tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingSetConfig {
    /// Maximum number of entries to keep.
    pub max_entries: usize,
    /// Maximum number of paths to pin during compaction.
    pub max_pinned_paths: usize,
    /// Maximum characters to scan per text block when pinning messages.
    pub max_scan_chars: usize,
    /// Maximum entries to show in the system prompt block.
    pub max_prompt_entries: usize,
}

impl Default for WorkingSetConfig {
    fn default() -> Self {
        Self {
            max_entries: 16,
            max_pinned_paths: 8,
            max_scan_chars: 2_000,
            max_prompt_entries: 8,
        }
    }
}

/// The source that most recently updated an entry.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum WorkingSetSource {
    UserMessage,
    ToolInput,
    ToolOutput,
    Rebuild,
}

/// A single working-set entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingSetEntry {
    /// Workspace-relative path string.
    pub path: String,
    /// Whether the path is a directory (best-effort).
    pub is_dir: bool,
    /// Whether the path exists on disk (best-effort).
    pub exists: bool,
    /// Number of times this path was observed.
    pub touches: u32,
    /// The last observed turn index.
    pub last_turn: u64,
    /// The last update source.
    pub last_source: WorkingSetSource,
}

impl WorkingSetEntry {
    fn new(path: String, exists: bool, is_dir: bool, turn: u64, source: WorkingSetSource) -> Self {
        Self {
            path,
            is_dir,
            exists,
            touches: 1,
            last_turn: turn,
            last_source: source,
        }
    }
}

/// Repo-aware working-set state.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkingSet {
    /// Tracking configuration.
    pub config: WorkingSetConfig,
    /// Monotonic turn counter (increments on user messages).
    pub turn: u64,
    /// Path entries keyed by workspace-relative path.
    pub entries: HashMap<String, WorkingSetEntry>,
}

impl WorkingSet {
    /// Advance to the next turn.
    pub fn next_turn(&mut self) {
        self.turn = self.turn.saturating_add(1);
    }

    /// Observe a user message and update the working set.
    pub fn observe_user_message(&mut self, text: &str, workspace: &Path) {
        self.next_turn();
        let paths = extract_paths_from_text(text);
        self.record_candidates(paths, workspace, WorkingSetSource::UserMessage);
    }

    /// Observe a tool call (input and optional output).
    pub fn observe_tool_call(
        &mut self,
        tool_name: &str,
        input: &Value,
        output: Option<&str>,
        workspace: &Path,
    ) {
        let input_candidates = extract_paths_from_value(input, Some(tool_name));
        self.record_candidates(input_candidates, workspace, WorkingSetSource::ToolInput);

        if let Some(text) = output {
            let output_candidates = extract_paths_from_text(text);
            self.record_candidates(output_candidates, workspace, WorkingSetSource::ToolOutput);
        }
    }

    /// Rebuild the working set from existing messages (best effort).
    ///
    /// This is used when syncing a resumed session.
    pub fn rebuild_from_messages(&mut self, messages: &[Message], workspace: &Path) {
        self.entries.clear();
        self.turn = 0;

        for message in messages {
            if message.role == "user" {
                self.next_turn();
            }
            let candidates = extract_paths_from_message(message);
            if candidates.is_empty() {
                continue;
            }
            self.record_candidates(candidates, workspace, WorkingSetSource::Rebuild);
        }
    }

    /// Render a compact working-set block for the system prompt.
    ///
    /// Byte-stable across `next_turn()` calls when no new paths are observed
    /// (#280): the rendered lines drop the turn-relative `touches` and
    /// `last seen N turn(s) ago` fields, and the order is taken from
    /// `sorted_for_prompt` (turn-agnostic) instead of `sorted_entries`.
    /// The block lands in the system prompt before the historical
    /// conversation; any byte that drifts here cache-misses everything that
    /// follows in DeepSeek's KV prefix cache.
    pub fn summary_block(&self, workspace: &Path) -> Option<String> {
        let prompt_entries: Vec<&WorkingSetEntry> = self
            .sorted_for_prompt()
            .into_iter()
            .take(self.config.max_prompt_entries)
            .collect();

        let repo_summary = summarize_repo_root(workspace);

        if repo_summary.is_none() && prompt_entries.is_empty() {
            return None;
        }

        let mut lines: Vec<String> = Vec::new();
        lines.push("## Repo Working Set".to_string());
        lines.push(format!("Workspace: {}", workspace.display()));

        if let Some(summary) = repo_summary {
            lines.push(summary);
        }

        if !prompt_entries.is_empty() {
            lines.push("Active paths (prioritize these):".to_string());
            for entry in prompt_entries {
                let kind = if entry.is_dir { "dir" } else { "file" };
                lines.push(format!("- {} ({kind})", entry.path));
            }
        }

        lines.push(
            "When in doubt, use tools to verify and keep changes focused on the working set."
                .to_string(),
        );

        Some(lines.join("\n"))
    }

    /// Return the most relevant paths in score order.
    pub fn top_paths(&self, limit: usize) -> Vec<String> {
        self.sorted_entries()
            .into_iter()
            .take(limit)
            .map(|entry| entry.path.clone())
            .collect()
    }

    /// Identify message indices that should be pinned during compaction.
    pub fn pinned_message_indices(&self, messages: &[Message], workspace: &Path) -> Vec<usize> {
        if messages.is_empty() || self.entries.is_empty() {
            return Vec::new();
        }

        let pinned_paths: Vec<&WorkingSetEntry> = self
            .sorted_entries()
            .into_iter()
            .take(self.config.max_pinned_paths)
            .collect();
        if pinned_paths.is_empty() {
            return Vec::new();
        }

        let needles = build_search_needles(&pinned_paths, workspace);
        if needles.is_empty() {
            return Vec::new();
        }

        let mut pinned: Vec<usize> = Vec::new();
        for (idx, message) in messages.iter().enumerate() {
            if message_mentions_any_path(message, &needles, self.config.max_scan_chars) {
                pinned.push(idx);
            }
        }
        pinned
    }

    fn record_candidates(
        &mut self,
        candidates: Vec<String>,
        workspace: &Path,
        source: WorkingSetSource,
    ) {
        if candidates.is_empty() {
            return;
        }

        let workspace_canon = workspace.canonicalize().ok();

        for raw in candidates {
            let Some(normalized) = normalize_candidate(&raw) else {
                continue;
            };
            let Some((rel, exists, is_dir)) =
                relativize_candidate(&normalized, workspace, workspace_canon.as_deref())
            else {
                continue;
            };
            self.record_path(rel, exists, is_dir, source);
        }

        self.prune();
    }

    fn record_path(&mut self, rel: String, exists: bool, is_dir: bool, source: WorkingSetSource) {
        match self.entries.get_mut(&rel) {
            Some(entry) => {
                entry.exists |= exists;
                entry.is_dir |= is_dir;
                entry.touches = entry.touches.saturating_add(1);
                entry.last_turn = self.turn;
                entry.last_source = source;
            }
            None => {
                let entry = WorkingSetEntry::new(rel.clone(), exists, is_dir, self.turn, source);
                let _ = self.entries.insert(rel, entry);
            }
        }
    }

    fn prune(&mut self) {
        let max_entries = self.config.max_entries;
        if self.entries.len() <= max_entries {
            return;
        }

        // Rank by score ascending and drop the lowest until within bounds.
        let mut ranked: Vec<(String, i64)> = self
            .entries
            .values()
            .map(|entry| (entry.path.clone(), score_entry(entry, self.turn)))
            .collect();
        ranked.sort_by_key(|a| a.1);

        let to_remove = self.entries.len().saturating_sub(max_entries);
        for (path, _) in ranked.into_iter().take(to_remove) {
            let _ = self.entries.remove(&path);
        }
    }

    fn sorted_entries(&self) -> Vec<&WorkingSetEntry> {
        let mut entries: Vec<&WorkingSetEntry> = self.entries.values().collect();
        entries.sort_by(|a, b| {
            let sb = score_entry(b, self.turn);
            let sa = score_entry(a, self.turn);
            sb.cmp(&sa).then_with(|| a.path.cmp(&b.path))
        });
        entries
    }

    /// Turn-agnostic ordering used when rendering the prompt summary block.
    /// `sorted_entries` mixes in a recency bonus from `self.turn`, so its
    /// output reorders as turns advance even when no new paths are touched —
    /// that movement would cross `max_prompt_entries` boundaries and bust the
    /// KV prefix cache (#280). Compaction pinning still uses the recency-aware
    /// `sorted_entries`; only the prompt-facing surface is stabilised here.
    fn sorted_for_prompt(&self) -> Vec<&WorkingSetEntry> {
        let mut entries: Vec<&WorkingSetEntry> = self.entries.values().collect();
        entries.sort_by(|a, b| b.touches.cmp(&a.touches).then_with(|| a.path.cmp(&b.path)));
        entries
    }
}

fn score_entry(entry: &WorkingSetEntry, current_turn: u64) -> i64 {
    let age = current_turn.saturating_sub(entry.last_turn);
    let recency_bonus = match age {
        0 => 6,
        1 => 4,
        2 => 3,
        3..=5 => 2,
        6..=10 => 1,
        _ => 0,
    };
    i64::from(entry.touches) * 4 + recency_bonus
}

fn normalize_candidate(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_matches(|c: char| {
        matches!(
            c,
            '"' | '\'' | '`' | ',' | ';' | ':' | '(' | ')' | '[' | ']'
        )
    });
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

fn relativize_candidate(
    candidate: &str,
    workspace: &Path,
    workspace_canon: Option<&Path>,
) -> Option<(String, bool, bool)> {
    let candidate_path = Path::new(candidate);

    // Reject obvious URLs and non-paths early.
    if candidate.contains("://") {
        return None;
    }

    let (rel_path, abs_path) = if candidate_path.is_absolute() {
        let within_workspace = workspace_canon
            .map(|ws| candidate_path.starts_with(ws))
            .unwrap_or_else(|| candidate_path.starts_with(workspace));
        if !within_workspace {
            return None;
        }
        let rel = candidate_path.strip_prefix(workspace).ok()?.to_path_buf();
        (rel, candidate_path.to_path_buf())
    } else {
        if starts_with_parent_dir(candidate_path) {
            return None;
        }
        let rel = clean_relative(candidate_path);
        let abs = workspace.join(&rel);
        (rel, abs)
    };

    let metadata = fs::metadata(&abs_path).ok();
    let exists = metadata.is_some();
    let is_dir = metadata
        .as_ref()
        .map(fs::Metadata::is_dir)
        .unwrap_or_else(|| candidate.ends_with('/'));

    let rel_string = path_to_string(&rel_path)?;
    Some((rel_string, exists, is_dir))
}

fn starts_with_parent_dir(path: &Path) -> bool {
    matches!(
        path.components().next(),
        Some(std::path::Component::ParentDir)
    )
}

fn clean_relative(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut parts: Vec<PathBuf> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = parts.pop();
            }
            Component::Normal(p) => parts.push(PathBuf::from(p)),
            Component::RootDir | Component::Prefix(_) => {}
        }
    }
    let mut out = PathBuf::new();
    for part in parts {
        out.push(part);
    }
    out
}

fn path_to_string(path: &Path) -> Option<String> {
    path.as_os_str().to_str().map(|s| s.replace('\\', "/"))
}

fn extract_paths_from_message(message: &Message) -> Vec<String> {
    let mut paths = Vec::new();
    for block in &message.content {
        match block {
            ContentBlock::Text { text, .. } => {
                paths.extend(extract_paths_from_text(text));
            }
            ContentBlock::ToolUse { input, .. } => {
                paths.extend(extract_paths_from_value(input, None));
            }
            ContentBlock::ToolResult { content, .. } => {
                paths.extend(extract_paths_from_text(content));
            }
            ContentBlock::Thinking { .. }
            | ContentBlock::ServerToolUse { .. }
            | ContentBlock::ToolSearchToolResult { .. }
            | ContentBlock::CodeExecutionToolResult { .. } => {}
        }
    }
    paths
}

fn extract_paths_from_value(value: &Value, tool_hint: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    extract_paths_from_value_inner(value, tool_hint, None, &mut out);
    out
}

fn extract_paths_from_value_inner(
    value: &Value,
    tool_hint: Option<&str>,
    key_hint: Option<&str>,
    out: &mut Vec<String>,
) {
    match value {
        Value::String(s) => {
            let key_suggests_path = key_hint.map(key_is_path_like).unwrap_or(false);
            if key_suggests_path || looks_like_path(s) {
                out.extend(extract_paths_from_text(s));
                if key_suggests_path && !s.contains('/') && !s.contains('\\') {
                    out.push(s.to_string());
                }
            } else if tool_hint == Some("exec_shell") && s.len() < 400 {
                out.extend(extract_paths_from_text(s));
            }
        }
        Value::Array(arr) => {
            for item in arr {
                extract_paths_from_value_inner(item, tool_hint, key_hint, out);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                extract_paths_from_value_inner(v, tool_hint, Some(k.as_str()), out);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn key_is_path_like(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    lower.contains("path")
        || lower.contains("file")
        || lower.contains("dir")
        || lower.contains("cwd")
        || lower.contains("workspace")
        || lower.contains("root")
        || lower == "target"
}

fn looks_like_path(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return true;
    }
    match Path::new(trimmed).extension().and_then(OsStr::to_str) {
        Some(ext) => COMMON_EXTENSIONS.contains(&ext),
        None => false,
    }
}

const COMMON_EXTENSIONS: &[&str] = &[
    "rs", "toml", "md", "txt", "json", "yaml", "yml", "ts", "tsx", "js", "jsx", "py", "go", "java",
    "c", "cc", "cpp", "h", "hpp", "sh", "bash", "zsh", "sql", "html", "css", "scss",
];

fn extract_paths_from_text(text: &str) -> Vec<String> {
    if text.trim().is_empty() {
        return Vec::new();
    }

    let re = path_regex();
    re.find_iter(text)
        .map(|m| m.as_str().to_string())
        .filter(|s| looks_like_path(s))
        .collect()
}

fn path_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Path-ish tokens with separators or file extensions.
        Regex::new(
            r#"(?x)
            (?:
                (?:[A-Za-z]:\\)?                # optional Windows drive
                (?:\./|\../|/)?                 # optional leading
                [A-Za-z0-9._-]+
                (?:[/\\][A-Za-z0-9._-]+)+
                (?:\.[A-Za-z0-9]{1,8})?         # optional extension
            )
            |
            (?:
                [A-Za-z0-9._-]+\.[A-Za-z0-9]{1,8}
            )
            "#,
        )
        .expect("path regex should compile")
    })
}

fn truncate_chars(text: &str, max_chars: usize) -> &str {
    if max_chars == 0 {
        return "";
    }
    match text.char_indices().nth(max_chars) {
        Some((idx, _)) => &text[..idx],
        None => text,
    }
}

fn build_search_needles(entries: &[&WorkingSetEntry], workspace: &Path) -> Vec<String> {
    let mut needles: HashSet<String> = HashSet::new();
    for entry in entries {
        let rel = entry.path.clone();
        if rel.is_empty() {
            continue;
        }
        let abs = workspace.join(&rel);
        let abs_str = abs.as_os_str().to_str().map(ToOwned::to_owned);

        let _ = needles.insert(rel.clone());
        if let Some(abs_str) = abs_str {
            let _ = needles.insert(abs_str);
        }
    }
    needles.into_iter().collect()
}

fn message_mentions_any_path(message: &Message, needles: &[String], max_scan_chars: usize) -> bool {
    if needles.is_empty() {
        return false;
    }
    for block in &message.content {
        match block {
            ContentBlock::Text { text, .. } => {
                let snippet = truncate_chars(text, max_scan_chars);
                if contains_any(snippet, needles) {
                    return true;
                }
            }
            ContentBlock::ToolUse { input, .. } => {
                if let Ok(json) = serde_json::to_string(input)
                    && contains_any(&json, needles)
                {
                    return true;
                }
            }
            ContentBlock::ToolResult { content, .. } => {
                let snippet = truncate_chars(content, max_scan_chars);
                if contains_any(snippet, needles) {
                    return true;
                }
            }
            ContentBlock::Thinking { .. }
            | ContentBlock::ServerToolUse { .. }
            | ContentBlock::ToolSearchToolResult { .. }
            | ContentBlock::CodeExecutionToolResult { .. } => {}
        }
    }
    false
}

fn contains_any(text: &str, needles: &[String]) -> bool {
    needles
        .iter()
        .any(|needle| !needle.is_empty() && text.contains(needle))
}

fn summarize_repo_root(workspace: &Path) -> Option<String> {
    let key_files = detect_key_files(workspace);
    let top_dirs = list_top_level_dirs(workspace, 8);

    if key_files.is_empty() && top_dirs.is_empty() {
        return None;
    }

    let mut parts: Vec<String> = Vec::new();
    if !key_files.is_empty() {
        parts.push(format!("Key files: {}", key_files.join(", ")));
    }
    if !top_dirs.is_empty() {
        parts.push(format!("Top-level dirs: {}", top_dirs.join(", ")));
    }
    Some(parts.join("\n"))
}

fn detect_key_files(workspace: &Path) -> Vec<String> {
    const CANDIDATES: &[&str] = &[
        "Cargo.toml",
        "README.md",
        "AGENTS.md",
        "CLAUDE.md",
        "package.json",
        "pyproject.toml",
        "go.mod",
        "Makefile",
    ];

    CANDIDATES
        .iter()
        .filter_map(|name| {
            let path = workspace.join(name);
            if path.exists() {
                Some((*name).to_string())
            } else {
                None
            }
        })
        .collect()
}

fn list_top_level_dirs(workspace: &Path, limit: usize) -> Vec<String> {
    let mut dirs = Vec::new();
    let entries = match fs::read_dir(workspace) {
        Ok(entries) => entries,
        Err(_) => return dirs,
    };

    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };

        if name.starts_with('.') || IGNORED_ROOT_DIRS.contains(&name) {
            continue;
        }

        if let Ok(meta) = entry.metadata()
            && meta.is_dir()
        {
            dirs.push(name.to_string());
        }

        if dirs.len() >= limit {
            break;
        }
    }

    dirs.sort();
    dirs
}

const IGNORED_ROOT_DIRS: &[&str] = &["target", "node_modules", "dist", "build", ".git"];

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_message(role: &str, text: &str) -> Message {
        Message {
            role: role.to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    #[test]
    fn observe_user_message_tracks_paths() {
        let tmp = TempDir::new().expect("tempdir");
        let src = tmp.path().join("src");
        let file = src.join("lib.rs");
        fs::create_dir_all(&src).expect("mkdir");
        fs::write(&file, "pub fn x() {}").expect("write");

        let mut ws = WorkingSet::default();
        ws.observe_user_message("Please check src/lib.rs", tmp.path());

        assert!(ws.entries.contains_key("src/lib.rs"));
        let entry = ws.entries.get("src/lib.rs").expect("entry");
        assert!(entry.exists);
        assert!(!entry.is_dir);
    }

    #[test]
    fn observe_tool_call_extracts_paths_from_input() {
        let tmp = TempDir::new().expect("tempdir");
        let file = tmp.path().join("Cargo.toml");
        fs::write(&file, "[package]\nname = \"x\"").expect("write");

        let mut ws = WorkingSet::default();
        let input = serde_json::json!({ "path": "Cargo.toml" });
        ws.observe_tool_call("read_file", &input, None, tmp.path());

        assert!(ws.entries.contains_key("Cargo.toml"));
    }

    #[test]
    fn pinned_message_indices_respects_working_set() {
        let tmp = TempDir::new().expect("tempdir");
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");
        let file = src.join("main.rs");
        fs::write(&file, "fn main() {}").expect("write");

        let mut ws = WorkingSet::default();
        ws.observe_user_message("Edit src/main.rs", tmp.path());

        let messages = vec![
            make_message("user", "Unrelated text"),
            make_message("assistant", "I will read src/main.rs next."),
            make_message("user", "More unrelated text"),
        ];

        let pinned = ws.pinned_message_indices(&messages, tmp.path());
        assert_eq!(pinned, vec![1]);
    }

    #[test]
    fn summary_block_includes_repo_and_working_set() {
        let tmp = TempDir::new().expect("tempdir");
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"").expect("write");
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");
        fs::write(src.join("lib.rs"), "pub fn x() {}").expect("write");

        let mut ws = WorkingSet::default();
        ws.observe_user_message("src/lib.rs", tmp.path());
        let block = ws.summary_block(tmp.path()).expect("block");

        assert!(block.contains("Repo Working Set"));
        assert!(block.contains("Cargo.toml"));
        assert!(block.contains("src"));
        assert!(block.contains("src/lib.rs"));
    }

    /// #280 regression: `summary_block` must produce byte-identical output
    /// across `next_turn()` advances when no new paths are touched. Prior to
    /// the fix, the rendered lines interpolated `entry.touches` and
    /// `self.turn - entry.last_turn`, both of which drift turn-over-turn even
    /// when the path set is unchanged. The drift busted DeepSeek's KV prefix
    /// cache on every user message because the working-set block lands in the
    /// system prompt before the historical conversation.
    #[test]
    fn summary_block_is_byte_stable_across_next_turn_when_no_new_paths_observed() {
        use crate::test_support::assert_byte_identical;

        let tmp = TempDir::new().expect("tempdir");
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"").expect("write");
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");
        fs::write(src.join("a.rs"), "a").expect("write");
        fs::write(src.join("b.rs"), "b").expect("write");

        let mut ws = WorkingSet::default();
        ws.observe_user_message("Edit src/a.rs and src/b.rs", tmp.path());

        let before = ws.summary_block(tmp.path()).expect("block before");
        ws.next_turn();
        let after = ws.summary_block(tmp.path()).expect("block after");

        assert_byte_identical(
            "summary_block must be stable across next_turn when no new paths touched",
            &before,
            &after,
        );
    }

    /// Companion to the byte-stability test: a fresh path *should* invalidate
    /// the block (the KV cache is allowed to miss when there's genuinely new
    /// signal), so the model still sees newly touched paths after the block
    /// stabilises across no-op turns.
    #[test]
    fn summary_block_changes_when_a_new_path_is_observed() {
        let tmp = TempDir::new().expect("tempdir");
        fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"x\"").expect("write");
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");
        fs::write(src.join("a.rs"), "a").expect("write");
        fs::write(src.join("c.rs"), "c").expect("write");

        let mut ws = WorkingSet::default();
        ws.observe_user_message("src/a.rs", tmp.path());
        let before = ws.summary_block(tmp.path()).expect("block before");

        ws.observe_user_message("src/c.rs", tmp.path());
        let after = ws.summary_block(tmp.path()).expect("block after");

        assert_ne!(before, after, "new path must update the rendered summary");
        assert!(after.contains("src/c.rs"));
    }

    #[test]
    fn extract_paths_from_message_picks_up_tool_results() {
        let msg = Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tool_1".to_string(),
                content: "Changed src/compaction.rs".to_string(),
                is_error: None,
                content_blocks: None,
            }],
        };

        let paths = extract_paths_from_message(&msg);
        assert!(paths.iter().any(|p| p.contains("src/compaction.rs")));
    }

    #[test]
    fn pinning_prefers_high_signal_paths() {
        let tmp = TempDir::new().expect("tempdir");
        fs::create_dir_all(tmp.path().join("src")).expect("mkdir");
        fs::write(tmp.path().join("src/a.rs"), "a").expect("write");
        fs::write(tmp.path().join("src/b.rs"), "b").expect("write");

        let mut ws = WorkingSet::default();
        ws.observe_user_message("src/a.rs", tmp.path());
        ws.observe_tool_call(
            "read_file",
            &serde_json::json!({ "path": "src/a.rs" }),
            Some("src/a.rs"),
            tmp.path(),
        );
        ws.observe_user_message("src/b.rs", tmp.path());

        let a_score = score_entry(ws.entries.get("src/a.rs").expect("a"), ws.turn);
        let b_score = score_entry(ws.entries.get("src/b.rs").expect("b"), ws.turn);
        assert!(a_score >= b_score);
    }

    #[test]
    fn estimate_tokens_is_available_for_future_budgeting() {
        use crate::compaction::estimate_tokens;
        let messages = vec![make_message("user", "src/main.rs")];
        assert!(estimate_tokens(&messages) > 0);
    }

    #[test]
    fn workspace_resolve_respects_cwd_and_workspace() {
        let tmp = TempDir::new().unwrap();

        let sub = tmp.path().join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let bar = sub.join("bar.txt");
        std::fs::write(&bar, "bar").unwrap();

        let nested = tmp.path().join("nested/deep");
        std::fs::create_dir_all(&nested).unwrap();
        let file_md = nested.join("file.md");
        std::fs::write(&file_md, "md").unwrap();

        // Construct with an explicit cwd so the test doesn't race with other
        // tests that mutate the real process cwd.
        let ws = Workspace::with_cwd(tmp.path().to_path_buf(), Some(sub.clone()));

        // #101 repro #1: @bar.txt with cwd=sub MUST resolve via the cwd pass,
        // never to the bogus workspace path tmp/bar.txt (which doesn't exist).
        let res1 = ws.resolve("bar.txt").unwrap();
        assert_eq!(
            res1.canonicalize().unwrap_or(res1.clone()),
            bar.canonicalize().unwrap_or(bar.clone())
        );
        let wrong = tmp.path().join("bar.txt");
        assert_ne!(res1, wrong, "must not have routed to workspace fallback");

        // #101 repro #2: @nested/deep/file.md falls through to workspace root.
        let res2 = ws.resolve("nested/deep/file.md").unwrap();
        assert_eq!(
            res2.canonicalize().unwrap_or(res2),
            file_md.canonicalize().unwrap_or(file_md)
        );
    }

    /// Negative test (#101): a truly missing path returns `Err` with a path
    /// that callers can show to the user as a signal of failure.
    #[test]
    fn workspace_resolve_returns_err_for_truly_missing_path() {
        let tmp = TempDir::new().unwrap();
        let ws = Workspace::with_cwd(tmp.path().to_path_buf(), Some(tmp.path().to_path_buf()));

        let res = ws.resolve("does/not/exist.txt");
        assert!(res.is_err(), "expected Err for missing path, got: {res:?}");
    }

    /// `Workspace::completions` returns workspace-relative entries for files
    /// under the root, and cwd-relative entries when the cwd-only file lives
    /// outside the workspace tree. Honors `.gitignore`.
    #[test]
    fn workspace_completions_walk_surfaces_workspace_and_cwd() {
        let tmp = TempDir::new().unwrap();
        // Two trees: a workspace under `ws/` and a cwd under `cwd/` that is
        // NOT inside the workspace, so the two walks are disjoint and we can
        // assert each branch contributed.
        let ws_root = tmp.path().join("ws");
        let cwd_root = tmp.path().join("cwd");
        std::fs::create_dir_all(&ws_root).unwrap();
        std::fs::create_dir_all(&cwd_root).unwrap();
        std::fs::write(ws_root.join("alpha.txt"), "a").unwrap();
        std::fs::write(cwd_root.join("alphabeta.txt"), "b").unwrap();

        let ws = Workspace::with_cwd(ws_root.clone(), Some(cwd_root.clone()));
        let entries = ws.completions("alpha", 16);
        assert!(
            entries.iter().any(|e| e == "alpha.txt"),
            "expected workspace entry alpha.txt; got: {entries:?}",
        );
        assert!(
            entries.iter().any(|e| e == "alphabeta.txt"),
            "expected cwd entry alphabeta.txt; got: {entries:?}",
        );
    }

    #[test]
    fn workspace_completions_surface_explicit_hidden_and_ignored_paths() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(".gitignore"), ".deepseek/\n.generated/\n").unwrap();
        std::fs::write(
            tmp.path().join(".deepseekignore"),
            ".generated/specs/secrets.env\n",
        )
        .unwrap();
        let deepseek_commands = tmp.path().join(".deepseek").join("commands");
        let generated_specs = tmp.path().join(".generated").join("specs");
        std::fs::create_dir_all(&deepseek_commands).unwrap();
        std::fs::create_dir_all(&generated_specs).unwrap();
        std::fs::write(deepseek_commands.join("start-task.md"), "start").unwrap();
        std::fs::write(generated_specs.join("device-layout.md"), "layout").unwrap();
        std::fs::write(generated_specs.join("secrets.env"), "secret").unwrap();

        let ws = Workspace::with_cwd(tmp.path().to_path_buf(), Some(tmp.path().to_path_buf()));

        let start_entries = ws.completions(".deepseek/commands", 16);
        assert!(
            start_entries
                .iter()
                .any(|e| e == ".deepseek/commands/start-task.md"),
            "expected explicitly addressed hidden command file in completions: {start_entries:?}",
        );

        let generated_entries = ws.completions(".generated/specs", 16);
        assert!(
            generated_entries
                .iter()
                .any(|e| e == ".generated/specs/device-layout.md"),
            "expected explicitly addressed ignored user folder in completions: {generated_entries:?}",
        );
        assert!(
            !generated_entries
                .iter()
                .any(|e| e == ".generated/specs/secrets.env"),
            ".deepseekignore entries must not be reintroduced by local fallback: {generated_entries:?}",
        );
    }

    #[test]
    fn fuzzy_index_resolves_hidden_and_ignored_files_except_deepseekignored() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(".gitignore"), ".generated/\n").unwrap();
        std::fs::write(
            tmp.path().join(".deepseekignore"),
            ".generated/specs/secrets.env\n",
        )
        .unwrap();
        let generated_specs = tmp.path().join(".generated").join("specs");
        std::fs::create_dir_all(&generated_specs).unwrap();
        std::fs::write(generated_specs.join("device-layout.md"), "layout").unwrap();
        std::fs::write(generated_specs.join("secrets.env"), "secret").unwrap();

        let ws = Workspace::with_cwd(tmp.path().to_path_buf(), None);
        let resolved = ws.resolve("device-layout.md").unwrap();

        assert!(resolved.ends_with(".generated/specs/device-layout.md"));
        assert!(
            ws.resolve("secrets.env").is_err(),
            "basename fuzzy resolution must honor .deepseekignore"
        );
        assert!(
            ws.resolve(".generated/specs/secrets.env").is_ok(),
            "exact user-specified paths should still resolve"
        );
    }

    #[test]
    fn fuzzy_index_finds_files_and_directories() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("a/b/target_dir")).unwrap();
        std::fs::write(tmp.path().join("a/b/needle.rs"), "fn main(){}").unwrap();

        let ws = Workspace::with_cwd(tmp.path().to_path_buf(), None);

        // Basename-only mention triggers fuzzy fallback for both files and dirs.
        let f = ws.resolve("needle.rs").unwrap();
        assert!(f.ends_with("a/b/needle.rs"));
        let d = ws.resolve("target_dir").unwrap();
        assert!(d.ends_with("a/b/target_dir"));

        // Index was populated exactly once (subsequent lookups reuse it).
        assert!(ws.file_index.get().is_some());
    }

    /// Regression: `@`-mention completion must discover files inside
    /// `.deepseek/`, `.cursor/`, `.claude/`, `.agents/` even when
    /// those directories are excluded by `.gitignore` (or `.ignore`).
    /// The `discovery_walk_builder` override un-ignores them.
    #[test]
    fn completions_discovers_files_inside_gitignored_dot_dirs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // `.ignore` works even outside a git repo; use it to simulate
        // a project that gitignores its AI-tool dot-directories.
        std::fs::write(
            root.join(".ignore"),
            ".deepseek/\n.cursor/\n.claude/\n.agents/\n",
        )
        .unwrap();

        // Create files inside each dot-dir.
        std::fs::create_dir_all(root.join(".deepseek/commands")).unwrap();
        std::fs::write(root.join(".deepseek/commands/build.md"), "build cmd").unwrap();
        std::fs::create_dir_all(root.join(".cursor/commands")).unwrap();
        std::fs::write(root.join(".cursor/commands/run.md"), "run cmd").unwrap();
        std::fs::create_dir_all(root.join(".claude/commands")).unwrap();
        std::fs::write(root.join(".claude/commands/test.md"), "test cmd").unwrap();
        std::fs::create_dir_all(root.join(".agents/skills/example")).unwrap();
        std::fs::write(
            root.join(".agents/skills/example/SKILL.md"),
            "name: example\n",
        )
        .unwrap();

        let ws = Workspace::with_cwd(root.to_path_buf(), None);

        // Completions should find entries inside the dot-dirs.
        {
            let entries = ws.completions("build", 16);
            assert!(
                entries.iter().any(|e| e.contains("build.md")),
                "expected build.md in completions although .deepseek/ is ignored; got: {entries:?}"
            );
        }
        {
            let entries = ws.completions("run", 16);
            assert!(
                entries.iter().any(|e| e.contains("run.md")),
                "expected run.md from .cursor/; got: {entries:?}"
            );
        }
        {
            let entries = ws.completions("test", 16);
            assert!(
                entries.iter().any(|e| e.contains("test.md")),
                "expected test.md from .claude/; got: {entries:?}"
            );
        }

        // Fuzzy resolution should also work.
        let f = ws.resolve("build.md").unwrap();
        assert!(f.ends_with("build.md"));
        let f2 = ws.resolve("SKILL.md").unwrap();
        assert!(f2.ends_with("SKILL.md"));
    }

    /// Regression: the dot-dir walk must NOT index `.deepseek/snapshots/`,
    /// which is the snapshot side repo that can grow to hundreds of GB.
    /// Indexing it would re-create the same OOM/hang that #1112 was built
    /// to prevent.
    #[test]
    fn dot_dir_walk_excludes_snapshot_side_repo() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // Create a snapshot-like directory tree.
        std::fs::create_dir_all(root.join(".deepseek/snapshots/deadbeef/deadbeef/.git/objects"))
            .unwrap();
        std::fs::write(
            root.join(".deepseek/snapshots/deadbeef/deadbeef/.git/objects/snapshot.pack"),
            b"fake pack data",
        )
        .unwrap();
        // Also create a legitimate file in .deepseek/ that should be found.
        std::fs::create_dir_all(root.join(".deepseek/commands")).unwrap();
        std::fs::write(root.join(".deepseek/commands/build.md"), "build cmd").unwrap();

        let ws = Workspace::with_cwd(root.to_path_buf(), None);

        // Searching for "build" must find build.md.
        let entries = ws.completions("build", 16);
        assert!(
            entries.iter().any(|e| e.contains("build.md")),
            "build.md must still be found; got: {entries:?}"
        );
        // Searching for "snapshot" must NOT return snapshot files.
        let snap_entries = ws.completions("snapshot", 16);
        assert!(
            !snap_entries.iter().any(|e| e.contains("snapshot")),
            "snapshot files must NOT appear in completions; got: {snap_entries:?}"
        );

        // Fuzzy index must also exclude snapshots.
        let f = ws.resolve("build.md").unwrap();
        assert!(f.ends_with("build.md"));
        // snapshot.pack should NOT resolve.
        let result = ws.resolve("snapshot.pack");
        assert!(
            result.is_err(),
            "snapshot.pack must not resolve via fuzzy index"
        );
    }
}
