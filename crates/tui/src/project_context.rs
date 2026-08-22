//! Project context loading for Codewhale.
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
//! Codewhale-specific repo authority/prioritization policy lives separately in
//! `.codewhale/constitution.json` and is rendered as its own higher-authority
//! block. The loaded content is injected into the system prompt to give the
//! agent context about the project's conventions, structure, and requirements.

mod constitution;
mod pack;
mod types;

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

pub(crate) use self::constitution::{RepoLawAction, RepoLawRule, load_repo_law_rules};
use self::constitution::{load_repo_constitution_block, repo_constitution_candidate_paths};
use self::pack::generate_bounded_project_overview;
pub use self::pack::generate_project_context_pack;
pub use self::types::ProjectContext;
use self::types::ProjectContextError;

/// Names of project context files to look for, in priority order.
///
/// `AGENTS.md` is the canonical cross-agent project-instructions file.
/// `WHALE.md` is no longer an active context surface; when present, Codewhale
/// reports a migration warning but ignores it. Codewhale-specific repo
/// authority now lives in `.codewhale/constitution.json`, not a bespoke
/// markdown file. `CLAUDE.md` and the `*/instructions.md` variants are
/// read-only compatibility fallbacks; Codewhale never creates or recommends
/// them.
const PROJECT_CONTEXT_FILES: &[&str] = &[
    "AGENTS.md",
    ".claude/instructions.md",
    "CLAUDE.md",
    ".codewhale/instructions.md",
    ".deepseek/instructions.md",
];

/// A foreign agent's instruction format. These are read only when the user
/// opts in by name, because a file written as law for a different tool is not
/// automatically law for this one — and because silently treating a file the
/// user never pointed at us as standing authority is an injection surface,
/// not a convenience.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ForeignInstructionFormat {
    Claude,
    Cursor,
    Cline,
    Windsurf,
    Gemini,
    Copilot,
    Muse,
}

impl ForeignInstructionFormat {
    pub(crate) const ALL: &'static [Self] = &[
        Self::Claude,
        Self::Cursor,
        Self::Cline,
        Self::Windsurf,
        Self::Gemini,
        Self::Copilot,
        Self::Muse,
    ];

    /// Configuration spelling, as written in `project_instruction_imports`.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Cursor => "cursor",
            Self::Cline => "cline",
            Self::Windsurf => "windsurf",
            Self::Gemini => "gemini",
            Self::Copilot => "copilot",
            Self::Muse => "muse",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        let value = value.trim().to_ascii_lowercase();
        Self::ALL.iter().copied().find(|f| f.key() == value)
    }

    /// Workspace-relative instruction files this format contributes.
    fn context_files(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &[".claude/instructions.md", "CLAUDE.md"],
            // The remaining formats are imported through the bounded-fragment
            // loader in `codewhale_core::fragments`, not through this chain.
            _ => &[],
        }
    }

    /// Workspace-relative rules directories this format contributes.
    fn rules_dirs(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &[".claude/rules"],
            _ => &[],
        }
    }

    /// Candidates handled by the bounded-fragment loader in
    /// `codewhale_core::fragments` rather than by the instruction chain.
    fn fragment_candidates(self) -> &'static [&'static str] {
        match self {
            Self::Cursor => &[".cursorrules", ".cursor/rules"],
            Self::Cline => &[".clinerules"],
            Self::Windsurf => &[".windsurf/rules"],
            Self::Gemini => &[".gemini"],
            Self::Copilot => &[".github/copilot-instructions.md"],
            Self::Muse => &[".github/muse-instructions.md"],
            // Claude's files are read by the instruction chain above, so
            // importing them here too would inject the same bytes twice.
            Self::Claude => &[],
        }
    }
}

/// The set of foreign formats the user has explicitly opted into.
///
/// Resolved from config once and read by the loader. Empty by default: a fresh
/// checkout containing a `CLAUDE.md` written for another tool contributes
/// nothing to Codewhale's standing instructions until someone says so.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForeignInstructionImports {
    enabled: std::collections::BTreeSet<&'static str>,
}

impl ForeignInstructionImports {
    #[cfg(test)]
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Parse configured names, returning the set plus any unrecognized names
    /// so the caller can warn instead of silently ignoring a typo.
    #[must_use]
    pub fn from_config(values: &[String]) -> (Self, Vec<String>) {
        let mut enabled = std::collections::BTreeSet::new();
        let mut unknown = Vec::new();
        for value in values {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.eq_ignore_ascii_case("all") {
                enabled.extend(ForeignInstructionFormat::ALL.iter().map(|f| f.key()));
                continue;
            }
            match ForeignInstructionFormat::parse(trimmed) {
                Some(format) => {
                    enabled.insert(format.key());
                }
                None => unknown.push(trimmed.to_string()),
            }
        }
        (Self { enabled }, unknown)
    }

    #[must_use]
    pub(crate) fn is_enabled(&self, format: ForeignInstructionFormat) -> bool {
        self.enabled.contains(format.key())
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.enabled.is_empty()
    }

