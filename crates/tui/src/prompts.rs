#![allow(dead_code)]
//! System prompts for different modes.
//!
//! Prompts are assembled from composable layers loaded at compile time:
//!   base.md → personality overlay → mode delta → approval policy
//!
//! This keeps each concern in its own file and makes prompt tuning
//! a single-file operation.

use crate::models::SystemPrompt;
use crate::project_context::{ProjectContext, load_project_context_with_parents};
use crate::tui::app::AppMode;
use crate::tui::approval::ApprovalMode;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Default)]
pub struct PromptSessionContext<'a> {
    pub user_memory_block: Option<&'a str>,
    pub goal_objective: Option<&'a str>,
    pub project_context_pack_enabled: bool,
    /// Resolved BCP-47 locale tag for the `## Environment` block in
    /// the system prompt (e.g. `"en"`, `"zh-Hans"`, `"ja"`). The
    /// caller is responsible for resolving this from `Settings`; no
    /// disk I/O happens inside the prompt builder, so the workspace-
    /// static portion of the system prompt stays cache-friendly.
    pub locale_tag: &'a str,
}

/// Conventional location for the structured session-handoff artifact (#32).
/// A previous session writes it on exit / `/compact`; the next session reads
/// it back on startup and prepends it to the system prompt so a fresh agent
/// doesn't have to re-discover open blockers from scratch.
pub const HANDOFF_RELATIVE_PATH: &str = ".deepseek/handoff.md";

/// Per-file size cap for `instructions = [...]` entries (#454). Mirrors
/// the existing project-context cap in `project_context::load_context_file`
/// so a malicious / oversized include can't blow the prompt budget on
/// its own. Files larger than this are truncated with an `[…elided]`
/// marker rather than skipped entirely so the model still sees the head.
const INSTRUCTIONS_FILE_MAX_BYTES: usize = 100 * 1024;

/// Render a `## Environment` block listing the resolved locale tag,
/// runtime version, host platform, login shell, and current working directory.
///
/// The block is appended to the workspace-static portion of the
/// system prompt (after mode prompt + project context, before
/// configured instructions / skills) so the `## Language` directive
/// in `prompts/base.md` can reference it without the model having to
/// guess from the user's first message. `locale_tag` is resolved by
/// the caller from `Settings` so this function stays I/O-free.
fn render_environment_block(workspace: &Path, locale_tag: &str) -> String {
    let deepseek_version = env!("CARGO_PKG_VERSION");
    let platform = std::env::consts::OS;
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "unknown".to_string());
    let pwd = workspace.display();

    format!(
        "## Environment\n\
         \n\
         - lang: {locale_tag}\n\
         - deepseek_version: {deepseek_version}\n\
         - platform: {platform}\n\
         - shell: {shell}\n\
         - pwd: {pwd}"
    )
}

/// Render the `instructions = [...]` config array as a single
/// system-prompt block (#454). Each path is loaded in declared order;
/// missing files are skipped with a tracing warning so a stale entry
/// in `~/.deepseek/config.toml` doesn't fail the launch. Empty input
/// (or all paths missing) returns `None` so callers append nothing.
fn render_instructions_block(paths: &[PathBuf]) -> Option<String> {
    let mut sections: Vec<String> = Vec::new();
    for path in paths {
        match std::fs::read_to_string(path) {
            Ok(raw) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let body = if trimmed.len() > INSTRUCTIONS_FILE_MAX_BYTES {
                    let head_end = (0..=INSTRUCTIONS_FILE_MAX_BYTES)
                        .rev()
                        .find(|&i| trimmed.is_char_boundary(i))
                        .unwrap_or(0);
                    format!("{}\n[…elided]", &trimmed[..head_end])
                } else {
                    trimmed.to_string()
                };
                sections.push(format!(
                    "<instructions source=\"{}\">\n{}\n</instructions>",
                    path.display(),
                    body
                ));
            }
            Err(err) => {
                tracing::warn!(
                    target: "instructions",
                    ?err,
                    ?path,
                    "skipping unreadable instructions file"
                );
            }
        }
    }
    if sections.is_empty() {
        None
    } else {
        Some(sections.join("\n\n"))
    }
}

/// Read the workspace-local handoff artifact, if present, and format it as a
/// system-prompt block. Returns `None` when the file is absent or empty so
/// callers can keep the default-uncluttered prompt for fresh workspaces.
fn load_handoff_block(workspace: &Path) -> Option<String> {
    let path = workspace.join(HANDOFF_RELATIVE_PATH);
    let raw = std::fs::read_to_string(&path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(format!(
        "## Previous Session Handoff\n\nThe previous session in this workspace left a handoff at `{}`. Consider it the first artifact to read on this turn — open blockers, in-flight changes, and recent decisions live there. Update or rewrite it before exiting if state changes materially.\n\n{}",
        HANDOFF_RELATIVE_PATH, trimmed
    ))
}

// ── Prompt layers loaded at compile time ──────────────────────────────

/// Core: task execution, tool-use rules, output format, toolbox reference,
/// "When NOT to use" guidance, sub-agent sentinel protocol.
pub const BASE_PROMPT: &str = include_str!("prompts/base.md");

/// Personality overlays — voice and tone.
pub const CALM_PERSONALITY: &str = include_str!("prompts/personalities/calm.md");
pub const PLAYFUL_PERSONALITY: &str = include_str!("prompts/personalities/playful.md");

/// Mode deltas — permissions, workflow expectations, mode-specific rules.
pub const AGENT_MODE: &str = include_str!("prompts/modes/agent.md");
pub const PLAN_MODE: &str = include_str!("prompts/modes/plan.md");
pub const YOLO_MODE: &str = include_str!("prompts/modes/yolo.md");

/// Approval-policy overlays — whether tool calls are auto-approved,
/// require confirmation, or are blocked.
pub const AUTO_APPROVAL: &str = include_str!("prompts/approvals/auto.md");
pub const SUGGEST_APPROVAL: &str = include_str!("prompts/approvals/suggest.md");
pub const NEVER_APPROVAL: &str = include_str!("prompts/approvals/never.md");

/// Compaction handoff template — written into the system prompt so the
/// model knows the format to use when writing `.deepseek/handoff.md`.
pub const COMPACT_TEMPLATE: &str = include_str!("prompts/compact.md");

// ── Legacy prompt constants (kept for backwards compatibility) ────────

/// Legacy base prompt (agent.txt — now decomposed into base.md + overlays).
/// Still available for callers that haven't migrated to the layered API.
pub const AGENT_PROMPT: &str = include_str!("prompts/agent.txt");

// ── Personality selection ─────────────────────────────────────────────

/// Which personality overlay to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Personality {
    /// Cool, spatial, reserved — the default.
    Calm,
    /// Warm, energetic, playful — alternative for fun mode.
    Playful,
}

impl Personality {
    /// Resolve from the `calm_mode` settings flag.
    /// When `calm_mode` is true → Calm; when false → Playful (future).
    /// For now, always returns Calm — Playful is wired but opt-in.
    #[must_use]
    pub fn from_settings(calm_mode: bool) -> Self {
        if calm_mode {
            Self::Calm
        } else {
            // Future: when playful mode is exposed in settings, return Playful here.
            // For now, calm is the only default.
            Self::Calm
        }
    }

    fn prompt(self) -> &'static str {
        match self {
            Self::Calm => CALM_PERSONALITY,
            Self::Playful => PLAYFUL_PERSONALITY,
        }
    }
}

// ── Composition ───────────────────────────────────────────────────────

fn mode_prompt(mode: AppMode) -> &'static str {
    match mode {
        AppMode::Agent => AGENT_MODE,
        AppMode::Yolo => YOLO_MODE,
        AppMode::Plan => PLAN_MODE,
    }
}

fn default_approval_mode_for_mode(mode: AppMode) -> ApprovalMode {
    match mode {
        AppMode::Agent => ApprovalMode::Suggest,
        AppMode::Yolo => ApprovalMode::Auto,
        AppMode::Plan => ApprovalMode::Never,
    }
}

fn approval_prompt_for_mode(mode: AppMode, approval_mode: ApprovalMode) -> &'static str {
    match mode {
        AppMode::Yolo => AUTO_APPROVAL,
        AppMode::Plan => NEVER_APPROVAL,
        AppMode::Agent => match approval_mode {
            ApprovalMode::Auto => AUTO_APPROVAL,
            ApprovalMode::Suggest => SUGGEST_APPROVAL,
            ApprovalMode::Never => NEVER_APPROVAL,
        },
    }
}

/// Compose the full system prompt in deterministic order:
///   1. base.md        — core identity, toolbox, execution contract
///   2. personality    — voice and tone overlay
///   3. mode delta     — mode-specific permissions and workflow
///   4. approval policy — tool-approval behavior
///
/// Each layer is separated by a blank line for readability in the
/// rendered prompt (the model sees them as contiguous sections).
pub fn compose_prompt(mode: AppMode, personality: Personality) -> String {
    compose_prompt_with_approval(mode, personality, default_approval_mode_for_mode(mode))
}

pub fn compose_prompt_with_approval(
    mode: AppMode,
    personality: Personality,
    approval_mode: ApprovalMode,
) -> String {
    let parts: [&str; 4] = [
        BASE_PROMPT.trim(),
        personality.prompt().trim(),
        mode_prompt(mode).trim(),
        approval_prompt_for_mode(mode, approval_mode).trim(),
    ];

    let mut out =
        String::with_capacity(parts.iter().map(|p| p.len()).sum::<usize>() + (parts.len() - 1) * 2);
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            out.push('\n');
            out.push('\n');
        }
        out.push_str(part);
    }
    out
}

/// Compose for the default personality (Calm).
fn compose_mode_prompt(mode: AppMode) -> String {
    compose_prompt(mode, Personality::Calm)
}

fn compose_mode_prompt_with_approval(mode: AppMode, approval_mode: ApprovalMode) -> String {
    compose_prompt_with_approval(mode, Personality::Calm, approval_mode)
}

// ── Public API ────────────────────────────────────────────────────────

/// Get the system prompt for a specific mode (default Calm personality).
pub fn system_prompt_for_mode(mode: AppMode) -> SystemPrompt {
    SystemPrompt::Text(compose_mode_prompt(mode))
}

/// Get the system prompt for a specific mode with explicit personality.
pub fn system_prompt_for_mode_with_personality(
    mode: AppMode,
    personality: Personality,
) -> SystemPrompt {
    SystemPrompt::Text(compose_prompt(mode, personality))
}

/// Get the system prompt for a specific mode with project context.
pub fn system_prompt_for_mode_with_context(
    mode: AppMode,
    workspace: &Path,
    working_set_summary: Option<&str>,
) -> SystemPrompt {
    system_prompt_for_mode_with_context_and_skills(
        mode,
        workspace,
        working_set_summary,
        None,
        None,
        None,
    )
}