    /// Enabled format keys, for provenance and diagnostics.
    #[cfg(test)]
    #[must_use]
    pub fn keys(&self) -> Vec<&'static str> {
        self.enabled.iter().copied().collect()
    }
}

/// Foreign fragment candidates enabled by the active opt-in set.
///
/// `.agents/AGENTS.md` is always included: it is the cross-agent `AGENTS.md`
/// standard Codewhale already follows, not another vendor's format.
#[must_use]
pub(crate) fn active_fragment_candidates() -> Vec<&'static str> {
    let imports = foreign_instruction_imports();
    fragment_candidates_for(&imports)
}

#[must_use]
pub(crate) fn fragment_candidates_for(imports: &ForeignInstructionImports) -> Vec<&'static str> {
    let mut candidates: Vec<&'static str> = vec![".agents/AGENTS.md"];
    for format in ForeignInstructionFormat::ALL {
        if imports.is_enabled(*format) {
            candidates.extend(format.fragment_candidates());
        }
    }
    candidates
}

static FOREIGN_IMPORTS: std::sync::RwLock<Option<ForeignInstructionImports>> =
    std::sync::RwLock::new(None);

/// Install the resolved opt-in set. Called once while config is applied.
pub fn set_foreign_instruction_imports(imports: ForeignInstructionImports) {
    if let Ok(mut guard) = FOREIGN_IMPORTS.write() {
        *guard = Some(imports);
    }
    crate::project_context_cache::clear();
}

/// The active opt-in set. Defaults to "import nothing".
#[must_use]
pub(crate) fn foreign_instruction_imports() -> ForeignInstructionImports {
    FOREIGN_IMPORTS
        .read()
        .ok()
        .and_then(|guard| guard.clone())
        .unwrap_or_default()
}

/// Instruction file candidates in priority order for the active opt-in set.
fn context_files_for(imports: &ForeignInstructionImports) -> Vec<&'static str> {
    // Order is priority: `load_dir_instructions` takes the first match in a
    // directory. AGENTS.md leads, then Codewhale's own files, and only then
    // anything imported from another tool — an imported CLAUDE.md must never
    // outrank .codewhale/instructions.md the way the old flat list let it.
    let mut files: Vec<&'static str> = vec![
        "AGENTS.md",
        ".codewhale/instructions.md",
        ".deepseek/instructions.md",
    ];
    for format in ForeignInstructionFormat::ALL {
        if imports.is_enabled(*format) {
            files.extend(format.context_files());
        }
    }
    files
}

/// Rules directories for the active opt-in set.
fn rules_dirs_for(imports: &ForeignInstructionImports) -> Vec<&'static str> {
    let mut dirs: Vec<&'static str> = vec![".codewhale/rules"];
    for format in ForeignInstructionFormat::ALL {
        if imports.is_enabled(*format) {
            dirs.extend(format.rules_dirs());
        }
    }
    dirs
}

/// Foreign instruction files that exist in the workspace but were not
/// imported, so the user can discover the opt-in instead of wondering why
/// their `CLAUDE.md` is being ignored.
fn unimported_foreign_warnings(
    workspace: &Path,
    imports: &ForeignInstructionImports,
) -> Vec<String> {
    let mut seen: Vec<&'static str> = Vec::new();
    for format in ForeignInstructionFormat::ALL {
        if imports.is_enabled(*format) {
            continue;
        }
        let present = format
            .context_files()
            .iter()
            .chain(format.rules_dirs().iter())
            .any(|relative| workspace.join(relative).exists());
        if present {
            seen.push(format.key());
        }
    }
    if seen.is_empty() {
        return Vec::new();
    }
    vec![format!(
        "Found instruction files for {} in this workspace; they are not loaded.          Codewhale reads AGENTS.md and its own instruction files by default.          To import them, set project_instruction_imports = [{}] in config.",
        seen.join(", "),
        seen.iter()
            .map(|key| format!("\"{key}\""))
            .collect::<Vec<_>>()
            .join(", ")
    )]
}

/// Rules directories auto-discovered at workspace level, in priority order.
/// `.codewhale/rules/` is Codewhale-native; `.claude/rules/` is Claude compatibility.
/// All `.md` files in these directories are loaded as project rules in filename order.
/// Security model: same trust class as AGENTS.md — workspace-contained content only,
/// no absolute-path escape. Does not require #417 project-config relaxation.
const RULES_DIRS: &[&str] = &[".codewhale/rules", ".claude/rules"];

/// File name of the deprecated Codewhale-native instructions file.
const DEPRECATED_WHALE_FILENAME: &str = "WHALE.md";

/// Warning surfaced when an ignored `WHALE.md` is present.
const WHALE_IGNORED_WARNING: &str = "WHALE.md is ignored; move project instructions to AGENTS.md, or Codewhale-specific authority policy to .codewhale/constitution.json.";

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