/// Get the system prompt for a specific mode with project and skills context.
///
/// **Volatile-content-last invariant.** Blocks are appended in order from
/// most-static to most-volatile so DeepSeek's KV prefix cache hits the
/// longest possible byte prefix turn-over-turn:
///
///   1. mode prompt (compile-time constant)
///   2. project context / fallback (workspace-static)
///   3. skills block (skills-dir-static)
///   4. `## Context Management` (compile-time constant, Agent/Yolo only)
///   5. compaction handoff template (compile-time constant)
///   6. handoff block — file-backed; rewritten by `/compact` and on exit
///
/// Anything appended after a volatile block forfeits the cache for the rest
/// of the request. New blocks belong above the handoff boundary unless they
/// themselves are turn-volatile. Working-set metadata is now injected into the
/// latest user message as per-turn metadata instead of this system prompt.
pub fn system_prompt_for_mode_with_context_and_skills(
    mode: AppMode,
    workspace: &Path,
    working_set_summary: Option<&str>,
    skills_dir: Option<&Path>,
    instructions: Option<&[PathBuf]>,
    user_memory_block: Option<&str>,
) -> SystemPrompt {
    system_prompt_for_mode_with_context_skills_and_session(
        mode,
        workspace,
        working_set_summary,
        skills_dir,
        instructions,
        PromptSessionContext {
            user_memory_block,
            goal_objective: None,
            project_context_pack_enabled: true,
            locale_tag: "en",
        },
    )
}

pub fn system_prompt_for_mode_with_context_skills_and_session(
    mode: AppMode,
    workspace: &Path,
    _working_set_summary: Option<&str>,
    skills_dir: Option<&Path>,
    instructions: Option<&[PathBuf]>,
    session_context: PromptSessionContext<'_>,
) -> SystemPrompt {
    system_prompt_for_mode_with_context_skills_session_and_approval(
        mode,
        workspace,
        _working_set_summary,
        skills_dir,
        instructions,
        session_context,
        default_approval_mode_for_mode(mode),
    )
}

pub fn system_prompt_for_mode_with_context_skills_session_and_approval(
    mode: AppMode,
    workspace: &Path,
    _working_set_summary: Option<&str>,
    skills_dir: Option<&Path>,
    instructions: Option<&[PathBuf]>,
    session_context: PromptSessionContext<'_>,
    approval_mode: ApprovalMode,
) -> SystemPrompt {
    let mode_prompt = compose_mode_prompt_with_approval(mode, approval_mode);

    // Load project context from workspace
    let project_context = load_project_context_with_parents(workspace);

    // 1–2. Mode prompt + project context.
    // `load_project_context_with_parents` auto-generates .deepseek/instructions.md
    // when no context file exists, so the fallback should always be available.
    let mut full_prompt = if let Some(project_block) = project_context.as_system_block() {
        format!("{}\n\n{}", mode_prompt, project_block)
    } else {
        // Extremely unlikely: context generation failed (e.g. filesystem error).
        // Use mode prompt alone rather than panic.
        tracing::warn!("No project context available and auto-generation failed");
        mode_prompt
    };

    if session_context.project_context_pack_enabled
        && let Some(pack) = crate::project_context::generate_project_context_pack(workspace)
    {
        full_prompt = format!("{full_prompt}\n\n{pack}");
    }

    // 2.25. Environment block — locale, platform, shell, pwd. All
    // four inputs are session-stable (workspace path is fixed for
    // the run; locale is loaded once by the caller; platform/shell
    // come from process env). Inserted above instructions/skills so
    // it remains in the workspace-static cache layer alongside the
    // mode prompt and project context.
    full_prompt = format!(
        "{full_prompt}\n\n{}",
        render_environment_block(workspace, session_context.locale_tag),
    );

    // 2.5a. Configured `instructions = [...]` files (#454). Loaded
    // and concatenated in declared order. Lives above the skills
    // block so it's part of the workspace-static layer that the KV
    // prefix cache can hit, and so per-project overrides apply
    // consistently turn-over-turn.
    if let Some(paths) = instructions
        && let Some(block) = render_instructions_block(paths)
    {
        full_prompt = format!("{full_prompt}\n\n{block}");
    }

    // 2.5b. User memory block (#489). Goes above skills/context-management
    // because it's session-stable: the memory file changes when the user
    // edits it via `/memory` or `# foo` quick-add, but not turn-over-turn.
    if let Some(memory_block) = session_context.user_memory_block
        && !memory_block.trim().is_empty()
    {
        full_prompt = format!("{full_prompt}\n\n{memory_block}");
    }

    if let Some(goal_objective) = session_context.goal_objective
        && !goal_objective.trim().is_empty()
    {
        full_prompt = format!(
            "{full_prompt}\n\n## Current Session Goal\n\n<session_goal>\n{}\n</session_goal>",
            goal_objective.trim()
        );
    }

    // 3. Skills block. #432: walks every candidate workspace
    // skills directory (`.agents/skills`, `skills`,
    // `.opencode/skills`, `.claude/skills`, `.cursor/skills`) plus global
    // `~/.agents/skills` / `~/.deepseek/skills` so skills installed for any
    // AI-tool convention show up in the catalogue. The legacy
    // single-`skills_dir` path is
    // honoured as a fallback for callers that don't supply a
    // workspace-aware view; it falls through to the same merged
    // registry when available.
    let skills_block = crate::skills::render_available_skills_context_for_workspace(workspace)
        .or_else(|| skills_dir.and_then(crate::skills::render_available_skills_context));
    if let Some(block) = skills_block {
        full_prompt = format!("{full_prompt}\n\n{block}");
    }

    // 4. Context Management (Agent / Yolo only).
    if matches!(mode, AppMode::Agent | AppMode::Yolo) {
        full_prompt.push_str(
            "\n\n## Context Management\n\n\
             When the conversation gets long (you'll see a context usage indicator), you can:\n\
             1. Use `/compact` to summarize earlier context and free up space\n\
             2. The system will preserve important information (files you're working on, recent messages, tool results)\n\
             3. After compaction, you'll see a summary of what was discussed and can continue seamlessly\n\n\
             If you notice context is getting long (>80%), proactively suggest using `/compact` to the user.\n\n\
             ### Prompt-cache awareness\n\n\
             DeepSeek caches the longest *byte-stable prefix* of every request and charges roughly 100× less for cache-hit tokens than miss tokens. The system prompt above is layered most-static-first specifically so the prefix stays stable turn-over-turn. To keep cache hits high:\n\
             - **Working set location:** the current repo working set is stored on new user messages inside a `<turn_meta>` block. Treat it as high-priority turn metadata, not as a stable system-prompt section.\n\
             - **Append, don't reorder.** New context goes at the end (latest user / tool messages). Reshuffling earlier messages or rewriting their content invalidates the cache for everything after the change.\n\
             - **Don't paraphrase quoted content.** If you've already read a file, refer to it by path or line range instead of re-quoting it with different formatting.\n\
             - **Use `/compact` as a hard reset, not a tweak.** Compaction is meant for when the cache is already losing — it intentionally rewrites the prefix to a shorter summary. Don't trigger it for small wins.\n\
             - **Read once, refer back.** Re-reading the same file produces a different tool-result envelope than the prior read; it's cheaper to scroll back than to re-fetch.\n\
             - **Footer chip:** the `cache hit %` chip turns red below 40% and yellow below 80%. If it's been red for several turns, that's a signal to consolidate."
        );
    }

    // 5. Compaction handoff template — so the model knows the format to use
    //    when writing `.deepseek/handoff.md` on exit / `/compact`.
    full_prompt.push_str("\n\n");
    full_prompt.push_str(COMPACT_TEMPLATE);

    // ── Volatile-content boundary ─────────────────────────────────────────
    // Everything below drifts mid-session and busts the prefix cache for
    // bytes that follow. Keep new static blocks above this comment.

    // 6. Previous-session handoff (file-backed, rewritten by `/compact`).
    if let Some(handoff_block) = load_handoff_block(workspace) {
        full_prompt = format!("{full_prompt}\n\n{handoff_block}");
    }

    SystemPrompt::Text(full_prompt)
}