/// One aggregate budget for everything that reaches the model as project
/// instruction authority: the repository-root → workspace instruction chain,
/// the global fallback layer merged into it, the assembled rules block, and
/// any opted-in foreign-agent rule files.
///
/// Previously each of those had its own cap — 200 KiB for the chain, 500 KiB
/// for the rules block, 40 KiB for the imported-fragment path — so a workspace
/// could put roughly three quarters of a megabyte of standing instructions in
/// front of the model before skills, memory, history, or tool schemas were
/// counted, and no single number described the ceiling. One budget, checked
/// once, is the number that can actually be reasoned about.
///
/// Authority decides what survives: `instructions` claims the budget first and
/// is trimmed from the *front* (the broadest scope) so the nearest-scope file
/// is the last thing dropped; the rules block takes what is left. Both trims
/// leave an explicit marker in the text.
pub(crate) const MAX_PROJECT_INSTRUCTION_BYTES: usize = 48 * 1024; // 48 KiB

/// Maximum number of rule files loaded per rules directory.
/// Prevents a project from silently injecting hundreds of rule files.
const MAX_RULES_FILES: usize = 50;

const CHAIN_TRUNCATION_MARKER: &str =
    "[…broader-scope project instructions dropped at the aggregate budget…]\n\n";
const RULES_TRUNCATION_MARKER: &str = "\n\n[…rules block truncated at the aggregate budget…]";

/// Trim `text` to at most `max` bytes, keeping the tail and marking the cut.
fn keep_tail_within(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let keep = max.saturating_sub(CHAIN_TRUNCATION_MARKER.len());
    if keep == 0 {
        return CHAIN_TRUNCATION_MARKER.to_string();
    }
    let mut start = text.len() - keep;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    format!("{CHAIN_TRUNCATION_MARKER}{}", &text[start..])
}

/// Trim `text` to at most `max` bytes, keeping the head and marking the cut.
fn keep_head_within(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let keep = max.saturating_sub(RULES_TRUNCATION_MARKER.len());
    if keep == 0 {
        return String::new();
    }
    let mut end = keep;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{RULES_TRUNCATION_MARKER}", &text[..end])
}

/// Apply the single aggregate project-instruction budget.
///
/// Called once, after every source has been assembled, so the ceiling holds
/// across the chain, the global layer, and the rules block together rather
/// than per-loader. Nothing here re-reads the filesystem.
pub(crate) fn enforce_project_instruction_budget(ctx: &mut ProjectContext) {
    let instructions_len = ctx.instructions.as_ref().map_or(0, String::len);
    let rules_len = ctx.rules_block.as_ref().map_or(0, String::len);
    if instructions_len + rules_len <= MAX_PROJECT_INSTRUCTION_BYTES {
        return;
    }

    // Instructions are the higher authority and claim the budget first.
    if instructions_len > MAX_PROJECT_INSTRUCTION_BYTES
        && let Some(text) = ctx.instructions.as_ref()
    {
        let trimmed = keep_tail_within(text, MAX_PROJECT_INSTRUCTION_BYTES);
        tracing::warn!(
            target: "project_context",
            was = instructions_len,
            now = trimmed.len(),
            cap = MAX_PROJECT_INSTRUCTION_BYTES,
            "Dropping broadest-scope project instructions at the aggregate budget"
        );
        ctx.instructions = Some(trimmed);
    }

    let used = ctx.instructions.as_ref().map_or(0, String::len);
    let remaining = MAX_PROJECT_INSTRUCTION_BYTES.saturating_sub(used);
    if rules_len > remaining
        && let Some(text) = ctx.rules_block.as_ref()
    {
        let trimmed = keep_head_within(text, remaining);
        tracing::warn!(
            target: "project_context",
            was = rules_len,
            now = trimmed.len(),
            remaining,
            "Truncating rules block to the aggregate project-instruction budget"
        );
        ctx.rules_block = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        };
    }
}
/// Load project context from the workspace directory.
///
/// This searches for known project context files and loads the first one found.
/// Convenience wrapper that reads the process-wide opt-in set.
///
/// Production goes through [`load_project_context_with_imports`] so the import
/// set is explicit at the call site; this exists for tests that only care about
/// default behaviour.
#[cfg(test)]
pub fn load_project_context(workspace: &Path) -> ProjectContext {
    load_project_context_with_imports(workspace, &foreign_instruction_imports())
}

/// Load workspace project context under an explicit foreign-import set.
///
/// The set is a parameter rather than a global read so a caller — and every
/// test — states which foreign formats are in play instead of depending on
/// process-wide state that parallel tests would race on.
pub(crate) fn load_project_context_with_imports(
    workspace: &Path,
    imports: &ForeignInstructionImports,
) -> ProjectContext {
    let mut ctx = ProjectContext::empty(workspace.to_path_buf());

    // Search for active project context files.
    let (instructions, source_path, warnings) = load_dir_instructions(workspace, imports);
    ctx.instructions = instructions;
    ctx.source_path = source_path;
    ctx.warnings.extend(warnings);

    ctx.warnings
        .extend(ignored_project_whale_warnings(workspace));
    ctx.warnings
        .extend(unimported_foreign_warnings(workspace, imports));

    // Load rules from auto-discovered directories (.codewhale/rules/, .claude/rules/)
    // Each rule file is wrapped in a <project_rule> block and appended after
    // the main instructions content. Security model: same as AGENTS.md —
    // workspace-contained content only, no absolute-path escape.
    let mut rules_content = String::new();
    for rules_dir in rules_dirs_for(imports) {
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
        // No private cap here: `enforce_project_instruction_budget` applies the
        // one aggregate ceiling across instructions and rules together, after
        // every source has been assembled.
        ctx.rules_block = Some(rules_content);
    }

    // Check for trust file
    ctx.is_trusted = check_trust_status(workspace);

    enforce_project_instruction_budget(&mut ctx);

    ctx
}