/// Build a system prompt with explicit project context
pub fn build_system_prompt(base: &str, project_context: Option<&ProjectContext>) -> SystemPrompt {
    let full_prompt =
        match project_context.and_then(super::project_context::ProjectContext::as_system_block) {
            Some(project_block) => format!("{}\n\n{}", base.trim(), project_block),
            None => base.trim().to_string(),
        };
    SystemPrompt::Text(full_prompt)
}

// ── Legacy functions for backwards compatibility ──────────────────────

pub fn base_system_prompt() -> SystemPrompt {
    SystemPrompt::Text(BASE_PROMPT.trim().to_string())
}

pub fn normal_system_prompt() -> SystemPrompt {
    system_prompt_for_mode(AppMode::Agent)
}

pub fn agent_system_prompt() -> SystemPrompt {
    system_prompt_for_mode(AppMode::Agent)
}

pub fn yolo_system_prompt() -> SystemPrompt {
    system_prompt_for_mode(AppMode::Yolo)
}

pub fn plan_system_prompt() -> SystemPrompt {
    system_prompt_for_mode(AppMode::Plan)
}

#[cfg(test)]
mod tests {
    // Don't assert on prose. If you wouldn't fail a code review for
    // changing the wording, don't fail a test for it.
    use super::*;
    use tempfile::tempdir;

    /// Discriminator unique to the injected handoff block (not present in the
    /// agent prompt's own discussion of the convention).
    const HANDOFF_BLOCK_MARKER: &str = "left a handoff at `.deepseek/handoff.md`";

    #[test]
    fn render_environment_block_lists_supplied_locale_and_workspace() {
        let tmp = tempdir().expect("tempdir");
        let block = render_environment_block(tmp.path(), "zh-Hans");
        assert!(block.starts_with("## Environment"));
        assert!(block.contains("- lang: zh-Hans"));
        assert!(block.contains(&format!(
            "- deepseek_version: {}",
            env!("CARGO_PKG_VERSION")
        )));
        assert!(block.contains(&format!("- pwd: {}", tmp.path().display())));
        assert!(block.contains("- platform:"));
        assert!(block.contains("- shell:"));
    }