/// Load the highest-priority instruction file from one directory.
///
/// Returns the content, its path, and any warnings from failed candidates.
/// A directory with no candidate file yields `(None, None, warnings)`.
fn load_dir_instructions(
    dir: &Path,
    imports: &ForeignInstructionImports,
) -> (Option<String>, Option<PathBuf>, Vec<String>) {
    let mut warnings = Vec::new();

    for filename in context_files_for(imports) {
        let file_path = dir.join(filename);

        if context_candidate_exists(&file_path) {
            match load_context_file(&file_path) {
                Ok(content) => {
                    tracing::info!(
                        "Loaded project context from {} ({} bytes)",
                        file_path.display(),
                        content.len()
                    );
                    return (Some(content), Some(file_path), warnings);
                }
                Err(error) => warnings.push(error.to_string()),
            }
        }
    }

    (None, None, warnings)
}

/// Load project context from the containing repository as well.
///
/// Applicable instruction files resolve from the repository root down to the
/// workspace (inclusive) and are assembled in that order under one aggregate
/// byte budget, so wider scopes read first and the workspace keeps the last
/// word. Repository identity comes from the containing checkout itself
/// (Git dir/worktree traversal, [`find_git_root`]) — never from branch names
/// or paths mentioned in conversation — and the chain never crosses the
/// repository boundary. Outside any repository only the workspace itself is
/// searched.
pub fn load_project_context_with_parents(workspace: &Path) -> ProjectContext {
    load_project_context_with_parents_cached_and_home(
        workspace,
        crate::config::effective_home_dir().as_deref(),
    )
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
    let imports = &foreign_instruction_imports();
    load_project_context_with_parents_and_home_imports(workspace, home_dir, imports)
}