    #[test]
    fn environment_block_is_inserted_into_system_prompt() {
        let tmp = tempdir().expect("tempdir");
        let prompt = match system_prompt_for_mode_with_context_skills_and_session(
            AppMode::Agent,
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: None,
                project_context_pack_enabled: true,
                locale_tag: "ja",
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert!(prompt.contains("## Environment"));
        assert!(prompt.contains("- lang: ja"));
        assert!(prompt.contains("- deepseek_version:"));
    }

    #[test]
    fn project_context_pack_can_be_disabled() {
        let tmp = tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("README.md"), "# Pack test").expect("write readme");
        let prompt = match system_prompt_for_mode_with_context_skills_and_session(
            AppMode::Agent,
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: None,
                project_context_pack_enabled: false,
                locale_tag: "en",
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert!(!prompt.contains("<project_context_pack>"));
    }

    #[test]
    fn project_context_pack_is_before_dynamic_tail() {
        let tmp = tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("README.md"), "# Pack test").expect("write readme");
        std::fs::create_dir_all(tmp.path().join(".deepseek")).expect("mkdir");
        std::fs::write(tmp.path().join(".deepseek").join("handoff.md"), "handoff")
            .expect("handoff");
        let prompt = match system_prompt_for_mode_with_context_skills_and_session(
            AppMode::Agent,
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: None,
                project_context_pack_enabled: true,
                locale_tag: "en",
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert!(prompt.contains("<project_context_pack>"));
        assert!(
            prompt.find("<project_context_pack>").expect("pack")
                < prompt.find("## Previous Session Handoff").expect("handoff")
        );
    }

    #[test]
    fn handoff_artifact_is_prepended_to_system_prompt_when_present() {
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path();
        let handoff_dir = workspace.join(".deepseek");
        std::fs::create_dir_all(&handoff_dir).unwrap();
        std::fs::write(
            handoff_dir.join("handoff.md"),
            "# Session handoff — prior\n\n## Active task\nFinish #32.\n\n## Open blockers\n- [ ] write the basic version\n",
        )
        .unwrap();

        let prompt = match system_prompt_for_mode_with_context(AppMode::Agent, workspace, None) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };

        assert!(prompt.contains(HANDOFF_BLOCK_MARKER));
        assert!(prompt.contains("Finish #32."));
        assert!(prompt.contains("write the basic version"));
    }

    #[test]
    fn missing_handoff_does_not_inject_block() {
        let tmp = tempdir().expect("tempdir");
        let prompt = match system_prompt_for_mode_with_context(AppMode::Agent, tmp.path(), None) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert!(!prompt.contains(HANDOFF_BLOCK_MARKER));
    }