fn load_project_context_with_parents_and_home_imports(
    workspace: &Path,
    home_dir: Option<&Path>,
    imports: &ForeignInstructionImports,
) -> ProjectContext {
    let workspace_canonical = canonicalize_workspace_or_keep(workspace);
    let mut ctx = load_project_context_with_imports(&workspace_canonical, imports);

    // Assemble the repository-root → workspace instruction chain. The chain
    // directories come from Git traversal of the containing checkout, so a
    // linked worktree contributes its own root and files above the root —
    // other checkouts, unrelated parents — stay out of scope.
    let chain_dirs = context_chain_dirs(&workspace_canonical, home_dir);
    // `chain_dirs` is ordered root → workspace; the workspace itself is the
    // last entry and was already loaded above.
    let ancestor_dirs = &chain_dirs[..chain_dirs.len().saturating_sub(1)];

    let mut ancestor_docs: Vec<(PathBuf, String)> = Vec::new();
    for dir in ancestor_dirs {
        ctx.warnings.extend(ignored_project_whale_warnings(dir));
        let (content, path, warnings) = load_dir_instructions(dir, imports);
        ctx.warnings.extend(warnings);
        if let (Some(content), Some(path)) = (content, path) {
            ancestor_docs.push((path, content));
        }
    }

    if !ancestor_docs.is_empty() {
        // Assemble root → workspace so the nearest scope has the last word.
        // The byte ceiling is applied once at the end of this function by
        // `enforce_project_instruction_budget`, which trims from the front —
        // i.e. drops the broadest scope first and never strands the workspace's
        // own file behind an exhausted budget the way per-segment accounting
        // did.
        let mut assembled = String::new();

        for (path, content) in &ancestor_docs {
            append_chain_segment(&mut assembled, path, content);
        }

        // The workspace's own file is the most specific link: it reads last,
        // and `source_path` keeps pointing at it so the user knows where the
        // workspace-level override lives.
        if let Some(content) = ctx.instructions.take() {
            let path = ctx
                .source_path
                .clone()
                .unwrap_or_else(|| workspace_canonical.clone());
            append_chain_segment(&mut assembled, &path, &content);
        } else if let Some((path, _)) = ancestor_docs.last() {
            // No workspace-level file: the nearest ancestor is the most
            // specific source.
            ctx.source_path = Some(path.clone());
        }

        ctx.instructions = Some(assembled);
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
    // `.codewhale/` files merely because Codewhale was opened in a directory.
    if !ctx.has_instructions()
        && let Some(generated) = generate_ephemeral_context(workspace)
    {
        ctx.instructions = Some(generated);
        ctx.source_path = None;
    }

    // Load the Codewhale-specific repo authority policy
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

    // The chain and the global layer were both rebuilt above, so re-apply the
    // one ceiling here. Without this the merged global block was entirely
    // unbudgeted: the old per-chain accounting closed before the merge.
    enforce_project_instruction_budget(&mut ctx);

    ctx
}

pub(crate) fn project_context_cache_candidate_paths(
    workspace: &Path,
    home_dir: Option<&Path>,
) -> Vec<PathBuf> {
    let workspace = canonicalize_workspace_or_keep(workspace);
    let mut paths = Vec::new();

    // Enumerate the superset of instruction candidates, not just the ones the
    // active opt-in set loads: a `CLAUDE.md` that is *not* imported still
    // decides whether the "not loaded" warning fires, so its content has to
    // invalidate the cache too. Changing the opt-in set clears the cache
    // outright (`set_foreign_instruction_imports`), so over-enumerating here
    // only ever costs an extra reload.
    for dir in context_chain_dirs(&workspace, home_dir) {
        for filename in PROJECT_CONTEXT_FILES {
            paths.push(dir.join(filename));
        }
        paths.push(dir.join(DEPRECATED_WHALE_FILENAME));
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

/// Find the root of the checkout that contains `dir`.
///
/// Walks upward looking for a `.git` entry, following Git's own discovery
/// semantics: a `.git` directory must hold `HEAD`, and a `.git` file must be
/// a `gitdir:` pointer (a linked worktree). A linked worktree is therefore
/// its own root — the main checkout is reachable only through that pointer,
/// never through directory heuristics, branch names, or paths mentioned in
/// conversation. This is the single source of truth for repository identity
/// in project-context scope resolution.
pub(crate) fn find_git_root(dir: &Path) -> Option<PathBuf> {
    let mut current = dir.to_path_buf();
    loop {
        let git_entry = current.join(".git");
        if is_git_metadata_entry(&git_entry) {
            return Some(current);
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => return None,
        }
    }
}

fn is_git_metadata_entry(path: &Path) -> bool {
    if path.is_dir() {
        return path.join("HEAD").is_file();
    }

    fs::read_to_string(path)
        .map(|content| content.trim_start().starts_with("gitdir:"))
        .unwrap_or(false)
}

/// Directories whose instruction files apply to `workspace`, ordered from the
/// repository root down to the workspace (inclusive).
///
/// Repository identity comes from the containing checkout itself
/// ([`find_git_root`]); the chain never crosses the repository boundary, so
/// sibling checkouts and unrelated parents stay out of scope. Outside any
/// repository only the workspace itself is searched. When `home_dir` is an
/// ancestor it remains an outer boundary the walk never leaves.
fn context_chain_dirs(workspace: &Path, home_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut stop = find_git_root(workspace).unwrap_or_else(|| workspace.to_path_buf());

    if let Some(home) = home_dir {
        let home = canonicalize_workspace_or_keep(home);
        // Clamp only when the walk would otherwise leave the user's home
        // (home sits between the workspace and the repository root).
        if workspace.starts_with(&home) && home.starts_with(&stop) {
            stop = home;
        }
    }

    let mut dirs = Vec::new();
    let mut cursor = workspace.to_path_buf();
    loop {
        dirs.push(cursor.clone());
        if cursor == stop {
            break;
        }
        match cursor.parent() {
            Some(parent) if parent != cursor => cursor = parent.to_path_buf(),
            _ => break,
        }
    }
    dirs.reverse();
    dirs
}

/// Append one chain segment to the assembled instruction text.
///
/// The first segment is the file's raw content (a single-file chain stays
/// byte-identical to a plain load); every later segment is prefixed with a
/// provenance label so the model can tell the scopes apart, wider scopes
/// first and the workspace last.
fn append_chain_segment(assembled: &mut String, path: &Path, content: &str) {
    if !assembled.is_empty() {
        assembled.push_str(&format!(
            "\n\n<!-- scoped instructions: {} (overrides wider scopes where they conflict) -->\n",
            path.display()
        ));
    }
    assembled.push_str(content);
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
         > This context was generated in memory by Codewhale.\n\
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

This file provides guidance to AI agents (Codewhale, Claude Code, etc.) when working with code in this repository.

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
        let home = tempdir().expect("home tempdir");

        // Create a nested structure
        let subdir = tmp.path().join("subproject");
        fs::create_dir(&subdir).expect("mkdir");

        // Put AGENTS.md in parent
        fs::write(tmp.path().join("AGENTS.md"), "Parent instructions").expect("write");
        // Also create a real .git marker to make the parent the repo root
        let git_dir = tmp.path().join(".git");
        fs::create_dir(&git_dir).expect("mkdir .git");
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").expect("write HEAD");

        // Load from subdir should find parent's AGENTS.md
        let ctx = load_project_context_with_parents_and_home(&subdir, Some(home.path()));

        assert!(ctx.has_instructions());
        assert!(
            ctx.instructions
                .as_ref()
                .unwrap()
                .contains("Parent instructions")
        );
    }

    #[test]
    fn parent_search_stops_at_the_repository_root() {
        let tmp = tempdir().expect("tempdir");
        let home = tempdir().expect("home tempdir");

        // AGENTS.md exists above the repository root. It belongs to another
        // scope (often another checkout) and must not leak into this one.
        fs::write(tmp.path().join("AGENTS.md"), "Organization instructions").expect("write");

        // Mark repository root one level below.
        let repo_root = tmp.path().join("repo");
        fs::create_dir(&repo_root).expect("mkdir repo");
        let git_dir = repo_root.join(".git");
        fs::create_dir(&git_dir).expect("mkdir .git");
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").expect("write HEAD");

        let workspace = repo_root.join("apps").join("client");
        fs::create_dir_all(&workspace).expect("mkdir workspace");

        let ctx = load_project_context_with_parents_and_home(&workspace, Some(home.path()));
        assert!(
            !ctx.instructions
                .as_deref()
                .is_some_and(|text| text.contains("Organization instructions")),
            "instruction files above the repository root must not be loaded: {:?}",
            ctx.instructions
        );
        assert_eq!(
            ctx.source_path, None,
            "no in-repo instruction file exists, so no source may be claimed"
        );
        assert!(
            !project_context_cache_candidate_paths(&workspace, Some(home.path()))
                .iter()
                .any(|candidate| candidate == &tmp.path().join("AGENTS.md")),
            "cache candidates must respect the repository boundary too"
        );
    }

    #[test]
    fn instruction_chain_assembles_worktree_root_to_cwd_in_order_within_budget() {
        let tmp = tempdir().expect("tempdir");
        let home = tempdir().expect("home tempdir");

        // A main checkout with its own AGENTS.md — it must stay out of the
        // nested worktree's instruction chain.
        let main = tmp.path().join("main-checkout");
        fs::create_dir_all(main.join(".git")).expect("mkdir main .git");
        fs::write(main.join(".git").join("HEAD"), "ref: refs/heads/main\n")
            .expect("write main HEAD");
        fs::write(main.join("AGENTS.md"), "MAIN-CHECKOUT-ONLY instructions")
            .expect("write main agents");

        // A linked worktree: `.git` is a `gitdir:` pointer file, so the
        // worktree is its own repository root for instruction assembly.
        let lane = tmp.path().join("worktrees").join("lane");
        fs::create_dir_all(&lane).expect("mkdir lane");
        fs::write(
            lane.join(".git"),
            format!("gitdir: {}/.git/worktrees/lane\n", main.display()),
        )
        .expect("write lane gitdir pointer");
        fs::write(lane.join("AGENTS.md"), "WORKTREE-ROOT instructions").expect("write lane agents");

        let nested = lane.join("crates").join("tui");
        fs::create_dir_all(&nested).expect("mkdir nested");
        fs::write(nested.join("AGENTS.md"), "NESTED-DIR instructions")
            .expect("write nested agents");

        let ctx = load_project_context_with_parents_and_home(&nested, Some(home.path()));

        let instructions = ctx.instructions.as_deref().unwrap_or("");
        assert!(
            instructions.contains("WORKTREE-ROOT instructions")
                && instructions.contains("NESTED-DIR instructions"),
            "worktree root and nested AGENTS.md must both assemble:\n{instructions}"
        );
        let root_at = instructions
            .find("WORKTREE-ROOT instructions")
            .expect("root");
        let nested_at = instructions
            .find("NESTED-DIR instructions")
            .expect("nested");
        assert!(
            root_at < nested_at,
            "repository root must read before the current directory (root={root_at}, nested={nested_at})"
        );
        assert!(
            !instructions.contains("MAIN-CHECKOUT-ONLY instructions"),
            "the main checkout's file is outside the worktree's chain:\n{instructions}"
        );
        let expected_source = fs::canonicalize(&nested)
            .expect("canonicalize nested")
            .join("AGENTS.md");
        assert_eq!(
            ctx.source_path.as_deref(),
            Some(expected_source.as_path()),
            "source_path points at the most specific (current-directory) file"
        );
        assert!(
            instructions.len() <= MAX_PROJECT_INSTRUCTION_BYTES,
            "assembled chain must stay within the aggregate budget ({} > {})",
            instructions.len(),
            MAX_PROJECT_INSTRUCTION_BYTES
        );
    }

    #[test]
    fn directory_outside_any_repository_loads_no_instruction_chain() {
        let tmp = tempdir().expect("tempdir");
        let home = tempdir().expect("home tempdir");

        // No `.git` anywhere: the parent's AGENTS.md is outside any chain.
        fs::write(
            tmp.path().join("AGENTS.md"),
            "PARENT-OUTSIDE-REPO instructions",
        )
        .expect("write parent agents");
        let child = tmp.path().join("project");
        fs::create_dir_all(&child).expect("mkdir child");
        fs::write(child.join("AGENTS.md"), "CHILD-ONLY instructions").expect("write child agents");

        let ctx = load_project_context_with_parents_and_home(&child, Some(home.path()));

        let instructions = ctx.instructions.as_deref().unwrap_or("");
        assert!(
            instructions.contains("CHILD-ONLY instructions"),
            "the workspace's own file still loads outside a repository:\n{instructions}"
        );
        assert!(
            !instructions.contains("PARENT-OUTSIDE-REPO instructions"),
            "no ancestor chain may be assembled outside a repository:\n{instructions}"
        );
        let expected_source = fs::canonicalize(&child)
            .expect("canonicalize child")
            .join("AGENTS.md");
        assert_eq!(ctx.source_path.as_deref(), Some(expected_source.as_path()));
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

        // Boundedness is a structural contract below; wall-clock time depends
        // on host load and is not a reliable assertion in the full suite.
        let ctx = load_project_context_with_parents_and_home(workspace.path(), Some(home.path()));
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
    fn explicit_home_bounds_parent_search_without_process_environment() {
        let home = tempdir().expect("home tempdir");
        fs::write(
            home.path().join("AGENTS.md"),
            "must not be loaded as project context",
        )
        .expect("write home AGENTS.md");
        let workspace = home.path().join("projects").join("demo");
        fs::create_dir_all(&workspace).expect("mkdir workspace");

        let ctx = load_project_context_with_parents_and_home(&workspace, Some(home.path()));

        assert_eq!(
            ctx.source_path, None,
            "the explicit home is the parent-search boundary, not project context"
        );
        assert!(
            ctx.instructions
                .as_deref()
                .is_some_and(|text| text.contains("Project Context (Auto-generated, ephemeral)")),
            "expected generated context, got {:?}",
            ctx.instructions
        );
        assert!(
            project_context_cache_candidate_paths(&workspace, Some(home.path()))
                .iter()
                .all(|candidate| candidate != &home.path().join("AGENTS.md")),
            "cache candidates must use the same explicit parent-search boundary"
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
        fs::write(&codewhale_agents, "Codewhale-specific instructions")
            .expect("write codewhale agents");

        let agents_dir = home.path().join(".agents");
        fs::create_dir(&agents_dir).expect("mkdir .agents");
        fs::write(agents_dir.join("AGENTS.md"), "Vendor-neutral instructions")
            .expect("write vendor-neutral agents");

        let ctx = load_project_context_with_parents_and_home(workspace.path(), Some(home.path()));

        assert!(ctx.has_instructions());
        let instructions = ctx.instructions.as_ref().unwrap();
        assert!(
            instructions.contains("Codewhale-specific instructions"),
            "Codewhale-specific global file should win:\n{instructions}"
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
        assert_eq!(
            ctx.source_path,
            Some(canonicalize_workspace_or_keep(workspace.path()).join("AGENTS.md"))
        );
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
    fn claude_rules_load_only_when_explicitly_imported() {
        let tmp = tempdir().expect("tempdir");
        let rules_dir = tmp.path().join(".claude/rules");
        fs::create_dir_all(&rules_dir).expect("mkdir rules");
        fs::write(rules_dir.join("style.md"), "Use tabs").expect("write");

        // Default: another tool's rules directory is not Codewhale's law.
        let ctx = load_project_context_with_imports(tmp.path(), &ForeignInstructionImports::none());
        assert!(
            ctx.rules_block.is_none(),
            "an un-imported .claude/rules must contribute nothing: {:?}",
            ctx.rules_block
        );
        assert!(
            ctx.warnings
                .iter()
                .any(|w| w.contains("claude") && w.contains("project_instruction_imports")),
            "the user must be told the files exist and how to import them: {:?}",
            ctx.warnings
        );

        // Opted in by name: loaded, and ranked after Codewhale's own.
        let (imports, unknown) = ForeignInstructionImports::from_config(&["claude".to_string()]);
        assert!(unknown.is_empty());
        let ctx = load_project_context_with_imports(tmp.path(), &imports);
        let rules = ctx.rules_block.as_ref().expect("rules should be loaded");
        assert!(rules.contains("Use tabs"), "expected .claude/rules/ import");
    }

    #[test]
    fn foreign_instruction_imports_parse_names_and_report_typos() {
        let (imports, unknown) = ForeignInstructionImports::from_config(&[]);
        assert!(imports.is_empty(), "default imports nothing");
        assert!(unknown.is_empty());

        let (imports, unknown) = ForeignInstructionImports::from_config(&[
            "Claude".to_string(),
            " cursor ".to_string(),
            "clawed".to_string(),
        ]);
        assert!(imports.is_enabled(ForeignInstructionFormat::Claude));
        assert!(imports.is_enabled(ForeignInstructionFormat::Cursor));
        assert!(!imports.is_enabled(ForeignInstructionFormat::Gemini));
        assert_eq!(unknown, vec!["clawed".to_string()], "typos are reported");

        let (all, _) = ForeignInstructionImports::from_config(&["all".to_string()]);
        for format in ForeignInstructionFormat::ALL {
            assert!(
                all.is_enabled(*format),
                "{} missing from `all`",
                format.key()
            );
        }
        assert_eq!(all.keys().len(), ForeignInstructionFormat::ALL.len());
    }

    #[test]
    fn codewhale_instructions_outrank_an_imported_claude_file() {
        // On the previous default list CLAUDE.md sat at rank 3 and
        // .codewhale/instructions.md at rank 4, so another tool's file won.
        let tmp = tempdir().expect("tempdir");
        fs::create_dir_all(tmp.path().join(".codewhale")).expect("mkdir codewhale");
        fs::write(
            tmp.path().join(".codewhale/instructions.md"),
            "CODEWHALE-OWN",
        )
        .expect("write codewhale");
        fs::write(tmp.path().join("CLAUDE.md"), "CLAUDE-FILE").expect("write claude");

        let (imports, _) = ForeignInstructionImports::from_config(&["claude".to_string()]);
        let files = context_files_for(&imports);
        let cw = files
            .iter()
            .position(|f| *f == ".codewhale/instructions.md")
            .expect("codewhale file in list");
        let cl = files
            .iter()
            .position(|f| *f == "CLAUDE.md")
            .expect("claude file in list");
        assert!(
            cl > cw,
            "Codewhale's own instruction file must outrank an imported CLAUDE.md: {files:?}"
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

        let (imports, _) = ForeignInstructionImports::from_config(&["claude".to_string()]);
        let ctx = load_project_context_with_imports(tmp.path(), &imports);
        let rules = ctx.rules_block.as_ref().unwrap();

        assert!(
            rules.contains("codewhale-rule"),
            ".codewhale/rules/ should be loaded"
        );
        assert!(
            rules.contains("claude-rule"),
            "an imported .claude/rules/ should be loaded"
        );
        // .codewhale/rules/ content should precede an imported foreign dir
        let pos_cw = rules.find("codewhale-rule").unwrap();
        let pos_claude = rules.find("claude-rule").unwrap();
        assert!(
            pos_cw < pos_claude,
            ".codewhale/rules/ should precede .claude/rules/"
        );
    }

    #[test]
    fn rules_block_truncated_at_the_aggregate_budget() {
        let tmp = tempdir().expect("tempdir");
        let rules_dir = tmp.path().join(".codewhale/rules");
        fs::create_dir_all(&rules_dir).expect("mkdir rules");

        let per_file = "X".repeat(20 * 1024); // 20 KB each
        for i in 0..30 {
            fs::write(rules_dir.join(format!("rule_{i:04}.md")), &per_file).expect("write");
        }

        let ctx = load_project_context(tmp.path());
        let rules = ctx.rules_block.as_ref().unwrap();

        assert!(
            rules.len() <= MAX_PROJECT_INSTRUCTION_BYTES,
            "rules block should be truncated to the aggregate budget: {} > {}",
            rules.len(),
            MAX_PROJECT_INSTRUCTION_BYTES
        );
        assert!(
            rules.contains("truncated at the aggregate budget"),
            "truncation marker missing:\n{}",
            &rules[rules.len().saturating_sub(200)..]
        );
    }

    #[test]
    fn instructions_and_rules_share_one_aggregate_budget() {
        // The point of the change: three separate caps (200 KiB chain,
        // 500 KiB rules, 40 KiB fragments) meant no single number described
        // how much standing instruction text could precede the conversation.
        let tmp = tempdir().expect("tempdir");
        fs::write(tmp.path().join("AGENTS.md"), "A".repeat(40 * 1024)).expect("write agents");
        let rules_dir = tmp.path().join(".codewhale/rules");
        fs::create_dir_all(&rules_dir).expect("mkdir rules");
        for i in 0..10 {
            fs::write(rules_dir.join(format!("r{i}.md")), "B".repeat(20 * 1024)).expect("write");
        }

        let ctx = load_project_context(tmp.path());
        let total = ctx.instructions.as_ref().map_or(0, String::len)
            + ctx.rules_block.as_ref().map_or(0, String::len);
        assert!(
            total <= MAX_PROJECT_INSTRUCTION_BYTES,
            "instructions + rules must share one ceiling: {total} > {MAX_PROJECT_INSTRUCTION_BYTES}"
        );
        // Instructions are the higher authority and are not starved by rules.
        assert!(
            ctx.instructions
                .as_ref()
                .is_some_and(|i| i.len() > 8 * 1024),
            "the instruction file must keep its claim on the budget"
        );
    }

    #[test]
    fn nearest_scope_survives_when_the_budget_forces_a_cut() {
        // Trimming from the front means the broadest scope is dropped first,
        // so the workspace's own file is the last thing to go. The previous
        // per-segment accounting spent the budget root-first and could strand
        // the most specific file entirely.
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path();
        fs::create_dir_all(root.join(".git")).expect("mkdir git");
        fs::write(root.join(".git").join("HEAD"), "ref: refs/heads/main\n").expect("head");
        fs::write(
            root.join("AGENTS.md"),
            format!("ROOT-MARKER {}", "R".repeat(60 * 1024)),
        )
        .expect("root agents");
        let nested = root.join("crates").join("tui");
        fs::create_dir_all(&nested).expect("mkdir nested");
        fs::write(nested.join("AGENTS.md"), "NEAREST-MARKER stays").expect("nested agents");

        let ctx = load_project_context_with_parents_and_home(&nested, None);
        let instructions = ctx.instructions.as_deref().unwrap_or("");

        assert!(
            instructions.contains("NEAREST-MARKER stays"),
            "the nearest-scope file must survive the cut"
        );
        assert!(
            instructions.contains(CHAIN_TRUNCATION_MARKER.trim()),
            "an explicit marker must record that broader scopes were dropped"
        );
        assert!(
            instructions.len() <= MAX_PROJECT_INSTRUCTION_BYTES,
            "budget not enforced: {}",
            instructions.len()
        );
    }
}