    #[test]
    fn empty_handoff_file_does_not_inject_block() {
        let tmp = tempdir().expect("tempdir");
        let dir = tmp.path().join(".deepseek");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("handoff.md"), "   \n\n  ").unwrap();
        let prompt = match system_prompt_for_mode_with_context(AppMode::Agent, tmp.path(), None) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert!(!prompt.contains(HANDOFF_BLOCK_MARKER));
    }

    #[test]
    fn compose_prompt_includes_all_layers() {
        let prompt = compose_prompt(AppMode::Agent, Personality::Calm);
        // Base layer
        assert!(prompt.contains("You are DeepSeek TUI"));
        // Personality layer
        assert!(prompt.contains("Personality: Calm"));
        // Mode layer
        assert!(prompt.contains("Mode: Agent"));
        // Approval layer
        assert!(prompt.contains("Approval Policy: Suggest"));
    }

    /// Gate against shipping a release with a missing CHANGELOG entry — which
    /// is exactly what happened with v0.8.21 / v0.8.22 (entries had to be
    /// backfilled in v0.8.23). Asserts the top-of-file CHANGELOG contains a
    /// `## [X.Y.Z]` heading matching the current `CARGO_PKG_VERSION`. No
    /// hardcoded version string — the test self-updates with the workspace
    /// version bump and only fires when the CHANGELOG is the missing piece.
    ///
    /// Walks up from `CARGO_MANIFEST_DIR` to find `CHANGELOG.md` instead of
    /// assuming a fixed `../../CHANGELOG.md` layout. The workspace root is
    /// the common case, but the walk also tolerates deeper crate layouts and
    /// the packaged-crate case (where the workspace root has been stripped
    /// out): if no `CHANGELOG.md` is reachable, the gate quietly skips
    /// rather than panicking, so consumers running the suite outside the
    /// workspace checkout don't see a spurious failure.
    #[test]
    fn changelog_entry_exists_for_current_package_version() {
        let version = env!("CARGO_PKG_VERSION");
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let Some(changelog_path) = manifest_dir
            .ancestors()
            .map(|dir| dir.join("CHANGELOG.md"))
            .find(|candidate| candidate.is_file())
        else {
            eprintln!(
                "changelog_entry_exists_for_current_package_version: no \
                 CHANGELOG.md found above {} — skipping (this gate only \
                 fires inside a workspace checkout).",
                manifest_dir.display()
            );
            return;
        };

        let contents = std::fs::read_to_string(&changelog_path).unwrap_or_else(|err| {
            panic!(
                "failed to read CHANGELOG.md at {}: {err}",
                changelog_path.display()
            )
        });
        let header = format!("## [{version}]");
        assert!(
            contents.contains(&header),
            "CHANGELOG.md is missing a `{header}` entry for the current package \
             version. Add a release section at the top before tagging — see \
             docs/RELEASE_CHECKLIST.md."
        );
    }

    #[test]
    fn compose_prompt_deterministic_order() {
        let prompt = compose_prompt(AppMode::Yolo, Personality::Calm);
        let base_pos = prompt.find("You are DeepSeek TUI").unwrap();
        let personality_pos = prompt.find("Personality: Calm").unwrap();
        let mode_pos = prompt.find("Mode: YOLO").unwrap();
        let approval_pos = prompt.find("Approval Policy: Auto").unwrap();

        assert!(base_pos < personality_pos);
        assert!(personality_pos < mode_pos);
        assert!(mode_pos < approval_pos);
    }

    #[test]
    fn each_mode_gets_correct_approval() {
        assert!(
            compose_prompt(AppMode::Agent, Personality::Calm).contains("Approval Policy: Suggest")
        );
        assert!(compose_prompt(AppMode::Yolo, Personality::Calm).contains("Approval Policy: Auto"));
        assert!(
            compose_prompt(AppMode::Plan, Personality::Calm).contains("Approval Policy: Never")
        );
    }

    #[test]
    fn agent_prompt_can_reflect_never_approval_policy() {
        let prompt =
            compose_prompt_with_approval(AppMode::Agent, Personality::Calm, ApprovalMode::Never);
        assert!(prompt.contains("Mode: Agent"));
        assert!(prompt.contains("Approval Policy: Never"));
        assert!(prompt.contains("/config approval_mode suggest"));
    }

    #[test]
    fn personality_switches_correctly() {
        let calm = compose_prompt(AppMode::Agent, Personality::Calm);
        let playful = compose_prompt(AppMode::Agent, Personality::Playful);
        assert!(calm.contains("Personality: Calm"));
        assert!(playful.contains("Personality: Playful"));
        assert!(!calm.contains("Personality: Playful"));
    }

    #[test]
    fn compact_template_is_included_in_full_prompt() {
        let tmp = tempdir().expect("tempdir");
        let prompt = match system_prompt_for_mode_with_context(AppMode::Agent, tmp.path(), None) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert!(prompt.contains("## Compaction Handoff"));
        // #429: structured Markdown template. Goal/Constraints/Progress
        // (Done/InProgress/Blocked)/Key Decisions/Next step.
        assert!(prompt.contains("### Goal"));
        assert!(prompt.contains("### Constraints"));
        assert!(prompt.contains("### Progress"));
        assert!(prompt.contains("#### Done"));
        assert!(prompt.contains("#### In Progress"));
        assert!(prompt.contains("#### Blocked"));
        assert!(prompt.contains("### Key Decisions"));
        assert!(prompt.contains("### Next step"));
    }

    #[test]
    fn session_goal_is_injected_above_handoff_tail() {
        let tmp = tempdir().expect("tempdir");
        let prompt = match system_prompt_for_mode_with_context_skills_and_session(
            AppMode::Agent,
            tmp.path(),
            Some("## Repo Working Set\nsrc/lib.rs"),
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: Some("Fix transcript corruption"),
                project_context_pack_enabled: true,
                locale_tag: "en",
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };

        let goal_pos = prompt.find("<session_goal>").expect("goal block");
        let compact_pos = prompt.find("## Compaction Handoff").expect("compact block");

        assert!(prompt.contains("Fix transcript corruption"));
        assert!(goal_pos < compact_pos);
        assert!(!prompt.contains("src/lib.rs"));
    }

    #[test]
    fn empty_session_goal_is_not_injected() {
        let tmp = tempdir().expect("tempdir");
        let prompt = match system_prompt_for_mode_with_context_skills_and_session(
            AppMode::Agent,
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: Some("   "),
                project_context_pack_enabled: true,
                locale_tag: "en",
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };

        assert!(!prompt.contains("<session_goal>"));
        assert!(!prompt.contains("## Current Session Goal"));
    }

    #[test]
    fn tool_selection_guide_avoids_defensive_tool_suppression() {
        let prompt = compose_prompt(AppMode::Agent, Personality::Calm);
        assert!(prompt.contains("Tool Selection Guide"));
        assert!(prompt.contains("Use `agent_result`"));
        assert!(
            !prompt.contains("When NOT to use certain tools"),
            "the system prompt should steer tool choice without training the model to avoid available tools"
        );
        assert!(
            !prompt.contains("Don't reach for"),
            "avoid defensive anti-tool wording in the base prompt"
        );
    }

    /// #588: language-mirroring directive must ship in every mode so
    /// DeepSeek's `reasoning_content` and final reply follow the user's
    /// language. Structural test — wording is not a test concern, but
    /// the cross-cutting commitment of #588 is specifically that the
    /// `reasoning_content` field tracks the user's language (not just
    /// the visible reply); pin that anchor token so a future edit
    /// can't silently weaken the section to a generic "respond in the
    /// user's language" directive while keeping the heading.
    #[test]
    fn language_mirroring_section_present_in_all_modes() {
        for mode in [AppMode::Agent, AppMode::Yolo, AppMode::Plan] {
            let prompt = compose_prompt(mode, Personality::Calm);
            assert!(
                prompt.contains("## Language"),
                "## Language section missing from mode {mode:?}"
            );
            assert!(
                prompt.contains("reasoning_content"),
                "## Language section in {mode:?} must mention `reasoning_content` — \
                 that field name is the structural anchor for the #588 commitment that \
                 internal reasoning, not just the visible reply, follows the user's language"
            );
        }
    }

    #[test]
    fn language_mirroring_prioritizes_latest_user_message_over_locale_default() {
        let prompt = compose_prompt(AppMode::Agent, Personality::Calm);
        assert!(
            prompt.contains("latest user message first"),
            "the language directive must choose the turn language from the user message before \
             falling back to the environment locale"
        );
        assert!(
            prompt.contains("even when the `lang` field in `## Environment` is `en`"),
            "Chinese user text must override an English resolved locale for reasoning_content"
        );
        assert!(
            prompt.contains("Use the `lang` field only when"),
            "environment locale should be an ambiguity fallback, not the primary language source"
        );
    }

    /// #358: rlm guidance was reframed from "first-class" to "specialty
    /// tool" — verify the structural markers are present so a future
    /// change doesn't silently remove the RLM section entirely.
    ///
    /// Don't assert on prose. If you wouldn't fail a code review for
    /// changing the wording, don't fail a test for it.
    #[test]
    fn rlm_specialty_tool_guidance_present() {
        let prompt = compose_prompt(AppMode::Agent, Personality::Calm);
        // Structural: the RLM heading must exist as a section anchor.
        assert!(prompt.contains("RLM — How to Use It"));
        // Structural: the word "rlm" must appear multiple times (tool
        // name, section heading, toolbox reference). Just verify the
        // lowercase form — exact wording is NOT a test concern.
        let rlm_count = prompt.to_lowercase().matches("rlm").count();
        assert!(
            rlm_count >= 5,
            "RLM guidance present: expected >= 5 mentions of 'rlm', got {rlm_count}"
        );
        assert!(
            !prompt.contains("When NOT to use RLM"),
            "RLM guidance should explain fit and verification without telling the model to avoid the tool"
        );
    }

    #[test]
    fn subagent_done_sentinel_section_present() {
        let prompt = compose_prompt(AppMode::Agent, Personality::Calm);
        assert!(prompt.contains("Internal Sub-agent Completion Events"));
        assert!(prompt.contains("<deepseek:subagent.done>"));
        assert!(prompt.contains("not user input"));
        assert!(prompt.contains("Integration protocol"));
        assert!(prompt.contains("Do not tell the user they pasted sentinels"));
    }

    #[test]
    fn preamble_rhythm_section_present() {
        let prompt = compose_prompt(AppMode::Agent, Personality::Calm);
        assert!(prompt.contains("Preamble Rhythm"));
        assert!(prompt.contains("I'll start by reading the module structure"));
    }

    #[test]
    fn legacy_constants_still_available() {
        // Verify the legacy .txt constant still compiles and contains expected content
        assert!(!AGENT_PROMPT.is_empty());
    }

    // ── Cache-prefix stability harness (#263 step 2) ───────────────────────
    //
    // These tests pin the byte-stability invariant required for DeepSeek's
    // KV prefix cache to hit: any prompt-construction surface that ends up
    // in the cached prefix must produce identical bytes given identical
    // inputs across calls.

    use crate::test_support::assert_byte_identical;

    #[test]
    fn compose_prompt_is_byte_stable_across_calls() {
        // Suspect #4 from #263: mode prompt churn within a single mode.
        // Two calls with identical (mode, personality) inputs must produce
        // identical bytes — anything else is a cache buster.
        for mode in [AppMode::Agent, AppMode::Yolo, AppMode::Plan] {
            for personality in [Personality::Calm, Personality::Playful] {
                let a = compose_prompt(mode, personality);
                let b = compose_prompt(mode, personality);
                assert_byte_identical(
                    &format!("compose_prompt(mode={mode:?}, personality={personality:?})"),
                    &a,
                    &b,
                );
            }
        }
    }

    #[test]
    fn system_prompt_for_mode_with_context_is_byte_stable_for_unchanged_workspace() {
        // Same workspace, no working_set / skills churn between calls →
        // identical bytes. This pins the most representative production
        // surface (engine.rs builds the system prompt via this fn or
        // its sibling _and_skills variant on every turn).
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path();

        for mode in [AppMode::Agent, AppMode::Yolo, AppMode::Plan] {
            let a = match system_prompt_for_mode_with_context(mode, workspace, None) {
                SystemPrompt::Text(text) => text,
                SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
            };
            let b = match system_prompt_for_mode_with_context(mode, workspace, None) {
                SystemPrompt::Text(text) => text,
                SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
            };
            assert_byte_identical(
                &format!("system_prompt_for_mode_with_context(mode={mode:?}) on empty workspace"),
                &a,
                &b,
            );
        }
    }

    #[test]
    fn system_prompt_ignores_working_set_summary_argument() {
        // Working-set metadata is now injected into the latest user message
        // per turn. The legacy argument remains for call-site compatibility
        // but must not reintroduce volatile bytes into the system prompt.
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path();
        let summary = "## Repo Working Set\nWorkspace: /tmp/x\n";

        let a = match system_prompt_for_mode_with_context(AppMode::Agent, workspace, Some(summary))
        {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        let b = match system_prompt_for_mode_with_context(AppMode::Agent, workspace, Some(summary))
        {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert_byte_identical(
            "system_prompt_for_mode_with_context with constant working_set summary",
            &a,
            &b,
        );
        assert!(
            !a.contains(summary),
            "summary must not be embedded in system prompt"
        );
    }

    #[test]
    fn system_prompt_with_handoff_file_is_byte_stable_when_file_is_unchanged() {
        // If `.deepseek/handoff.md` hasn't moved between two builds, the
        // rendered prompt must produce identical bytes. The handoff block
        // lands below the static boundary in
        // `system_prompt_for_mode_with_context_and_skills`.
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path();
        let handoff_dir = workspace.join(".deepseek");
        std::fs::create_dir_all(&handoff_dir).unwrap();
        std::fs::write(
            handoff_dir.join("handoff.md"),
            "# Session handoff\n\n## Active task\nFinish #280.\n\n## Open blockers\n- [ ] none\n",
        )
        .unwrap();

        let a = match system_prompt_for_mode_with_context(AppMode::Agent, workspace, None) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        let b = match system_prompt_for_mode_with_context(AppMode::Agent, workspace, None) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert_byte_identical(
            "system_prompt_for_mode_with_context with constant handoff file",
            &a,
            &b,
        );
        assert!(a.contains(HANDOFF_BLOCK_MARKER), "handoff must be embedded");
        assert!(a.contains("Finish #280."), "handoff body must be present");
    }

    #[test]
    fn handoff_appears_after_static_blocks_without_working_set() {
        // Cache-prefix invariant: the handoff block must come after static
        // `## Context Management` and the compaction handoff template
        // (`## Compaction Handoff`). Working-set metadata is per-turn user
        // metadata now, not a system-prompt tail block.
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path();
        let handoff_dir = workspace.join(".deepseek");
        std::fs::create_dir_all(&handoff_dir).unwrap();
        std::fs::write(handoff_dir.join("handoff.md"), "# handoff body\n").unwrap();

        let summary = "## Repo Working Set\nWorkspace: /tmp/x\n";
        let prompt =
            match system_prompt_for_mode_with_context(AppMode::Agent, workspace, Some(summary)) {
                SystemPrompt::Text(text) => text,
                SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
            };

        let context_pos = prompt
            .find("## Context Management")
            .expect("Context Management section present in Agent mode");
        let compact_pos = prompt
            .find("## Compaction Handoff")
            .expect("compaction handoff template present");
        let handoff_pos = prompt
            .find(HANDOFF_BLOCK_MARKER)
            .expect("handoff block present when fixture file exists");
        assert!(
            !prompt.contains("## Repo Working Set"),
            "working-set summary must stay out of the system prompt"
        );

        assert!(
            context_pos < handoff_pos,
            "## Context Management must precede the handoff block"
        );
        assert!(
            compact_pos < handoff_pos,
            "## Compaction Handoff must precede the handoff block"
        );
    }

    #[test]
    fn render_instructions_block_returns_none_for_empty_input() {
        assert!(super::render_instructions_block(&[]).is_none());
    }

    #[test]
    fn render_instructions_block_skips_missing_files_with_warning() {
        let tmp = tempdir().expect("tempdir");
        let real = tmp.path().join("real.md");
        std::fs::write(&real, "real content here").unwrap();
        let bogus = tmp.path().join("does-not-exist.md");

        let block = super::render_instructions_block(&[bogus.clone(), real.clone()])
            .expect("present file should produce a block");
        assert!(block.contains("real content here"));
        assert!(block.contains(&real.display().to_string()));
        // Bogus path is skipped, not rendered.
        assert!(!block.contains(&bogus.display().to_string()));
    }

    #[test]
    fn render_instructions_block_concatenates_in_declared_order() {
        let tmp = tempdir().expect("tempdir");
        let a = tmp.path().join("a.md");
        let b = tmp.path().join("b.md");
        std::fs::write(&a, "ALPHA_MARKER").unwrap();
        std::fs::write(&b, "BRAVO_MARKER").unwrap();

        let block = super::render_instructions_block(&[a, b]).expect("non-empty");
        let alpha_pos = block.find("ALPHA_MARKER").expect("alpha rendered");
        let bravo_pos = block.find("BRAVO_MARKER").expect("bravo rendered");
        assert!(
            alpha_pos < bravo_pos,
            "instructions must concatenate in declared order"
        );
    }

    #[test]
    fn render_instructions_block_skips_empty_files() {
        let tmp = tempdir().expect("tempdir");
        let empty = tmp.path().join("empty.md");
        let real = tmp.path().join("real.md");
        std::fs::write(&empty, "   \n   \n").unwrap();
        std::fs::write(&real, "real content").unwrap();

        let block = super::render_instructions_block(&[empty, real]).expect("non-empty");
        // Empty file produces no `<instructions>` section, only the real one.
        let count = block.matches("<instructions").count();
        assert_eq!(count, 1, "only the non-empty file should produce a section");
    }

    #[test]
    fn render_instructions_block_truncates_oversize_files() {
        let tmp = tempdir().expect("tempdir");
        let big = tmp.path().join("big.md");
        // 200 KiB of content — well above the 100 KiB cap.
        std::fs::write(&big, "X".repeat(200 * 1024)).unwrap();

        let block = super::render_instructions_block(&[big]).expect("non-empty");
        assert!(block.contains("[…elided]"), "truncation marker missing");
        // Block should be much smaller than the original file.
        assert!(
            block.len() < 110 * 1024,
            "block should be capped near 100 KiB"
        );
    }

    #[test]
    fn instructions_block_appears_in_system_prompt_when_configured() {
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path();
        let extra = workspace.join("extra-instructions.md");
        std::fs::write(&extra, "EXTRA_INSTRUCTIONS_MARKER_BODY").unwrap();

        let prompt = match super::system_prompt_for_mode_with_context_and_skills(
            AppMode::Agent,
            workspace,
            None,
            None,
            Some(std::slice::from_ref(&extra)),
            None,
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };

        assert!(
            prompt.contains("EXTRA_INSTRUCTIONS_MARKER_BODY"),
            "configured instructions file body must appear in the prompt"
        );
        assert!(
            prompt.contains(&extra.display().to_string()),
            "instructions block must annotate its source path"
        );
    }
}
