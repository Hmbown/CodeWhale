#![allow(dead_code)]
//! System prompts for different modes.
//!
//! Prompts are assembled from composable layers loaded at compile time:
//!   constitution.md + personality overlay → message[0] (byte-stable).
//!   mode delta + tool taxonomy + approval policy → request-time runtime metadata.
//!
//! This keeps each concern in its own file and makes prompt tuning
//! a single-file operation.

use crate::models::SystemPrompt;
use crate::project_context::{ProjectContext, load_project_context_with_parents};
use crate::tui::app::AppMode;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

#[derive(Debug, Clone)]
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
    /// When true, a ## Language Output Requirement block is appended
    /// to the system prompt instructing the model to respond in
    /// the resolved session locale.
    pub translation_enabled: bool,
    /// Active model identifier. The bundled constitution is model-agnostic,
    /// but embedders may still provide a prompt override containing
    /// `{model_id}`. Defaults to `"codewhale"` when the caller doesn't supply one.
    pub model_id: &'a str,
    /// Route-effective context window, when known. Prompt composition no
    /// longer prints context-window facts, but the field remains part of the
    /// session context contract for embedders and future runtime metadata.
    pub context_window_override: Option<u32>,
    /// Whether the user-visible transcript renders thinking blocks.
    /// When false, the prompt should not spend localization pressure on
    /// `reasoning_content` the user will never see.
    pub show_thinking: bool,
    /// Optional output-verbosity mode. `concise` appends a short output
    /// discipline block; unset keeps the normal conversational prompt.
    pub verbosity: Option<&'a str>,
    /// Restrict skill discovery to CodeWhale-owned roots plus explicit
    /// `skills_dir` configuration.
    pub skills_scan_codewhale_only: bool,
}

impl Default for PromptSessionContext<'_> {
    fn default() -> Self {
        Self {
            user_memory_block: None,
            goal_objective: None,
            project_context_pack_enabled: true,
            locale_tag: "en",
            translation_enabled: false,
            model_id: "codewhale",
            context_window_override: None,
            show_thinking: true,
            verbosity: None,
            skills_scan_codewhale_only: false,
        }
    }
}

/// Conventional location for the structured session relay artifact (#32).
/// A previous session writes it on exit / `/compact`; the next session reads
/// it back on startup and prepends it to the system prompt so a fresh agent
/// doesn't have to re-discover open blockers from scratch.
pub const HANDOFF_RELATIVE_PATH: &str = ".codewhale/handoff.md";
/// Legacy handoff path for reading from existing installs.
const LEGACY_HANDOFF_RELATIVE_PATH: &str = ".deepseek/handoff.md";

/// Per-file size cap for `instructions = [...]` entries (#454). Mirrors
/// the existing project-context cap in `project_context::load_context_file`
/// so a malicious / oversized include can't blow the prompt budget on
/// its own. Files larger than this are truncated with an explicit `[…truncated: N bytes omitted]`
/// marker rather than skipped entirely so the model still sees the head.
const INSTRUCTIONS_FILE_MAX_BYTES: usize = 100 * 1024;

/// System prompt block appended when `translation_enabled` is true.
/// Instructs the model to respond in the resolved session locale for all
/// natural-language output — explanations, summaries, conversation.
/// Code identifiers, untranslatable technical terms, and explicitly
/// requested English code blocks are exempt.
fn translation_output_instruction(locale_tag: &str) -> String {
    let target_language = translation_target_language_for_tag(locale_tag);
    format!(
        "\
## Language Output Requirement\n\
\n\
The user requires all responses in {target_language}. \
Always respond in {target_language} — use natural, professional language for all \
explanations, code comments, summaries, and conversational turns. \
Only output English for:\n\
- Code identifiers (variable names, function names, file paths)\n\
- Technical terms that lack a standard translation in {target_language}\n\
- Code blocks the user explicitly requests in English\n\n\
This is a hard display requirement: the user does not read English, \
so any English prose in your response will block their decision-making."
    )
}

fn concise_output_discipline_instruction() -> &'static str {
    "\
## Concise Output Discipline

To minimize token usage and optimize speed:
- Output only direct, actionable code, technical steps, or final answers.
- Eliminate all conversational filler, fluff, introductions, transitions, or summarizing conclusions.
- Do NOT explain what you are about to do or what you have just completed.
- Do NOT provide conversational status updates before or after running tools.
- Keep explanations and comments extremely brief and technical, explaining only non-obvious reasoning."
}

fn is_concise_verbosity(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.trim().eq_ignore_ascii_case("concise"))
}

fn translation_target_language_for_tag(locale_tag: &str) -> &'static str {
    let normalized = locale_tag.trim().to_ascii_lowercase();
    if normalized.starts_with("ja") {
        "Japanese (日本語)"
    } else if normalized.starts_with("zh-hant")
        || normalized.contains("-tw")
        || normalized.contains("-hk")
        || normalized.contains("-mo")
    {
        "Traditional Chinese (繁體中文)"
    } else if normalized.starts_with("zh") {
        "Simplified Chinese (简体中文)"
    } else if normalized.starts_with("pt") {
        "Brazilian Portuguese (Português do Brasil)"
    } else if normalized.starts_with("vi") {
        "Vietnamese (Tiếng Việt)"
    } else {
        "English"
    }
}

fn hidden_thinking_language_instruction(locale_tag: &str) -> String {
    let fallback_language = translation_target_language_for_tag(locale_tag);
    format!(
        "\
## Hidden Thinking Language\n\
\n\
The user has disabled thinking display (`show_thinking = false`). If you emit \
`reasoning_content`, keep that hidden internal thinking in English regardless \
of the latest user-message language or `## Environment.lang`; the user will \
not see it, so localizing hidden thinking only adds language switching.\n\
\n\
The final reply is still user-visible. Follow the normal `## Language` rule \
for the final reply: mirror the latest user message, and use \
{fallback_language} only when the user message is ambiguous. If the user \
explicitly asks for a different thinking language, follow that explicit request \
for the current turn."
    )
}

/// Render a `## Environment` block listing the resolved locale tag,
/// runtime version, host platform, login shell, and current working directory.
///
/// The block is appended to the workspace-static portion of the
/// system prompt (after mode prompt + project context, before
/// configured instructions / skills). `locale_tag` is resolved by the caller
/// from `Settings` so this function stays I/O-free.
fn render_environment_block(_workspace: &Path, locale_tag: &str) -> String {
    let codewhale_version = env!("CARGO_PKG_VERSION");
    let platform = std::env::consts::OS;
    let shell = crate::shell_dispatcher::global_dispatcher()
        .kind()
        .binary()
        .to_string();

    // The workspace path (`pwd`) is intentionally delivered per-turn via the
    // `<turn_meta>` block (see `turn_metadata_block`) rather than embedded here.
    //
    // Rationale: when the workspace path changes between sessions (e.g. an
    // ephemeral per-session workspace), a volatile value inside the otherwise
    // static system prefix invalidates the inference server's prefix cache at
    // that exact point. The cache then only partially matches and the tail must
    // be re-prefilled from the divergence boundary. On backends that pair prefix
    // caching with speculative decoding, this partial re-prefill can perturb the
    // logits at the boundary enough to degrade structured tool-call emission
    // (the model regresses to bare text). Keeping the static system prefix
    // byte-identical across sessions lets the prefix cache be reused; the live
    // workspace path still reaches the model every turn through `turn_meta`.
    format!(
        "## Environment\n\
         \n\
         - lang: {locale_tag}\n\
         - codewhale_version: {codewhale_version}\n\
         - platform: {platform}\n\
         - shell: {shell}"
    )
}

/// Source for an `EngineConfig.instructions` entry. Either a disk file (loaded
/// at render time, original semantics) or an inline string (content baked into
/// `EngineConfig`, no disk I/O at render time).
///
/// The inline variant is useful for embedders that compute instructions at
/// runtime (e.g. rendering a template with workspace-specific substitutions)
/// and don't want to stage the content to a disk file just to satisfy a path
/// API. Staging adds two problems the inline path avoids:
///
///   1. The disk file looks like editable config but gets overwritten on
///      every launch — confusing for users browsing the install dir.
///   2. Multi-engine setups need per-engine paths to avoid `rehydrate`
///      reading another session's instructions; with inline sources the
///      content lives in the per-engine `EngineConfig` and the race
///      surface goes away.
///
/// `From<PathBuf>` is provided so existing callers passing `Vec<PathBuf>` can
/// keep working with a `.into()` upgrade at the call site.
#[derive(Debug, Clone)]
pub enum InstructionSource {
    /// Load this file from disk at prompt-render time. Original behavior:
    /// missing files are skipped with a warning, oversized files are
    /// truncated to `INSTRUCTIONS_FILE_MAX_BYTES` with an `[…elided]`
    /// marker.
    File(PathBuf),
    /// Use the provided string directly. `name` becomes the
    /// `<instructions source="…">` attribute (typically a synthetic
    /// identifier like `embedded:my-template` or a logical path).
    Inline { name: String, content: String },
}

impl From<PathBuf> for InstructionSource {
    fn from(path: PathBuf) -> Self {
        InstructionSource::File(path)
    }
}

impl From<&PathBuf> for InstructionSource {
    fn from(path: &PathBuf) -> Self {
        InstructionSource::File(path.clone())
    }
}

/// Render the `instructions = [...]` config array as a single
/// system-prompt block (#454). Each source is processed in declared order;
/// missing `File` sources are skipped with a tracing warning so a stale entry
/// doesn't fail the launch. Empty input (or all sources missing/empty)
/// returns `None` so callers append nothing.
fn render_instructions_block(sources: &[InstructionSource]) -> Option<String> {
    let mut sections: Vec<String> = Vec::new();
    for source in sources {
        let (raw_source_name, raw_content): (String, String) = match source {
            InstructionSource::File(path) => match std::fs::read_to_string(path) {
                Ok(raw) => (path.display().to_string(), raw),
                Err(err) => {
                    tracing::warn!(
                        target: "instructions",
                        ?err,
                        ?path,
                        "skipping unreadable instructions file"
                    );
                    continue;
                }
            },
            InstructionSource::Inline { name, content } => (name.clone(), content.clone()),
        };
        let trimmed = raw_content.trim();
        if trimmed.is_empty() {
            continue;
        }
        let body = if trimmed.len() > INSTRUCTIONS_FILE_MAX_BYTES {
            let head_end = (0..=INSTRUCTIONS_FILE_MAX_BYTES)
                .rev()
                .find(|&i| trimmed.is_char_boundary(i))
                .unwrap_or(0);
            format!(
                "{}\n[…truncated: {} of {} bytes omitted — consider splitting this instructions file]",
                &trimmed[..head_end],
                trimmed.len() - head_end,
                trimmed.len()
            )
        } else {
            trimmed.to_string()
        };
        sections.push(format!(
            "<instructions source=\"{raw_source_name}\">\n{body}\n</instructions>"
        ));
    }
    if sections.is_empty() {
        None
    } else {
        Some(sections.join("\n\n"))
    }
}

/// Read the workspace-local relay artifact, if present, and format it as a
/// system-prompt block. Returns `None` when the file is absent or empty so
/// callers can keep the default-uncluttered prompt for fresh workspaces.
fn load_handoff_block(workspace: &Path) -> Option<String> {
    let primary = workspace.join(HANDOFF_RELATIVE_PATH);
    let path = if primary.exists() {
        primary
    } else {
        workspace.join(LEGACY_HANDOFF_RELATIVE_PATH)
    };
    let raw = std::fs::read_to_string(&path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(format!(
        "## Previous Session Relay\n\nThe previous session in this workspace left a relay artifact at `{HANDOFF_RELATIVE_PATH}`. Consider it the first artifact to read on this turn — open blockers, in-flight changes, and recent decisions live there. Update or rewrite it before exiting if state changes materially.\n\n{trimmed}"
    ))
}

/// Load the structured user-global constitution, if present, and render it as
/// its own model-facing block.
fn load_user_constitution_block() -> Option<String> {
    if user_constitution_disabled_by_setup_state() {
        return None;
    }

    let path = match codewhale_config::UserConstitution::path() {
        Ok(path) => path,
        Err(err) => {
            tracing::warn!(
                target: "prompts",
                "could not resolve user-global constitution path: {err:#}"
            );
            return None;
        }
    };

    match codewhale_config::UserConstitution::load_from(&path) {
        codewhale_config::UserConstitutionLoad::Loaded(constitution) => {
            constitution.render_block(None)
        }
        codewhale_config::UserConstitutionLoad::Missing
        | codewhale_config::UserConstitutionLoad::Empty => None,
        codewhale_config::UserConstitutionLoad::Invalid(err) => {
            tracing::warn!(
                target: "prompts",
                "skipping invalid user-global constitution {}: {err}",
                path.display()
            );
            None
        }
        codewhale_config::UserConstitutionLoad::Unreadable(err) => {
            tracing::warn!(
                target: "prompts",
                "skipping unreadable user-global constitution {}: {err}",
                path.display()
            );
            None
        }
    }
}

fn user_constitution_disabled_by_setup_state() -> bool {
    match codewhale_config::SetupState::load() {
        Ok(Some(state)) => matches!(
            state.constitution_choice,
            codewhale_config::ConstitutionChoice::Bundled
                | codewhale_config::ConstitutionChoice::Deferred
                | codewhale_config::ConstitutionChoice::ExpertOverride
        ),
        Ok(None) => false,
        Err(err) => {
            tracing::warn!(
                target: "prompts",
                "could not resolve setup-state path while loading user constitution: {err:#}"
            );
            false
        }
    }
}

// ── Prompt layers loaded at compile time ──────────────────────────────

/// Core: task execution, tool-use rules, output format, toolbox reference,
/// "When NOT to use" guidance, sub-agent sentinel protocol.
///
/// This markdown is the single hand-maintained source of the constitutional
/// system prompt. The earlier YAML + Python-renderer generation pipeline
/// (`constitution.yaml` / `render_constitution.py`) was retired because it
/// had drifted from this file since the v4 "zero ceremony" adoption and the
/// renderer could no longer reproduce it byte-for-byte. The layered runtime
/// assembly composes this core with mode / approval / skills /
/// context-management / compaction / authority-recap layers at runtime (see
/// `system_prompt_for_mode_with_context_skills_and_session`). Edit this file
/// directly; `constitution_md_carries_required_structure` guards its skeleton.
pub const BASE_PROMPT: &str = include_str!("prompts/constitution.md");
/// Language mirroring law, split from the compact constitution in 0.9.0.
pub const LANGUAGE_PROMPT: &str = include_str!("prompts/language.md");
/// Terminal-facing output formatting law, split from the compact constitution.
pub const OUTPUT_PROMPT: &str = include_str!("prompts/output.md");

// ── Embedder prompt overrides ──
// Let an embedder replace these compile-time prompt constants at startup,
// so brand / slimming customizations live in the embedder crate instead of
// editing these files in-tree. Unset → the bundled constant (fully
// backward compatible). Intended to be set once at process start, before
// any engine spawns; later sets return the rejected override string.
static BASE_PROMPT_OVERRIDE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static LOCALE_PREAMBLE_ZH_HANS_OVERRIDE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static LOCALE_PREAMBLE_JA_OVERRIDE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static LOCALE_PREAMBLE_PT_BR_OVERRIDE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static LOCALE_PREAMBLE_VI_OVERRIDE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static LOCALE_CLOSER_ZH_HANS_OVERRIDE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static LOCALE_CLOSER_JA_OVERRIDE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static LOCALE_CLOSER_PT_BR_OVERRIDE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static LOCALE_CLOSER_VI_OVERRIDE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static AUTHORITY_RECAP_OVERRIDE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static STATIC_PROMPT_COMPOSER: std::sync::OnceLock<Box<StaticPromptComposer>> =
    std::sync::OnceLock::new();
static PROMPT_OVERRIDE_NOTICES: LazyLock<Mutex<Vec<String>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// Context passed to an embedder-provided static prompt composer.
///
/// This hook only replaces the byte-stable base/personality prompt segment.
/// Mode deltas, approval policy, tool taxonomy, Context Management, and the
/// Compaction Relay stay owned by CodeWhale's system prompt assembly.
#[non_exhaustive]
#[derive(Debug)]
pub struct StaticPromptCtx<'a> {
    /// Active model identifier after caller-side routing.
    pub model_id: &'a str,
    /// Personality overlay requested for the base static prompt.
    pub personality: Personality,
    /// Default base/personality prompt layers that would be used without an
    /// override.
    pub default_layers: &'a str,
}

/// Embedder hook for replacing CodeWhale's byte-stable base/personality prompt
/// segment.
pub type StaticPromptComposer = dyn Fn(&StaticPromptCtx<'_>) -> String + Send + Sync + 'static;

/// Replace `BASE_PROMPT` for all subsequent prompt composition. First call
/// wins; later calls return the rejected string. Set before spawning any
/// engine.
pub fn set_base_prompt_override(s: String) -> Result<(), String> {
    set_prompt_override(&BASE_PROMPT_OVERRIDE, s)
}

/// Replace the Simplified-Chinese locale preamble (`## 语言要求`).
pub fn set_locale_preamble_zh_hans_override(s: String) -> Result<(), String> {
    set_prompt_override(&LOCALE_PREAMBLE_ZH_HANS_OVERRIDE, s)
}

/// Replace the Japanese locale preamble.
pub fn set_locale_preamble_ja_override(s: String) -> Result<(), String> {
    set_prompt_override(&LOCALE_PREAMBLE_JA_OVERRIDE, s)
}

/// Replace the Brazilian-Portuguese locale preamble.
pub fn set_locale_preamble_pt_br_override(s: String) -> Result<(), String> {
    set_prompt_override(&LOCALE_PREAMBLE_PT_BR_OVERRIDE, s)
}

/// Replace the Vietnamese locale preamble.
pub fn set_locale_preamble_vi_override(s: String) -> Result<(), String> {
    set_prompt_override(&LOCALE_PREAMBLE_VI_OVERRIDE, s)
}

/// Replace the Simplified-Chinese locale closer (`## 语言再次提醒`).
pub fn set_locale_closer_zh_hans_override(s: String) -> Result<(), String> {
    set_prompt_override(&LOCALE_CLOSER_ZH_HANS_OVERRIDE, s)
}

/// Replace the Japanese locale closer.
pub fn set_locale_closer_ja_override(s: String) -> Result<(), String> {
    set_prompt_override(&LOCALE_CLOSER_JA_OVERRIDE, s)
}

/// Replace the Brazilian-Portuguese locale closer.
pub fn set_locale_closer_pt_br_override(s: String) -> Result<(), String> {
    set_prompt_override(&LOCALE_CLOSER_PT_BR_OVERRIDE, s)
}

/// Replace the Vietnamese locale closer.
pub fn set_locale_closer_vi_override(s: String) -> Result<(), String> {
    set_prompt_override(&LOCALE_CLOSER_VI_OVERRIDE, s)
}

/// Replace the trailing `## Authority Recap` block.
pub fn set_authority_recap_override(s: String) -> Result<(), String> {
    set_prompt_override(&AUTHORITY_RECAP_OVERRIDE, s)
}

/// Replace the byte-stable base/personality prompt segment for subsequent
/// prompt composition. First call wins; later calls return the rejected
/// composer so embedders can preserve ownership.
pub fn set_static_prompt_composer_override(
    f: Box<StaticPromptComposer>,
) -> Result<(), Box<StaticPromptComposer>> {
    set_static_prompt_composer(&STATIC_PROMPT_COMPOSER, f)
}

// ── Config-directory prompt overrides (issue #3638) ──
// Bridge the embedder override hooks above to a user-facing source: an
// optional file in the CodeWhale config directory. This lets users repurpose
// the TUI for non-software use cases (e.g. long-form writing) by swapping the
// constitutional base prompt, without editing in-tree files or shipping a
// custom embedder build.
//
// Scope is deliberately narrow: only the byte-stable base prompt segment is
// user-overridable. Mode deltas, approval policy, tool taxonomy, Context
// Management, and the Compaction Relay stay owned by the runtime assembly (see
// `StaticPromptCtx`), so an override cannot strip safety-relevant guidance.
// A missing or empty file is a no-op — the bundled constant is used — so this
// is fully backward compatible.
//
// Because replacing the base prompt is a trust-boundary action (per maintainer
// review on #3638), the override file alone is NOT sufficient: the user must
// also set an explicit opt-in flag (`CODEWHALE_ALLOW_BASE_PROMPT_OVERRIDE`).
// This keeps replacing the global Constitution a deliberate, auditable act
// rather than something a stray file can do.

/// Relative path, under the config directory, of the optional base-prompt
/// (constitution) override file.
pub const CONSTITUTION_OVERRIDE_FILE: &str = "prompts/constitution.md";

/// Env flag that must be set (`1`/`true`/`on`/`yes`) to enable config-dir base
/// prompt overrides. Required in addition to the override file so the global
/// base prompt can never be replaced by file presence alone.
pub const BASE_PROMPT_OVERRIDE_OPT_IN_ENV: &str = "CODEWHALE_ALLOW_BASE_PROMPT_OVERRIDE";

/// Whether the user has explicitly opted in to base-prompt overrides.
pub(crate) fn base_prompt_override_opt_in() -> bool {
    match std::env::var(BASE_PROMPT_OVERRIDE_OPT_IN_ENV) {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "on" | "yes"
        ),
        Err(_) => false,
    }
}

/// Read an optional prompt-override file rooted at `config_dir`.
///
/// Returns the file contents when it exists and is non-empty after trimming;
/// otherwise `None` so the caller falls back to the embedded default. Pure
/// over `config_dir`, so it is unit-testable without touching the global
/// override cells.
fn read_prompt_override_file(config_dir: &Path, relative: &str) -> Option<String> {
    let path = config_dir.join(relative);
    let raw = std::fs::read_to_string(&path).ok()?;
    if raw.trim().is_empty() {
        tracing::warn!(
            target: "prompts",
            "ignoring empty prompt override file {}",
            path.display(),
        );
        return None;
    }
    tracing::info!(
        target: "prompts",
        "loaded prompt override from {}",
        path.display(),
    );
    Some(raw)
}

fn push_prompt_override_notice(message: String) {
    if let Ok(mut notices) = PROMPT_OVERRIDE_NOTICES.lock() {
        notices.push(message);
    }
}

pub fn take_prompt_override_notices() -> Vec<String> {
    PROMPT_OVERRIDE_NOTICES
        .lock()
        .map(|mut notices| std::mem::take(&mut *notices))
        .unwrap_or_default()
}

/// Load user prompt overrides from `config_dir` and install them through the
/// existing override hooks. Returns the names of the overrides that were
/// applied (for logging/diagnostics).
///
/// Call once at startup, before any engine spawns, because the underlying
/// override cells are first-call-wins. Missing files are a no-op, preserving
/// the bundled defaults.
pub fn load_config_dir_prompt_overrides(config_dir: &Path) -> Vec<&'static str> {
    let mut applied = Vec::new();
    if let Some(text) = read_prompt_override_file(config_dir, CONSTITUTION_OVERRIDE_FILE) {
        if !base_prompt_override_opt_in() {
            // A file exists but the user hasn't opted in. Don't silently
            // replace the base prompt — surface the gate instead.
            let warning = format!(
                "Custom Constitution override found at {}/{} but {} is not set; using the bundled Constitution. Set {}=1 to opt in.",
                config_dir.display(),
                CONSTITUTION_OVERRIDE_FILE,
                BASE_PROMPT_OVERRIDE_OPT_IN_ENV,
                BASE_PROMPT_OVERRIDE_OPT_IN_ENV,
            );
            tracing::warn!(
                target: "prompts",
                "{warning}",
            );
            push_prompt_override_notice(warning);
        } else if set_base_prompt_override(text).is_ok() {
            applied.push("constitution");
        }
    }
    applied
}

/// Resolve the CodeWhale config directory and load any prompt overrides found
/// there. Convenience wrapper around [`load_config_dir_prompt_overrides`] for
/// startup wiring; silently does nothing when the config home cannot be
/// resolved.
pub fn load_prompt_overrides_from_config_home() {
    let Ok(home) = codewhale_config::codewhale_home() else {
        return;
    };
    let applied = load_config_dir_prompt_overrides(&home);
    if !applied.is_empty() {
        tracing::info!(
            target: "prompts",
            "applied {} config-directory prompt override(s): {}",
            applied.len(),
            applied.join(", "),
        );
    }
}

fn set_prompt_override(cell: &std::sync::OnceLock<String>, s: String) -> Result<(), String> {
    cell.set(s)
}

fn set_static_prompt_composer(
    cell: &std::sync::OnceLock<Box<StaticPromptComposer>>,
    f: Box<StaticPromptComposer>,
) -> Result<(), Box<StaticPromptComposer>> {
    cell.set(f)
}

fn effective_prompt_override<'a>(
    cell: &'a std::sync::OnceLock<String>,
    fallback: &'static str,
) -> &'a str {
    cell.get().map(String::as_str).unwrap_or(fallback)
}

fn effective_base_prompt() -> &'static str {
    effective_prompt_override(&BASE_PROMPT_OVERRIDE, BASE_PROMPT)
}

fn effective_static_prompt_composer() -> Option<&'static StaticPromptComposer> {
    STATIC_PROMPT_COMPOSER.get().map(Box::as_ref)
}

fn effective_locale_preamble_zh_hans() -> &'static str {
    effective_prompt_override(&LOCALE_PREAMBLE_ZH_HANS_OVERRIDE, LOCALE_PREAMBLE_ZH_HANS)
}

fn effective_locale_preamble_ja() -> &'static str {
    effective_prompt_override(&LOCALE_PREAMBLE_JA_OVERRIDE, LOCALE_PREAMBLE_JA)
}

fn effective_locale_preamble_pt_br() -> &'static str {
    effective_prompt_override(&LOCALE_PREAMBLE_PT_BR_OVERRIDE, LOCALE_PREAMBLE_PT_BR)
}

fn effective_locale_preamble_vi() -> &'static str {
    effective_prompt_override(&LOCALE_PREAMBLE_VI_OVERRIDE, LOCALE_PREAMBLE_VI)
}

fn effective_locale_closer_zh_hans() -> &'static str {
    effective_prompt_override(&LOCALE_CLOSER_ZH_HANS_OVERRIDE, LOCALE_CLOSER_ZH_HANS)
}

fn effective_locale_closer_ja() -> &'static str {
    effective_prompt_override(&LOCALE_CLOSER_JA_OVERRIDE, LOCALE_CLOSER_JA)
}

fn effective_locale_closer_pt_br() -> &'static str {
    effective_prompt_override(&LOCALE_CLOSER_PT_BR_OVERRIDE, LOCALE_CLOSER_PT_BR)
}

fn effective_locale_closer_vi() -> &'static str {
    effective_prompt_override(&LOCALE_CLOSER_VI_OVERRIDE, LOCALE_CLOSER_VI)
}

fn effective_authority_recap() -> &'static str {
    effective_prompt_override(&AUTHORITY_RECAP_OVERRIDE, AUTHORITY_RECAP)
}

/// Optional locale-native reinforcement preamble prepended to the system
/// prompt when the user's UI locale is non-English.
///
/// `constitution.md` itself stays English (single source of truth, model is
/// natively multilingual, prefix-cache stable across users in the same
/// locale). For non-English locales we prepend a short locale-native
/// passage so the model's first exposure to the prompt overrides the
/// "match user message language" English directive with an explicit
/// "use {locale}" instruction in the user's own writing system. Reduces
/// the model's reliance on inferring intent from `## Environment.lang`
/// — which previously got overpowered by overwhelmingly English task
/// context, the symptom reported in #1118 and visible in the WeChat
/// screenshot that prompted this change.
///
/// The list is intentionally short (only locales the TUI ships UI
/// strings for: `zh-Hans`, `ja`, `pt-BR`). Other locales fall through
/// to `None` and get the English-only directive, which is the same
/// behavior as before this change.
///
/// ## Design philosophy: why a bookend, not a full translation
///
/// Community feedback on the WeChat thread that prompted this work
/// pointed out — correctly — that DeepSeek V4 is a Chinese-first
/// multilingual model, not an English-only model with multilingual
/// veneer. Its tokenizer is co-trained on Chinese; `你好` typically
/// encodes to ~1 token, not 2 — the "Chinese is expensive in tokens"
/// folk wisdom from Western-LLM commentary doesn't apply here.
///
/// The naïve translation of that argument would be: ship a fully
/// translated `constitution.md` per locale. We deliberately stop short of
/// that for v0.8.29. The reasons, ranked:
///
///   1. **Drift risk.** A 200+ line technical prompt has subtle
///      phrasing that drives subtle behavior. Every rule change has
///      to land in N translated copies, kept in lockstep. The class
///      of bug that arises (Chinese users see slightly different
///      agent behavior than English users) is hard to reproduce and
///      hard to triage from bug reports.
///   2. **Cache stability.** With one English `constitution.md` and a
///      per-locale preamble+closer, the largest cacheable chunk
///      (mode prompt + project context + environment) stays
///      byte-stable within a session and across users in the same
///      locale. A fully translated per-locale `constitution.md` keeps cache
///      per-locale but doesn't share with English users.
///   3. **Translation QA is expensive.** Each prompt-language pair
///      needs a native speaker reviewing tone, register, and rule
///      preservation. Getting it 95% right is bad, because the
///      missing 5% becomes silent behavior divergence.
///
/// What we DO instead — the bookend pattern @MuMu described from
/// their other project — is reinforce the locale directive in
/// native script at BOTH ends of the prompt. The opening anchors
/// behavior at session start; the closing reinforcement
/// (`locale_reinforcement_closer`) sits at the maximum-recency
/// position right before the user's next message. Empirically this
/// is sufficient to keep `reasoning_content` in the target locale
/// even as English code accumulates in context turn-over-turn.
///
/// If at some future point the bookend proves insufficient — or if
/// the maintenance cost of per-locale `constitution.md` files becomes
/// preferable to whatever's blocking it — full translation is the
/// natural next step. The locale tags here, the test invariants,
/// and the closer position would all carry over unchanged.
pub(crate) fn locale_reinforcement_preamble(locale_tag: &str) -> Option<&'static str> {
    match locale_tag {
        "zh-Hans" | "zh-CN" | "zh" => Some(effective_locale_preamble_zh_hans()),
        "ja" | "ja-JP" => Some(effective_locale_preamble_ja()),
        "pt-BR" | "pt" => Some(effective_locale_preamble_pt_br()),
        "vi" | "vi-VN" => Some(effective_locale_preamble_vi()),
        _ => None,
    }
}

/// Locale-native closing reinforcement appended to the very end of the
/// system prompt — the bookend MuMu described in the WeChat thread that
/// prompted #1118 follow-up work.
///
/// The opening preamble alone is not enough: as the model accumulates
/// English context turn-over-turn (code, error logs, search results,
/// file listings), the recency bias of the transformer's attention
/// drifts thinking back toward English even when the user keeps writing
/// in their own language. A closing native-script reinforcement sits at
/// the position closest to the user's next message — where attention
/// weight is highest — and re-asserts the language rule right before
/// the model generates `reasoning_content` for the turn.
///
/// Like the opening preamble, English (and unknown) locales return
/// `None` and the system prompt is byte-identical to the pre-bookend
/// behavior.
pub(crate) fn locale_reinforcement_closer(locale_tag: &str) -> Option<&'static str> {
    match locale_tag {
        "zh-Hans" | "zh-CN" | "zh" => Some(effective_locale_closer_zh_hans()),
        "ja" | "ja-JP" => Some(effective_locale_closer_ja()),
        "pt-BR" | "pt" => Some(effective_locale_closer_pt_br()),
        "vi" | "vi-VN" => Some(effective_locale_closer_vi()),
        _ => None,
    }
}

const LOCALE_PREAMBLE_ZH_HANS: &str = "## 语言要求\n\n\
你正在 codewhale 中运行。无论任务上下文（代码、错误日志、文件名）\
是英文，无论系统提示的其余部分是英文，你都必须用简体中文进行 \
`reasoning_content`（内部思考）和最终回复。代码、文件路径、工具名称\
（例如 `read_file`、`exec_shell`）、环境变量、命令行参数和 URL \
保持原样 —— 只有自然语言散文要切换到简体中文。\n\n\
如果用户在会话中切换到另一种语言，从下一轮开始跟随切换。\
如果用户明确要求（例如 \"think in English\"），则覆盖此规则。";

const LOCALE_PREAMBLE_JA: &str = "## 言語要件\n\n\
codewhale を実行しています。タスクコンテキスト（コード、エラーログ、\
ファイル名）が英語であっても、システムプロンプトの他の部分が英語で\
あっても、`reasoning_content`（内部思考）と最終的な返信は日本語で\
行ってください。コード、ファイルパス、ツール名（例：`read_file`、\
`exec_shell`）、環境変数、コマンドライン引数、URL は元のまま —— \
自然言語の文章のみ日本語に切り替えます。\n\n\
ユーザーがセッション中に別の言語に切り替えた場合は、次のターンから\
それに従ってください。ユーザーが明示的に要求した場合（例：\
\"think in English\"）はこのルールを上書きします。";

const LOCALE_PREAMBLE_PT_BR: &str = "## Requisito de Idioma\n\n\
Você está rodando dentro do codewhale. Escreva tanto \
`reasoning_content` (seu pensamento interno) quanto a resposta final \
em português do Brasil, mesmo quando o contexto da tarefa (código, \
logs de erro, nomes de arquivos) estiver em inglês e mesmo quando o \
resto do system prompt for em inglês. Mantenha código, caminhos de \
arquivos, nomes de ferramentas (por exemplo `read_file`, \
`exec_shell`), variáveis de ambiente, flags de linha de comando e \
URLs no formato original — apenas a prosa em linguagem natural muda \
para português do Brasil.\n\n\
Se o usuário mudar de idioma no meio da sessão, mude no próximo turno. \
Se o usuário pedir explicitamente (por exemplo, \"think in English\"), \
isso sobrescreve esta regra.";

// ── Closing bookends (appended to the very end of the system prompt) ──

const LOCALE_CLOSER_ZH_HANS: &str = "## 语言再次提醒\n\n\
**重要：你的 `reasoning_content`（内部思考）和最终回复必须保持简体中文。** \
无论你在这次会话中读到了多少英文代码、错误日志或文档，无论项目上下文 \
是英文，思考过程不能漂移到英文。这是会话级硬性要求 —— 用户的语言决定 \
你的语言，与上下文中累积的英文内容无关。除非用户明确要求切换（例如 \
\"think in English\"），否则继续用简体中文思考和回答。";

const LOCALE_CLOSER_JA: &str = "## 言語再確認\n\n\
**重要：`reasoning_content`（内部思考）と最終的な返信は日本語で行ってください。** \
このセッションで読み込んだ英語のコード、エラーログ、ドキュメントの量に \
関係なく、プロジェクトコンテキストが英語であっても、思考プロセスを \
英語に逸らさないでください。これはセッションレベルの厳格な要件であり、 \
ユーザーの言語があなたの言語を決定します。ユーザーが明示的に切り替えを \
要求しない限り（例：\"think in English\"）、日本語で思考し、回答し続けて \
ください。";

const LOCALE_CLOSER_PT_BR: &str = "## Reforço de Idioma\n\n\
**Importante: seu `reasoning_content` (pensamento interno) e a resposta \
final devem permanecer em português do Brasil.** Independentemente de \
quanto código em inglês, logs de erro ou documentação você ler nesta \
sessão, e independentemente de o contexto do projeto ser em inglês, o \
processo de pensamento não pode derivar para o inglês. Este é um \
requisito rígido em nível de sessão — o idioma do usuário define seu \
idioma. A menos que o usuário peça explicitamente a troca (por exemplo, \
\"think in English\"), continue pensando e respondendo em português do \
Brasil.";

const LOCALE_PREAMBLE_VI: &str = "## Yêu cầu ngôn ngữ\n\n\
Bạn đang chạy trong codewhale. Cho dù ngữ cảnh tác vụ (mã nguồn, nhật ký lỗi, tên tệp) \
là tiếng Anh, cho dù phần còn lại của system prompt là tiếng Anh, bạn đều phải sử dụng \
tiếng Việt cho phần `reasoning_content` (suy nghĩ nội bộ) và câu trả lời cuối cùng. Các từ \
mã nguồn, đường dẫn tệp, tên công cụ (ví dụ `read_file`, `exec_shell`), biến môi trường, \
tham số dòng lệnh và URL giữ nguyên dạng gốc —— chỉ các văn bản giải thích bằng ngôn ngữ \
tự nhiên mới được chuyển sang tiếng Việt.\n\n\
Nếu người dùng chuyển sang ngôn ngữ khác trong phiên làm việc, hãy chuyển theo từ lượt tiếp theo. \
Nếu người dùng yêu cầu rõ ràng (ví dụ \"think in English\"), hãy ghi đè quy tắc này.";

const LOCALE_CLOSER_VI: &str = "## Nhắc nhở ngôn ngữ một lần nữa\n\n\
**Quan trọng: phần `reasoning_content` (suy nghĩ nội bộ) và phản hồi cuối cùng của bạn phải được viết bằng tiếng Việt.** \
Dù bạn có đọc bao nhiêu mã nguồn tiếng Anh, nhật ký lỗi hay tài liệu trong phiên làm việc này, và dù ngữ cảnh \
dự án có là tiếng Anh, quá trình suy nghĩ của bạn cũng không được chuyển sang tiếng Anh. Đây là yêu cầu cứng \
ở cấp phiên làm việc —— ngôn ngữ của người dùng quyết định ngôn ngữ của bạn, không phụ thuộc vào nội dung tiếng Anh \
tích lũy trong ngữ cảnh. Trừ khi người dùng yêu cầu rõ ràng việc chuyển đổi (ví dụ \"think in English\"), \
hãy tiếp tục suy nghĩ và trả lời bằng tiếng Việt.";

/// Personality overlays — voice and tone.
pub const CALM_PERSONALITY: &str = include_str!("prompts/personalities/calm.md");
pub const PLAYFUL_PERSONALITY: &str = include_str!("prompts/personalities/playful.md");

/// Mode deltas — permissions, workflow expectations, mode-specific rules.
pub const AGENT_MODE: &str = include_str!("prompts/modes/agent.md");
pub const PLAN_MODE: &str = include_str!("prompts/modes/plan.md");
pub const YOLO_MODE: &str = include_str!("prompts/modes/yolo.md");
pub const OPERATE_MODE: &str = include_str!("prompts/modes/operate.md");

/// Approval-policy overlays — whether tool calls are auto-approved,
/// require confirmation, or are blocked.
pub const AUTO_APPROVAL: &str = include_str!("prompts/approvals/auto.md");
pub const SUGGEST_APPROVAL: &str = include_str!("prompts/approvals/suggest.md");
pub const NEVER_APPROVAL: &str = include_str!("prompts/approvals/never.md");

/// Shell policy guidance for `allow_shell=false`. Referenced from the
/// Runtime Policy Reference so the model can adapt without mutating the
/// static system-prompt prefix (preserves DeepSeek prefix cache across
/// shell-access toggles).
pub const SHELL_POLICY_DISABLED: &str = "Shell tools unavailable. For mandatory-use items referencing \
`exec_shell`, use `code_execution` (Python sandbox). For GitHub triage, use \
`github_issue_context` / `github_pr_context` as primary route.";

/// Compaction relay template — written into the system prompt so the
/// model knows the format to use when writing `.codewhale/handoff.md`.
pub const COMPACT_TEMPLATE: &str = include_str!("prompts/compact.md");

/// Goal continuation audit template — injected by the engine when a runtime
/// goal is active and the assistant tries to end a turn without closing it.
pub const GOAL_CONTINUATION_PROMPT: &str = include_str!("prompts/continuation.md");

/// Memory hygiene guidance — appended to the system prompt only when the
/// session has a non-empty user-memory block. Steers the model toward
/// writing durable memories as declarative facts ("User prefers concise
/// responses") rather than imperatives ("Always respond concisely"),
/// because imperatives get re-read as directives in later sessions and
/// can override the user's current request (#725).
pub const MEMORY_GUIDANCE: &str = include_str!("prompts/memory_guidance.md");

// ── Legacy prompt constants (kept for backwards compatibility) ────────

/// Legacy base prompt (agent.txt — now decomposed into constitution.md + overlays).
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

/// Compose the full system prompt in deterministic order:
///   1. tool taxonomy  — compact hints generated from the eager core tools
///   2. constitution.md — core identity, toolbox, execution contract
///   3. personality    — voice and tone overlay
///   4. mode delta     — mode-specific permissions and workflow
///   5. approval policy — tool-approval behavior
///
/// Each layer is separated by a blank line for readability in the
/// rendered prompt (the model sees them as contiguous sections).
/// Substitute the model id for embedder-supplied prompt overrides that still
/// template it. The bundled constitution is deliberately model-agnostic and
/// carries no model-fact placeholders.
fn apply_model_template(
    prompt: &str,
    model_id: &str,
    _context_window_override: Option<u32>,
) -> String {
    prompt.replace("{model_id}", model_id)
}

const TOOL_TAXONOMY_DISCOVERY: &[&str] = &["grep_files", "file_search"];
const TOOL_TAXONOMY_GIT: &[&str] = &["git_status", "git_diff"];
const TOOL_TAXONOMY_VERIFICATION: &[&str] = &["run_tests", "run_verifiers"];

/// Return the core tool taxonomy body **without** a markdown heading.
/// Suitable for embedding under a mode-specific sub-heading in the
/// Runtime Policy Reference without producing a broken heading hierarchy.
pub(crate) fn render_core_tool_taxonomy_body(mode: AppMode) -> String {
    let core_tools = core_taxonomy_tools_for_mode(mode);
    let mut sentences = Vec::new();

    if let Some(discovery) = render_core_tool_group(TOOL_TAXONOMY_DISCOVERY, &core_tools) {
        sentences.push(format!("Use {discovery} for discovery."));
    }
    if let Some(git) = render_core_tool_group(TOOL_TAXONOMY_GIT, &core_tools) {
        sentences.push(format!("Use {git} for git inspection."));
    }
    if let Some(verification) = render_core_tool_group(TOOL_TAXONOMY_VERIFICATION, &core_tools) {
        sentences.push(format!("Use {verification} for verification."));
    }
    if core_tools.contains(&"run_verifiers") {
        sentences.push(
            "For long build/test/lint verifier suites, call `run_verifiers` with `background: true` or use `task_shell_start`, then poll while continuing independent inspection."
                .to_string(),
        );
    }

    debug_assert!(
        !sentences.is_empty(),
        "core tool taxonomy has no active tool groups"
    );
    sentences.join(" ")
}

fn core_taxonomy_tools_for_mode(mode: AppMode) -> Vec<&'static str> {
    let core_tools = crate::core::engine::default_active_native_tool_names();
    core_tools
        .iter()
        .copied()
        .filter(|tool| mode != AppMode::Plan || !matches!(*tool, "run_tests" | "run_verifiers"))
        .collect()
}

fn render_core_tool_group(group: &[&str], core_tools: &[&str]) -> Option<String> {
    let rendered = group
        .iter()
        .copied()
        .filter(|tool| core_tools.contains(tool))
        .map(|tool| format!("`{tool}`"))
        .collect::<Vec<_>>()
        .join("/");
    (!rendered.is_empty()).then_some(rendered)
}

/// Authority recap block — appended at the end of the system prompt,
/// just before the user's first message. Uses recency bias constructively:
/// this is the last thing the model reads before generating, so it
/// reinforces the Constitutional hierarchy without occupying cache-stable
/// prefix space.
const AUTHORITY_RECAP: &str = "\
## Authority Recap

CodeWhale's constitution governs your behavior. Ground truth underlies the
whole list: the user may override a fact, but no one may invent one. When
guidance conflicts, the user's request this turn outranks this constitution,
which outranks nearest-scope project law and instructions, which outrank
standing user-global preferences, which outrank memory and previous-session
handoffs. When in doubt, consult ### Whose word wins.";

pub fn compose_prompt(personality: Personality) -> String {
    compose_prompt_with_approval_model_and_shell(personality, "codewhale")
}

pub(crate) fn compose_prompt_with_approval_model_and_shell(
    personality: Personality,
    model_id: &str,
) -> String {
    let default_layers = compose_default_static_layers(personality, model_id);
    apply_static_prompt_composer(
        effective_static_prompt_composer(),
        personality,
        model_id,
        &default_layers,
    )
}

fn compose_default_static_layers(_personality: Personality, model_id: &str) -> String {
    // Personality is folded into the constitutional preamble/articles — no
    // separate overlay is appended. Language and output rules are split into
    // their own static segments so the 0.9.0 constitution stays compact.
    let layers = format!(
        "{}\n\n{}\n\n{}",
        effective_base_prompt().trim(),
        LANGUAGE_PROMPT.trim(),
        OUTPUT_PROMPT.trim()
    );
    apply_model_template(&layers, model_id, None)
}

fn apply_static_prompt_composer(
    composer: Option<&StaticPromptComposer>,
    personality: Personality,
    model_id: &str,
    default_layers: &str,
) -> String {
    match composer {
        Some(composer) => composer(&StaticPromptCtx {
            model_id,
            personality,
            default_layers,
        }),
        None => default_layers.to_string(),
    }
}

// The full base prompt is always used; effective tool availability is enforced
// by the tool catalog and execution layer rather than by mutating message[0].

// ── Public API ────────────────────────────────────────────────────────

/// Get the system prompt for a specific mode with project context.
pub fn system_prompt_for_mode_with_context(
    workspace: &Path,
    working_set_summary: Option<&str>,
) -> SystemPrompt {
    system_prompt_for_mode_with_context_and_skills(workspace, working_set_summary, None, None, None)
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
///   5. compaction relay template (compile-time constant)
///   6. relay block — file-backed; rewritten by `/compact` and on exit
///
/// Anything appended after a volatile block forfeits the cache for the rest
/// of the request. New blocks belong above the relay boundary unless they
/// themselves are turn-volatile. Working-set metadata is now injected into the
/// latest user message as per-turn metadata instead of this system prompt.
pub fn system_prompt_for_mode_with_context_and_skills(
    workspace: &Path,
    working_set_summary: Option<&str>,
    skills_dir: Option<&Path>,
    instructions: Option<&[InstructionSource]>,
    user_memory_block: Option<&str>,
) -> SystemPrompt {
    system_prompt_for_mode_with_context_skills_and_session(
        workspace,
        working_set_summary,
        skills_dir,
        instructions,
        PromptSessionContext {
            user_memory_block,
            goal_objective: None,
            project_context_pack_enabled: true,
            locale_tag: "en",
            translation_enabled: false,
            model_id: "codewhale",
            context_window_override: None,
            show_thinking: true,
            verbosity: None,
            skills_scan_codewhale_only: false,
        },
    )
}

pub fn system_prompt_for_mode_with_context_skills_and_session(
    workspace: &Path,
    _working_set_summary: Option<&str>,
    skills_dir: Option<&Path>,
    instructions: Option<&[InstructionSource]>,
    session_context: PromptSessionContext<'_>,
) -> SystemPrompt {
    system_prompt_for_mode_with_context_skills_session_and_approval(
        workspace,
        _working_set_summary,
        skills_dir,
        instructions,
        session_context,
    )
}

pub fn system_prompt_for_mode_with_context_skills_session_and_approval(
    workspace: &Path,
    _working_set_summary: Option<&str>,
    skills_dir: Option<&Path>,
    instructions: Option<&[InstructionSource]>,
    session_context: PromptSessionContext<'_>,
) -> SystemPrompt {
    let default_layers = apply_model_template(
        effective_base_prompt().trim(),
        session_context.model_id,
        session_context.context_window_override,
    );
    let mode_prompt = apply_static_prompt_composer(
        effective_static_prompt_composer(),
        Personality::Calm,
        session_context.model_id,
        &default_layers,
    );

    // Load project context from workspace
    let project_context = load_project_context_with_parents(workspace);

    // 0. Locale-native reinforcement preamble (#1118 follow-up). When the
    // user's UI locale is non-English we prepend a short native-script
    // passage so the model's first exposure to the prompt is an explicit
    // "think and reply in {locale}" directive in the user's own writing
    // system — defeats the "task context is English, so the model thinks
    // in English even though `lang: zh-Hans` is set" failure mode that
    // PR #1398 partially addressed. English (and unknown) locales get
    // `None` and keep the previous behavior unchanged.
    let preamble = if session_context.show_thinking {
        locale_reinforcement_preamble(session_context.locale_tag)
    } else {
        None
    };

    // 1–2. Mode prompt + project context.
    // `load_project_context_with_parents` generates an in-memory bounded
    // overview when no context file exists, so the fallback should usually be
    // available without writing project-local files.
    let mut full_prompt = if let Some(project_block) = project_context.as_system_block() {
        format!("{mode_prompt}\n\n{project_block}")
    } else {
        // Extremely unlikely: context generation failed (e.g. filesystem error).
        // Use mode prompt alone rather than panic.
        tracing::warn!("No project context available and auto-generation failed");
        mode_prompt
    };

    if let Some(preamble) = preamble {
        full_prompt = format!("{preamble}\n\n{full_prompt}");
    }

    if let Some(user_constitution_block) = load_user_constitution_block() {
        full_prompt = format!("{full_prompt}\n\n{user_constitution_block}");
    }

    if session_context.project_context_pack_enabled
        && let Some(pack) = crate::project_context::generate_project_context_pack(workspace)
    {
        full_prompt = format!("{full_prompt}\n\n{pack}");
    }

    // 2.3a. Translation output instruction — when enabled, instruct
    // the model to respond in the resolved session locale. Stays
    // above the volatile-content boundary because it's a per-session
    // flag, not a per-turn one: enabling `/translate` is a session
    // toggle, so the prompt-prefix bytes don't drift turn-over-turn.
    if session_context.translation_enabled {
        full_prompt = format!(
            "{full_prompt}\n\n{}",
            translation_output_instruction(session_context.locale_tag)
        );
    }

    if is_concise_verbosity(session_context.verbosity) {
        full_prompt = format!(
            "{full_prompt}\n\n{}",
            concise_output_discipline_instruction()
        );
    }

    // 3. Skills block. #432: default discovery walks every compatible
    // workspace/global skill directory so skills installed for other AI-tool
    // conventions show up in the catalogue. Users can opt into a CodeWhale-only
    // scan with `[skills] scan_codewhale_only = true`. When an explicit
    // `skills_dir` is configured, union it with the workspace view instead of
    // treating it as a fallback; the workspace view often returns Some and
    // would otherwise shadow the configured directory entirely.
    let skill_discovery_mode = crate::skills::SkillDiscoveryMode::from_codewhale_only(
        session_context.skills_scan_codewhale_only,
    );
    let skills_block = match skills_dir {
        Some(dir) => {
            crate::skills::render_available_skills_context_for_workspace_and_dir_with_mode(
                workspace,
                dir,
                skill_discovery_mode,
                session_context.locale_tag,
            )
        }
        None => crate::skills::render_available_skills_context_for_workspace_with_mode(
            workspace,
            skill_discovery_mode,
            session_context.locale_tag,
        ),
    };
    if let Some(block) = skills_block {
        full_prompt = format!("{full_prompt}\n\n{block}");
    }

    // 4. Context Management — included in all modes.
    {
        full_prompt.push_str(
            "\n\n## Context Management\n\n\
             When the conversation gets long (you'll see a context usage indicator), you can:\n\
             1. Use `/compact` to summarize earlier context and free up space\n\
             2. The system will preserve important information (files you're working on, recent messages, tool results)\n\
             3. After compaction, you'll see a summary of what was discussed and can continue seamlessly\n\n\
             If you notice context is getting long (>60% during sustained work), proactively suggest using `/compact` or Ctrl+L to the user. If auto_compact is enabled, the engine can compact before the next send once the configured threshold is crossed.\n\n\
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

    // 5. Compaction relay template — so the model knows the format to use
    //    when writing `.codewhale/handoff.md` on exit / `/compact`.
    full_prompt.push_str("\n\n");
    full_prompt.push_str(COMPACT_TEMPLATE);

    // ── Volatile-content boundary ─────────────────────────────────────────
    // Everything below drifts mid-session and busts the prefix cache for
    // bytes that follow. All static layers (mode, project context, env,
    // skills, context management, compact template) live above this line
    // so DeepSeek's KV prefix cache can hit on the entire system prompt
    // regardless of per-session edits to memory, goals, or instructions.

    // 6. Environment block — platform, shell, pwd, locale.
    //
    // Placed below the volatile-content boundary. The original comment claimed
    // "workspace path is fixed for the run" → static-cacheable, which is true
    // for the terminal use case (one process owns one workspace for its
    // lifetime). It is **not** true for embedders that swap workspaces between
    // sessions (the Op::SyncSession path, multi-engine pools, IDE
    // integrations binding the engine to a per-tab workspace, etc.):
    // `pwd` drifts session-to-session and drags the entire static prefix
    // out of cache reuse. Moving the block below the volatile boundary keeps
    // mode / project / skills / context-mgmt / compact-template byte-stable
    // across sessions while preserving the pwd info the model needs for
    // `exec_shell` and structured search tools.
    full_prompt = format!(
        "{full_prompt}\n\n{}",
        render_environment_block(workspace, session_context.locale_tag),
    );

    // 6a. Configured `instructions = [...]` files (#454). Loaded
    // and concatenated in declared order. Placed below the volatile boundary
    // because these files are workspace-scoped and may differ between
    // sessions; any edit to them would otherwise bust the prefix cache for
    // all subsequent static layers.
    if let Some(sources) = instructions
        && let Some(block) = render_instructions_block(sources)
    {
        full_prompt = format!("{full_prompt}\n\n{block}");
    }

    // 6b. User memory block (#489). Placed below the volatile boundary
    // because memory entries are editable mid-session via `/memory` or
    // `# foo` quick-add. When they change, they only invalidate the
    // trailing relay block — the static prefix above stays cached.
    if let Some(memory_block) = session_context.user_memory_block
        && !memory_block.trim().is_empty()
    {
        full_prompt = format!("{full_prompt}\n\n{memory_block}\n\n{MEMORY_GUIDANCE}");
    }

    // 6c. Current session goal. Also volatile: users set / change goals
    // during a session via `/goal`. Placed below the boundary for the
    // same reason as memory.
    if let Some(goal_objective) = session_context.goal_objective
        && !goal_objective.trim().is_empty()
    {
        full_prompt = format!(
            "{full_prompt}\n\n## Current Goal\n\n<session_goal>\n{}\n</session_goal>",
            goal_objective.trim()
        );
    }

    // 7. Previous-session relay (file-backed, rewritten by `/compact`).
    if let Some(handoff_block) = load_handoff_block(workspace) {
        full_prompt = format!("{full_prompt}\n\n{handoff_block}");
    }

    // 7a. Authority recap — the final tier reminder before user messages.
    // Uses recency bias constructively: this is the last content the model
    // sees before the user's turn, reinforcing the Constitutional hierarchy.
    let authority_recap = effective_authority_recap();
    full_prompt = format!("{full_prompt}\n\n{authority_recap}");

    // 8. Locale-native closing reinforcement (#1118 follow-up #2). The
    // opening preamble alone wasn't enough — community feedback (the
    // WeChat thread about XML-tagged bilingual bookends) flagged that as
    // English context accumulates turn-over-turn, the model's recency
    // bias pulls thinking back to English. Putting the same directive at
    // the END of the system prompt — right before the user's next
    // message — uses recency bias *in our favor*: the model sees the
    // native-script "keep thinking in Chinese / Japanese / Portuguese"
    // rule immediately before it generates `reasoning_content` for the
    // turn. English (and unknown) locales return `None` and the prompt
    // stays byte-identical to the pre-bookend behavior.
    if let Some(closer) = session_context
        .show_thinking
        .then(|| locale_reinforcement_closer(session_context.locale_tag))
        .flatten()
    {
        full_prompt = format!("{full_prompt}\n\n{closer}");
    } else if !session_context.show_thinking {
        full_prompt = format!(
            "{full_prompt}\n\n{}",
            hidden_thinking_language_instruction(session_context.locale_tag)
        );
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

#[cfg(test)]
mod tests {
    // Don't assert on prose. If you wouldn't fail a code review for
    // changing the wording, don't fail a test for it.
    use super::*;
    use crate::tools::apply_patch::ApplyPatchTool;
    use crate::tools::file::{EditFileTool, WriteFileTool};
    use crate::tools::handle::HandleReadTool;
    use crate::tools::rlm::{RlmCloseTool, RlmConfigureTool, RlmEvalTool, RlmOpenTool};
    use crate::tools::shell::ExecShellTool;
    use crate::tools::spec::ToolSpec;
    use tempfile::tempdir;

    /// Discriminator unique to the injected relay block (not present in the
    /// agent prompt's own discussion of the convention).
    const HANDOFF_BLOCK_MARKER: &str = "left a relay artifact at `.codewhale/handoff.md`";

    // Config-directory prompt override resolution (#3638). These exercise the
    // pure file resolver only; the global install path is intentionally not
    // unit-tested here because `set_base_prompt_override` writes a process-wide
    // `OnceLock` that would leak into sibling tests (same reason
    // `prompt_override_storage_reports_duplicate_sets` uses a local cell).

    #[test]
    fn config_override_reads_present_nonempty_file() {
        let tmp = tempdir().expect("tempdir");
        let prompts_dir = tmp.path().join("prompts");
        std::fs::create_dir_all(&prompts_dir).expect("mkdir");
        std::fs::write(
            prompts_dir.join("constitution.md"),
            "You are a long-form writing companion.\n",
        )
        .expect("write override");

        let got = read_prompt_override_file(tmp.path(), CONSTITUTION_OVERRIDE_FILE);
        assert_eq!(
            got.as_deref(),
            Some("You are a long-form writing companion.\n")
        );
    }

    #[test]
    fn config_override_absent_file_falls_back() {
        let tmp = tempdir().expect("tempdir");
        // No prompts/ directory at all → None so the embedded constant is used.
        assert!(read_prompt_override_file(tmp.path(), CONSTITUTION_OVERRIDE_FILE).is_none());
    }

    #[test]
    fn config_override_requires_explicit_opt_in() {
        // A present, non-empty override file must NOT replace the base prompt
        // unless the explicit opt-in flag is set. This test drains the shared
        // process-global PROMPT_OVERRIDE_NOTICES queue, so it must serialize
        // against the sibling test that also touches it
        // (`tui::ui::tests::prompt_override_notice_surfaces_in_transcript_and_toast`);
        // both take `lock_test_env()` for mutual exclusion under the multi-
        // threaded test binary.
        let _env_guard = crate::test_support::lock_test_env();
        let tmp = tempdir().expect("tempdir");
        let prompts_dir = tmp.path().join("prompts");
        std::fs::create_dir_all(&prompts_dir).expect("mkdir");
        std::fs::write(
            prompts_dir.join("constitution.md"),
            "You are a long-form writing companion.\n",
        )
        .expect("write override");

        // The resolver still finds the file...
        assert!(read_prompt_override_file(tmp.path(), CONSTITUTION_OVERRIDE_FILE).is_some());
        // ...but without the opt-in flag, nothing is applied.
        if std::env::var(BASE_PROMPT_OVERRIDE_OPT_IN_ENV).is_err() {
            let _ = take_prompt_override_notices();
            assert!(
                load_config_dir_prompt_overrides(tmp.path()).is_empty(),
                "override must require the explicit opt-in flag, not just a file"
            );
            let notices = take_prompt_override_notices();
            assert!(
                notices
                    .iter()
                    .any(|notice| notice.contains(BASE_PROMPT_OVERRIDE_OPT_IN_ENV)
                        && notice.contains("using the bundled Constitution")),
                "gated override should record a visible notice, got {notices:?}"
            );
        }
    }

    #[test]
    fn config_override_empty_file_is_ignored() {
        let tmp = tempdir().expect("tempdir");
        let prompts_dir = tmp.path().join("prompts");
        std::fs::create_dir_all(&prompts_dir).expect("mkdir");
        std::fs::write(prompts_dir.join("constitution.md"), "   \n\t\n").expect("write blank");

        // Whitespace-only overrides are treated as absent so a stray empty file
        // can't silently blank the system prompt.
        assert!(read_prompt_override_file(tmp.path(), CONSTITUTION_OVERRIDE_FILE).is_none());
    }

    #[test]
    fn prompt_override_storage_reports_duplicate_sets() {
        let cell = std::sync::OnceLock::new();

        assert_eq!(effective_prompt_override(&cell, "fallback"), "fallback");
        assert!(set_prompt_override(&cell, "first".to_string()).is_ok());
        assert_eq!(effective_prompt_override(&cell, "fallback"), "first");
        assert_eq!(
            set_prompt_override(&cell, "second".to_string()),
            Err("second".to_string())
        );
        assert_eq!(effective_prompt_override(&cell, "fallback"), "first");
    }

    #[test]
    fn static_prompt_composer_storage_returns_rejected_composer() {
        let cell = std::sync::OnceLock::new();
        let first: Box<StaticPromptComposer> =
            Box::new(|ctx| format!("first:{}", ctx.default_layers.len()));
        let second: Box<StaticPromptComposer> =
            Box::new(|ctx| format!("second:{}", ctx.default_layers.len()));

        assert!(set_static_prompt_composer(&cell, first).is_ok());
        let rejected = set_static_prompt_composer(&cell, second)
            .expect_err("second composer should be rejected");
        let ctx = StaticPromptCtx {
            model_id: "deepseek-v4-pro",
            personality: Personality::Calm,
            default_layers: "fallback",
        };

        assert_eq!(rejected(&ctx), "second:8");
        assert_eq!(
            cell.get().expect("first composer retained")(&ctx),
            "first:8"
        );
    }

    #[test]
    fn static_prompt_composer_unset_keeps_default_layers_byte_identical() {
        for personality in [Personality::Calm, Personality::Playful] {
            let default_layers = compose_default_static_layers(personality, "deepseek-v4-flash");
            let composed = apply_static_prompt_composer(
                None,
                personality,
                "deepseek-v4-flash",
                &default_layers,
            );

            assert_byte_identical("unset static prompt composer", &default_layers, &composed);
        }
    }

    #[test]
    fn static_prompt_composer_receives_context_and_replaces_layers() {
        let default_layers = compose_default_static_layers(Personality::Calm, "deepseek-v4-pro");
        let composer: Box<StaticPromptComposer> = Box::new(|ctx| {
            assert_eq!(ctx.model_id, "deepseek-v4-pro");
            assert_eq!(ctx.personality, Personality::Calm);
            // The 0.9.0 core is model-agnostic ("You are CodeWhale") and
            // folds tone in — no per-model id line, no separate personality
            // section in default_layers.
            assert!(ctx.default_layers.contains("You are CodeWhale"));
            assert!(
                ctx.default_layers
                    .contains("Take the work seriously. Don't take")
            );
            assert!(!ctx.default_layers.contains("## Core Tool Taxonomy"));
            assert!(!ctx.default_layers.contains("Approval Policy"));
            "embedder static prompt".to_string()
        });

        let composed = apply_static_prompt_composer(
            Some(composer.as_ref()),
            Personality::Calm,
            "deepseek-v4-pro",
            &default_layers,
        );

        assert_eq!(composed, "embedder static prompt");
    }

    fn contains_cjk(text: &str) -> bool {
        text.chars().any(|ch| {
            matches!(
                ch,
                '\u{3040}'..='\u{30ff}'
                    | '\u{3400}'..='\u{4dbf}'
                    | '\u{4e00}'..='\u{9fff}'
                    | '\u{f900}'..='\u{faff}'
            )
        })
    }

    #[test]
    fn agent_mode_carries_execution_discipline_block() {
        for phrase in [
            "Execution Discipline",
            "Do not end with \"I'll check\"",
            "After spawning a background shell or sub-agent",
            "<codewhale:subagent.done>",
            "verify load-bearing claims",
        ] {
            assert!(
                AGENT_MODE.contains(phrase),
                "AGENT_MODE missing execution-discipline phrase {phrase:?}"
            );
        }
        assert!(
            !BASE_PROMPT.contains("<tool_persistence>")
                && !BASE_PROMPT.contains("Tool-use enforcement"),
            "0.9.0 base constitution should not carry the old execution-discipline tail"
        );
    }

    #[test]
    fn base_prompt_carries_constitutional_core() {
        for phrase in [
            "## CodeWhale",
            "You are CodeWhale",
            "The A is already yours",
            "Let the work speak",
            "### Ground truth",
            "### Verify before you claim",
            "### Do what's asked",
            "### Restraint",
            "### Whose word wins",
        ] {
            assert!(
                BASE_PROMPT.contains(phrase),
                "BASE_PROMPT missing Constitutional phrase {phrase:?}"
            );
        }
    }

    #[test]
    fn constitutional_hierarchy_keeps_user_turn_above_local_law() {
        let heading_at = BASE_PROMPT
            .find("### Whose word wins")
            .expect("Whose word wins heading present");
        let user_at = BASE_PROMPT
            .find("1. The user's request, this turn.")
            .expect("user request tier present");
        let constitution_at = BASE_PROMPT
            .find("2. This constitution.")
            .expect("constitution tier present");
        let project_at = BASE_PROMPT
            .find("3. Project law and instructions")
            .expect("project tier present");
        let preference_at = BASE_PROMPT
            .find("4. Your standing user-global preferences.")
            .expect("user-global preference tier present");
        let memory_at = BASE_PROMPT
            .find("5. Memory and previous-session handoffs.")
            .expect("memory/handoff tier present");

        assert!(
            heading_at < user_at
                && user_at < constitution_at
                && constitution_at < project_at
                && project_at < preference_at
                && preference_at < memory_at,
            "Whose word wins must rank the current user request above constitution, \
             project law, standing user-global preferences, then memory/handoffs"
        );
        assert!(
            BASE_PROMPT.contains("the user may override a fact, but no one may invent\none"),
            "Whose word wins must keep ground truth overridable but never inventable"
        );
        assert!(
            BASE_PROMPT.contains("A tie you cannot break is not yours to break"),
            "Whose word wins must keep tie-break escalation"
        );
    }

    #[test]
    fn base_prompt_is_model_fact_free() {
        for placeholder in [
            "{model_id}",
            "{context_window_note}",
            "{subagent_economics}",
            "{model_thinking_note}",
            "{model_characteristics}",
        ] {
            assert!(
                !BASE_PROMPT.contains(placeholder),
                "0.9.0 BASE_PROMPT must not contain model-fact placeholder {placeholder}"
            );
        }
        for forbidden in [
            "Your V4 Characteristics",
            "Model Characteristics",
            "one-million-token context window",
            "provider-dependent and not known",
        ] {
            assert!(
                !BASE_PROMPT.contains(forbidden),
                "0.9.0 BASE_PROMPT must not contain model-specific fact {forbidden:?}"
            );
        }
    }

    fn assert_no_unresolved_model_placeholders(prompt: &str) {
        for placeholder in [
            "{model_id}",
            "{context_window_note}",
            "{subagent_economics}",
            "{model_thinking_note}",
            "{model_characteristics}",
        ] {
            assert!(
                !prompt.contains(placeholder),
                "composed prompt must not contain unresolved {placeholder}"
            );
        }
    }

    #[test]
    fn compose_prompt_for_v4_model_stays_model_fact_free() {
        let prompt =
            compose_prompt_with_approval_model_and_shell(Personality::Calm, "deepseek-v4-pro");
        assert!(prompt.contains("You are CodeWhale"));
        assert!(!prompt.contains("Your V4 Characteristics"));
        assert!(!prompt.contains("one-million-token context window"));
        assert_no_unresolved_model_placeholders(&prompt);
    }

    #[test]
    fn compose_prompt_for_kimi_stays_model_fact_free() {
        let prompt =
            compose_prompt_with_approval_model_and_shell(Personality::Calm, "moonshotai/kimi-k2.6");
        assert!(prompt.contains("You are CodeWhale"));
        assert!(!prompt.contains("Your V4 Characteristics"));
        assert!(!prompt.contains("one-million"));
        assert!(!prompt.contains("$0.14"));
        assert!(!prompt.contains("262144-token context window"));
        assert!(!prompt.contains("Models may emit *thinking tokens*"));
        assert_no_unresolved_model_placeholders(&prompt);
    }

    #[test]
    fn compose_prompt_for_openai_api_gpt_55_stays_model_fact_free() {
        let prompt = compose_prompt_with_approval_model_and_shell(Personality::Calm, "gpt-5.5");
        assert!(prompt.contains("You are CodeWhale"));
        assert!(!prompt.contains("Your V4 Characteristics"));
        assert!(!prompt.contains("1050000-token context window"));
        assert!(!prompt.contains("Models may emit *thinking tokens*"));
        assert!(!prompt.contains("provider-dependent and not known"));
        assert_no_unresolved_model_placeholders(&prompt);
    }

    #[test]
    fn compose_prompt_for_unknown_model_stays_model_fact_free() {
        let prompt =
            compose_prompt_with_approval_model_and_shell(Personality::Calm, "llama3.3:70b");
        assert!(prompt.contains("You are CodeWhale"));
        assert!(!prompt.contains("Your V4 Characteristics"));
        assert!(!prompt.contains("one-million"));
        assert!(!prompt.contains("$0.14"));
        assert!(!prompt.contains("provider-dependent and not known"));
        assert!(!prompt.contains("Models may emit *thinking tokens*"));
        assert_no_unresolved_model_placeholders(&prompt);
    }

    #[test]
    fn apply_model_template_replaces_placeholder() {
        let result = apply_model_template("You are {model_id}", "deepseek-v4-pro", None);
        assert_eq!(result, "You are deepseek-v4-pro");
        assert!(!result.contains("{model_id}"));
    }

    #[test]
    fn apply_model_template_does_not_resolve_removed_model_fact_templates() {
        let result = apply_model_template("{context_window_note}", "gpt-5.5", Some(400_000));
        assert_eq!(result, "{context_window_note}");
        assert!(!result.contains("400000-token context window"));
        assert!(!result.contains("1050000-token context window"));
    }

    #[test]
    fn compose_prompt_is_model_agnostic_in_preamble() {
        // 0.9.0 keeps the preamble byte-for-byte the same regardless of
        // model id, and no {model_id} placeholder leaks.
        let flash =
            compose_prompt_with_approval_model_and_shell(Personality::Calm, "deepseek-v4-flash");
        let kimi =
            compose_prompt_with_approval_model_and_shell(Personality::Calm, "moonshotai/kimi-k2.6");
        assert!(
            flash.contains("You are CodeWhale"),
            "0.9.0 preamble must open with the model-agnostic CodeWhale stance"
        );
        assert!(
            !flash.contains("You are deepseek-v4-flash")
                && !kimi.contains("You are moonshotai/kimi-k2.6"),
            "0.9.0 preamble must not inject a per-model identity line"
        );
        assert!(
            !flash.contains("{model_id}") && !kimi.contains("{model_id}"),
            "composed prompt must not contain the raw {{model_id}} placeholder"
        );
    }

    #[test]
    fn tool_descriptions_carry_edit_and_shell_guidance() {
        let write = WriteFileTool.description();
        assert!(write.contains("instead of heredocs") && write.contains("exec_shell"));

        let edit = EditFileTool.description();
        assert!(edit.contains("read_file"));
        assert!(edit.contains("apply_patch") && edit.contains("write_file"));

        let patch = ApplyPatchTool.description();
        assert!(patch.contains("unified-diff") && patch.contains("transactional"));

        let shell = ExecShellTool.description();
        assert!(shell.contains("background=true"));
        assert!(shell.contains("task_shell_start"));
        assert!(shell.contains(">5 seconds"));
    }

    #[test]
    fn composed_prompt_no_longer_inlines_tool_taxonomy() {
        let prompt =
            compose_prompt_with_approval_model_and_shell(Personality::Calm, "deepseek-v4-pro");
        // The core tool taxonomy (grep_files / git_status / run_tests hints)
        // is no longer prepended as a standalone "## Core Tool Taxonomy" block.
        // It now lives inside the "## Runtime Policy Reference" section of the
        // system prompt, scoped under each mode sub-heading.
        assert!(!prompt.contains("## Core Tool Taxonomy"));
        assert!(!prompt.contains("## Toolbox"));
        assert!(prompt.contains("You are CodeWhale"));
    }

    #[test]
    fn plan_prompt_taxonomy_omits_run_tests() {
        let taxonomy = render_core_tool_taxonomy_body(AppMode::Plan);
        // Plan taxonomy should omit execution tools (verified at the source).
        assert!(
            taxonomy.contains("for discovery") && taxonomy.contains("for git inspection"),
            "Plan taxonomy should keep read-only discovery and git guidance"
        );
        assert!(
            !taxonomy.contains("run_tests")
                && !taxonomy.contains("run_verifiers")
                && !taxonomy.contains("exec_shell"),
            "Plan taxonomy must not mention run_tests, run_verifiers, or exec_shell"
        );
        // The taxonomy block is rendered correctly but no longer inlined
        // into the base system prompt — it lives inside the
        // "## Runtime Policy Reference" section of the system prompt,
        // scoped under each mode sub-heading.
    }

    #[test]
    fn core_tool_taxonomy_only_references_default_active_tools() {
        let core_tools = crate::core::engine::default_active_native_tool_names();
        for tool in TOOL_TAXONOMY_DISCOVERY
            .iter()
            .chain(TOOL_TAXONOMY_GIT)
            .chain(TOOL_TAXONOMY_VERIFICATION)
        {
            assert!(
                core_tools.contains(tool),
                "tool taxonomy references {tool}, but it is not in the eager native-tool list"
            );
        }
    }

    #[test]
    fn authority_recap_appears_in_full_prompt() {
        let tmp = tempdir().expect("tempdir");
        let text = match system_prompt_for_mode_with_context_skills_session_and_approval(
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext::default(),
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert!(
            text.contains("## Authority Recap"),
            "full system prompt must contain the authority recap"
        );
        assert!(
            text.contains("CodeWhale's constitution governs your behavior"),
            "authority recap must reference the Constitution"
        );
        assert!(
            text.contains("consult ### Whose word wins"),
            "authority recap must point at 0.9.0's precedence section"
        );
    }

    #[test]
    fn system_prompt_merges_workspace_and_configured_skills_dir() {
        let _env_guard = crate::test_support::lock_test_env();
        let tmp = tempdir().expect("tempdir");
        let _home = ScopedHome::set(tmp.path().join("home"));
        let workspace = tmp.path().join("workspace");
        let configured_dir = tmp.path().join("configured-skills");
        write_test_skill(
            &workspace.join(".claude").join("skills"),
            "workspace-skill",
            "workspace skill",
        );
        write_test_skill(&configured_dir, "configured-skill", "configured skill");

        let text = match system_prompt_for_mode_with_context_and_skills(
            &workspace,
            None,
            Some(&configured_dir),
            None,
            None,
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };

        assert!(text.contains("workspace-skill"));
        assert!(text.contains("configured-skill"));
    }

    struct ScopedHome {
        previous: Option<std::ffi::OsString>,
    }

    impl ScopedHome {
        fn set(path: std::path::PathBuf) -> Self {
            let previous = std::env::var_os("HOME");
            // Safety: this test serializes environment access with
            // lock_test_env and restores HOME in Drop.
            unsafe {
                std::env::set_var("HOME", path);
            }
            Self { previous }
        }
    }

    impl Drop for ScopedHome {
        fn drop(&mut self) {
            // Safety: this test serializes environment access with
            // lock_test_env and restores HOME in Drop.
            unsafe {
                if let Some(previous) = self.previous.take() {
                    std::env::set_var("HOME", previous);
                } else {
                    std::env::remove_var("HOME");
                }
            }
        }
    }

    fn write_test_skill(root: &std::path::Path, name: &str, description: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).expect("skill dir");
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n"),
        )
        .expect("skill file");
    }

    #[test]
    fn constitution_has_no_separate_personality_tier() {
        // 0.9.0 has no personality tier. Voice and tone live in the
        // compact constitution rather than a separate section, so
        // personality remains folded in by omission.
        let prompt = compose_prompt(Personality::Calm);
        assert!(
            !prompt.contains("Personality: Calm — Tier 8"),
            "Personality tier should not appear as a separate section"
        );
        assert!(
            prompt.contains("Take the work seriously. Don't take"),
            "Preamble should carry tone guidance (take the work, not yourself, seriously)"
        );
        // Verify the preamble still carries the CodeWhale identity.
        assert!(prompt.contains("You are CodeWhale"));
        assert!(prompt.contains("Let the work speak"));
    }

    #[test]
    fn execution_discipline_lives_in_agent_mode_after_core_constitution() {
        let discipline_at = AGENT_MODE
            .find("###### Execution Discipline")
            .expect("Execution Discipline anchor present");
        let longevity_at = AGENT_MODE
            .find("###### Session Longevity")
            .expect("Session Longevity anchor present");
        assert!(
            longevity_at < discipline_at,
            "agent-mode execution discipline should follow the shorter mode/longevity delta"
        );
        assert!(
            !BASE_PROMPT.contains("Execution Discipline")
                && !BASE_PROMPT.contains("<tool_persistence>"),
            "base constitution should stay reduced; execution discipline belongs to Agent mode"
        );
    }

    #[test]
    fn plan_mode_prompt_uses_update_plan_as_confirmation_handoff() {
        assert!(
            PLAN_MODE.contains("call `update_plan`"),
            "Plan mode must tell the model to finish plans through update_plan"
        );
        assert!(
            PLAN_MODE.contains("accept / revise / exit prompt"),
            "Plan mode must explain why update_plan is the UI handoff signal"
        );
    }

    #[test]
    fn render_environment_block_lists_supplied_locale_and_workspace() {
        let tmp = tempdir().expect("tempdir");
        let block = render_environment_block(tmp.path(), "zh-Hans");
        assert!(block.starts_with("## Environment"));
        assert!(block.contains("- lang: zh-Hans"));
        assert!(block.contains(&format!(
            "- codewhale_version: {}",
            env!("CARGO_PKG_VERSION")
        )));
        // pwd is now delivered per-turn via `turn_meta`, not in the static block.
        assert!(!block.contains("- pwd:"));
        assert!(block.contains("- platform:"));
        assert!(block.contains("- shell:"));
    }

    #[test]
    fn locale_reinforcement_preamble_returns_native_script_for_supported_locales() {
        // English (and unknown locales) get None — the existing English
        // directive in `constitution.md` is sufficient.
        assert!(locale_reinforcement_preamble("en").is_none());
        assert!(locale_reinforcement_preamble("en-US").is_none());
        assert!(locale_reinforcement_preamble("fr-FR").is_none());
        assert!(locale_reinforcement_preamble("").is_none());

        // zh-Hans (and the de-facto equivalents the TUI accepts) get a
        // native-script preamble. The text must explicitly mention
        // `reasoning_content` (the V4 knob this is meant to steer) and
        // preserve tool-name immutability — those are the load-bearing
        // claims behind the #1118 fix that someone could quietly
        // delete in a future translation pass.
        for tag in ["zh-Hans", "zh-CN", "zh"] {
            let preamble =
                locale_reinforcement_preamble(tag).expect("zh-Hans preamble should exist");
            assert!(
                preamble.contains("简体中文"),
                "zh preamble must be in Simplified Chinese: {preamble:?}"
            );
            assert!(
                preamble.contains("reasoning_content"),
                "zh preamble must steer reasoning_content: {preamble:?}"
            );
            assert!(
                preamble.contains("read_file"),
                "zh preamble must call out tool-name immutability: {preamble:?}"
            );
        }

        let ja = locale_reinforcement_preamble("ja").expect("ja preamble");
        assert!(ja.contains("日本語"), "ja preamble must be in Japanese");
        assert!(ja.contains("reasoning_content"));

        let pt = locale_reinforcement_preamble("pt-BR").expect("pt-BR preamble");
        assert!(
            pt.contains("português do Brasil"),
            "pt preamble must call out pt-BR explicitly"
        );
        assert!(pt.contains("reasoning_content"));
    }

    #[test]
    fn system_prompt_prepends_locale_preamble_for_zh_hans() {
        // Build the full system prompt with locale=zh-Hans and assert
        // the native-script preamble shows up *before* the English
        // base-prompt body. Cache stability and attention precedence
        // both depend on this ordering.
        let tmp = tempdir().expect("tempdir");
        let text = match system_prompt_for_mode_with_context_skills_session_and_approval(
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: None,
                project_context_pack_enabled: false,
                locale_tag: "zh-Hans",
                translation_enabled: false,
                model_id: "codewhale",
                context_window_override: None,
                show_thinking: true,
                verbosity: None,
                skills_scan_codewhale_only: false,
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        let preamble_marker = "## 语言要求";
        let base_marker = "You are CodeWhale";
        let preamble_pos = text
            .find(preamble_marker)
            .expect("zh-Hans preamble should be present");
        let base_pos = text
            .find(base_marker)
            .expect("base prompt should be present");
        assert!(
            preamble_pos < base_pos,
            "locale preamble must precede the English base prompt (preamble={preamble_pos}, base={base_pos})",
        );
    }

    #[test]
    fn locale_reinforcement_closer_returns_native_script_for_supported_locales() {
        // English (and unknown locales) get None.
        assert!(locale_reinforcement_closer("en").is_none());
        assert!(locale_reinforcement_closer("fr-FR").is_none());
        assert!(locale_reinforcement_closer("").is_none());

        // Each supported locale gets a closer in its own script that
        // explicitly tells the model "don't drift to English even as
        // English context accumulates" — that's the load-bearing claim
        // behind the bookend pattern.
        let zh = locale_reinforcement_closer("zh-Hans").expect("zh closer");
        assert!(
            zh.contains("简体中文"),
            "zh closer must be in Simplified Chinese"
        );
        assert!(
            zh.contains("reasoning_content"),
            "zh closer must steer reasoning_content"
        );
        let ja = locale_reinforcement_closer("ja").expect("ja closer");
        assert!(ja.contains("日本語"), "ja closer must be in Japanese");
        assert!(ja.contains("reasoning_content"));
        let pt = locale_reinforcement_closer("pt-BR").expect("pt-BR closer");
        assert!(pt.contains("português do Brasil"));
        assert!(pt.contains("reasoning_content"));
    }

    #[test]
    fn system_prompt_bookends_zh_hans_with_preamble_and_closer() {
        // The full system prompt for zh-Hans must contain BOTH the
        // opening preamble (`## 语言要求`) and the closing reinforcement
        // (`## 语言再次提醒`), with the closer appearing AFTER the
        // preamble — i.e. the prompt is "bookended" in native script,
        // matching the empirical finding from the WeChat thread that
        // motivated the closer.
        let tmp = tempdir().expect("tempdir");
        let text = match system_prompt_for_mode_with_context_skills_session_and_approval(
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: None,
                project_context_pack_enabled: false,
                locale_tag: "zh-Hans",
                translation_enabled: false,
                model_id: "codewhale",
                context_window_override: None,
                show_thinking: true,
                verbosity: None,
                skills_scan_codewhale_only: false,
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        let preamble_pos = text
            .find("## 语言要求")
            .expect("zh-Hans preamble must be in prompt");
        let closer_pos = text
            .find("## 语言再次提醒")
            .expect("zh-Hans closer must be in prompt");
        assert!(
            preamble_pos < closer_pos,
            "closer must come after preamble (preamble={preamble_pos}, closer={closer_pos})",
        );
        // The closer must be the very last block — anything else after
        // it defeats the recency-bias purpose. Skip the closer's own
        // `## ` header before scanning.
        let closer_header_end = closer_pos + "## 语言再次提醒".len();
        let after_closer_body = &text[closer_header_end..];
        assert!(
            !after_closer_body.contains("\n## "),
            "no other top-level section should follow the closer; got: {after_closer_body:?}",
        );
    }

    #[test]
    fn hidden_thinking_uses_english_reasoning_without_locale_bookends() {
        let tmp = tempdir().expect("tempdir");
        let text = match system_prompt_for_mode_with_context_skills_session_and_approval(
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: None,
                project_context_pack_enabled: false,
                locale_tag: "zh-Hans",
                translation_enabled: false,
                model_id: "codewhale",
                context_window_override: None,
                show_thinking: false,
                verbosity: None,
                skills_scan_codewhale_only: false,
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };

        assert!(
            text.contains("## Hidden Thinking Language"),
            "hidden thinking prompt must include the request-side language override"
        );
        assert!(
            text.contains("reasoning_content") && text.contains("English"),
            "hidden thinking override must steer reasoning_content to English"
        );
        assert!(
            text.contains("final reply") && text.contains("Simplified Chinese"),
            "hidden thinking override must preserve the visible reply language"
        );
        assert!(
            !text.contains("## 语言要求") && !text.contains("## 语言再次提醒"),
            "hidden thinking prompt must not also ask for localized reasoning"
        );

        let hidden_pos = text
            .find("## Hidden Thinking Language")
            .expect("hidden thinking block present");
        let hidden_header_end = hidden_pos + "## Hidden Thinking Language".len();
        let after_hidden_body = &text[hidden_header_end..];
        assert!(
            !after_hidden_body.contains("\n## "),
            "hidden thinking override must be the final top-level block; got: {after_hidden_body:?}",
        );
    }

    #[test]
    fn system_prompt_skips_locale_preamble_for_english() {
        // English locale → no preamble injected. Asserts the
        // "preamble is opt-in for non-English" invariant.
        let tmp = tempdir().expect("tempdir");
        let text = match system_prompt_for_mode_with_context_skills_session_and_approval(
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: None,
                project_context_pack_enabled: false,
                locale_tag: "en",
                translation_enabled: false,
                model_id: "codewhale",
                context_window_override: None,
                show_thinking: true,
                verbosity: None,
                skills_scan_codewhale_only: false,
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert!(
            !text.contains("语言要求"),
            "English locale must not get a zh preamble: {text:?}"
        );
        assert!(
            !text.contains("言語要件"),
            "English locale must not get a ja preamble: {text:?}"
        );
        assert!(
            !text.contains("Requisito de Idioma"),
            "English locale must not get a pt-BR preamble: {text:?}"
        );
        // Closer too — same bookend rule.
        assert!(
            !text.contains("语言再次提醒"),
            "English locale must not get a zh closer: {text:?}"
        );
        assert!(
            !text.contains("言語再確認"),
            "English locale must not get a ja closer: {text:?}"
        );
        assert!(
            !text.contains("Reforço de Idioma"),
            "English locale must not get a pt-BR closer: {text:?}"
        );
        assert!(
            !contains_cjk(BASE_PROMPT),
            "base prompt must not contain static CJK priming tokens"
        );
        for mode in [AppMode::Agent, AppMode::Plan, AppMode::Yolo] {
            let taxonomy = render_core_tool_taxonomy_body(mode);
            assert!(
                !contains_cjk(&taxonomy),
                "tool taxonomy must not contain static CJK priming tokens: {taxonomy:?}"
            );
        }
        // Do not assert on arbitrary CJK in the full system prompt: project
        // context may legitimately contain localized file names, README text,
        // or user-authored instructions. The locale bookend markers above are
        // the priming tokens this test is meant to guard.
    }

    #[test]
    fn locale_bookends_carry_reasoning_content_directives_for_1118() {
        // #1118 ("Language has been configured to Chinese, but thinking
        // outputs are still in English"): after the 0.9.0 constitution
        // reduction, locale-native bookends carry the runtime language
        // reinforcement instead of the base constitution.
        let lang = LOCALE_PREAMBLE_ZH_HANS;
        assert!(
            lang.contains("reasoning_content"),
            "locale preamble must explicitly call out reasoning_content"
        );
        assert!(
            lang.contains("最终回复"),
            "locale preamble must explicitly cover the final reply"
        );
        assert!(
            lang.contains("代码") && lang.contains("工具名称"),
            "code and tool names must be named as non-language signals"
        );
        assert!(
            LOCALE_CLOSER_ZH_HANS.contains("reasoning_content")
                && LOCALE_CLOSER_ZH_HANS.contains("继续用简体中文思考和回答"),
            "closing bookend must preserve recency-positioned language reinforcement"
        );
        // Explicit-user-override clause keeps the prompt useful for the
        // opposite preference (#1118 commenters who want English
        // thinking for token-cost reasons).
        let phrase = "think in English";
        assert!(
            lang.contains(phrase) && LOCALE_CLOSER_ZH_HANS.contains(phrase),
            "expected the user-override example `{phrase}`"
        );
    }

    #[test]
    fn environment_block_is_inserted_into_system_prompt() {
        let tmp = tempdir().expect("tempdir");
        let prompt = match system_prompt_for_mode_with_context_skills_and_session(
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: None,
                project_context_pack_enabled: true,
                locale_tag: "ja",
                translation_enabled: false,
                model_id: "codewhale",
                context_window_override: None,
                show_thinking: true,
                verbosity: None,
                skills_scan_codewhale_only: false,
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert!(prompt.contains("## Environment"));
        assert!(prompt.contains("- lang: ja"));
        assert!(prompt.contains("- codewhale_version:"));
    }

    #[test]
    fn user_global_constitution_block_is_injected_separately() {
        let _env_guard = crate::test_support::lock_test_env();
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        let codewhale_home = tmp.path().join("codewhale-home");
        std::fs::create_dir_all(&codewhale_home).expect("codewhale home");
        let _codewhale_home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());

        let constitution = codewhale_config::UserConstitution {
            about: Some("Maintains CodeWhale release lanes.".to_string()),
            working_style: vec!["Prefer live verification before claims.".to_string()],
            priorities: vec!["Keep release gates green.".to_string()],
            autonomy_preference: codewhale_config::AutonomyPreference::Balanced,
            ..codewhale_config::UserConstitution::default()
        };
        constitution
            .save_to(
                &codewhale_home
                    .join(codewhale_config::user_constitution::USER_CONSTITUTION_FILE_NAME),
            )
            .expect("save user constitution");

        let prompt = match system_prompt_for_mode_with_context_skills_and_session(
            &workspace,
            None,
            None,
            None,
            PromptSessionContext {
                project_context_pack_enabled: false,
                ..PromptSessionContext::default()
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };

        let base_at = prompt.find("### Whose word wins").expect("base prompt");
        let user_block_at = prompt
            .find("<codewhale_user_constitution")
            .expect("user constitution block");
        let env_at = prompt
            .find("- codewhale_version:")
            .expect("rendered environment block");
        assert!(
            base_at < user_block_at && user_block_at < env_at,
            "user constitution should be its own layer after the base/project context and before volatile environment data"
        );
        assert!(prompt.contains("source=\"user-global\""));
        assert!(prompt.contains("Maintains CodeWhale release lanes."));
        assert!(prompt.contains("Prefer live verification before claims."));
        assert!(
            !prompt.contains(&codewhale_home.display().to_string()),
            "prompt should use the stable user-global source label, not a device-specific home path"
        );
    }

    #[test]
    fn bundled_choice_disables_user_global_constitution_block() {
        let _env_guard = crate::test_support::lock_test_env();
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        let codewhale_home = tmp.path().join("codewhale-home");
        std::fs::create_dir_all(&codewhale_home).expect("codewhale home");
        let _codewhale_home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());

        let constitution = codewhale_config::UserConstitution {
            about: Some("This file should stay inactive.".to_string()),
            ..codewhale_config::UserConstitution::default()
        };
        constitution
            .save_to(
                &codewhale_home
                    .join(codewhale_config::user_constitution::USER_CONSTITUTION_FILE_NAME),
            )
            .expect("save user constitution");

        let mut state = codewhale_config::SetupState::default();
        state.complete_constitution_checkpoint(
            "0.8.67",
            codewhale_config::ConstitutionChoice::Bundled,
        );
        state
            .save_to(&codewhale_home.join(codewhale_config::setup_state::SETUP_STATE_FILE_NAME))
            .expect("save setup state");

        let prompt = match system_prompt_for_mode_with_context_skills_and_session(
            &workspace,
            None,
            None,
            None,
            PromptSessionContext {
                project_context_pack_enabled: false,
                ..PromptSessionContext::default()
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };

        assert!(!prompt.contains("<codewhale_user_constitution"));
        assert!(!prompt.contains("This file should stay inactive."));
    }

    #[test]
    fn invalid_user_global_constitution_is_skipped() {
        let _env_guard = crate::test_support::lock_test_env();
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        let codewhale_home = tmp.path().join("codewhale-home");
        std::fs::create_dir_all(&codewhale_home).expect("codewhale home");
        std::fs::write(
            codewhale_home.join(codewhale_config::user_constitution::USER_CONSTITUTION_FILE_NAME),
            "{ not valid json",
        )
        .expect("write invalid user constitution");
        let _codewhale_home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", codewhale_home.as_os_str());

        let prompt = match system_prompt_for_mode_with_context_skills_and_session(
            &workspace,
            None,
            None,
            None,
            PromptSessionContext {
                project_context_pack_enabled: false,
                ..PromptSessionContext::default()
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };

        assert!(!prompt.contains("<codewhale_user_constitution"));
    }

    #[test]
    fn memory_guidance_carries_paired_examples() {
        // The fragment is the contract — verify the verbatim ✓ / ✗
        // pair is present so V4 has both shapes to imitate.
        assert!(MEMORY_GUIDANCE.contains("declarative facts"));
        assert!(MEMORY_GUIDANCE.contains(" ✓"));
        assert!(MEMORY_GUIDANCE.contains(" ✗"));
        assert!(MEMORY_GUIDANCE.contains("Imperative"));
    }

    #[test]
    fn memory_guidance_does_not_claim_moraine_tools_are_always_available() {
        assert!(!MEMORY_GUIDANCE.contains("You have access to Moraine MCP tools"));
        assert!(MEMORY_GUIDANCE.contains("When a `moraine-mcp` server is configured"));
        assert!(MEMORY_GUIDANCE.contains("current tool catalog exposes"));
        assert!(MEMORY_GUIDANCE.contains("search_sessions"));
        assert!(
            !MEMORY_GUIDANCE.contains("searchsessions"),
            "Moraine search tool spelling must stay consistent"
        );
    }

    #[test]
    fn memory_guidance_absent_when_no_memory_block() {
        let tmp = tempdir().expect("tempdir");
        let prompt = match system_prompt_for_mode_with_context_skills_and_session(
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: None,
                project_context_pack_enabled: false,
                locale_tag: "en",
                translation_enabled: false,
                model_id: "codewhale",
                context_window_override: None,
                show_thinking: true,
                verbosity: None,
                skills_scan_codewhale_only: false,
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert!(
            !prompt.contains("Memory Hygiene"),
            "memory guidance must not leak into sessions without a memory block"
        );
    }

    #[test]
    fn memory_guidance_appended_after_memory_block() {
        let tmp = tempdir().expect("tempdir");
        let block = "## User Memory\n\n- prefers Rust\n";
        let prompt = match system_prompt_for_mode_with_context_skills_and_session(
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext {
                user_memory_block: Some(block),
                goal_objective: None,
                project_context_pack_enabled: false,
                locale_tag: "en",
                translation_enabled: false,
                model_id: "codewhale",
                context_window_override: None,
                show_thinking: true,
                verbosity: None,
                skills_scan_codewhale_only: false,
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        let mem_at = prompt.find("User Memory").expect("user memory present");
        let guide_at = prompt.find("Memory Hygiene").expect("guidance present");
        assert!(
            mem_at < guide_at,
            "guidance must come after the user memory block"
        );
    }

    #[test]
    fn memory_guidance_matches_constitutional_tier_order() {
        let guidance = MEMORY_GUIDANCE
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let current_request_at = guidance
            .find("the user's current request (Tier 2)")
            .expect("current request tier present");
        let statutes_at = guidance
            .find("Statutes (Tier 3)")
            .expect("statutes tier present");
        let local_law_at = guidance
            .find("Local Law (Tier 5)")
            .expect("local law tier present");
        let live_evidence_at = guidance
            .find("live evidence (Tier 6)")
            .expect("live evidence tier present");

        assert!(
            current_request_at < statutes_at
                && statutes_at < local_law_at
                && local_law_at < live_evidence_at,
            "memory guidance must keep the current request above memory and local law"
        );
    }

    #[test]
    fn project_context_pack_can_be_disabled() {
        let tmp = tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("README.md"), "# Pack test").expect("write readme");
        let prompt = match system_prompt_for_mode_with_context_skills_and_session(
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: None,
                project_context_pack_enabled: false,
                locale_tag: "en",
                translation_enabled: false,
                model_id: "codewhale",
                context_window_override: None,
                show_thinking: true,
                verbosity: None,
                skills_scan_codewhale_only: false,
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
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: None,
                project_context_pack_enabled: true,
                locale_tag: "en",
                translation_enabled: false,
                model_id: "codewhale",
                context_window_override: None,
                show_thinking: true,
                verbosity: None,
                skills_scan_codewhale_only: false,
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert!(prompt.contains("<project_context_pack>"));
        assert!(
            prompt.find("<project_context_pack>").expect("pack")
                < prompt.find("## Previous Session Relay").expect("relay")
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
            "# Session relay — prior\n\n## Active task\nFinish #32.\n\n## Open blockers\n- [ ] write the basic version\n",
        )
        .unwrap();

        let prompt = match system_prompt_for_mode_with_context(workspace, None) {
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
        let prompt = match system_prompt_for_mode_with_context(tmp.path(), None) {
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
        let prompt = match system_prompt_for_mode_with_context(tmp.path(), None) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert!(!prompt.contains(HANDOFF_BLOCK_MARKER));
    }

    #[test]
    fn compose_prompt_includes_all_layers() {
        let prompt = compose_prompt(Personality::Calm);
        // Base layer — 0.9.0 compact Constitution
        assert!(prompt.contains("## CodeWhale"));
        assert!(prompt.contains("### Whose word wins"));
        assert!(!prompt.contains("## STATUTES (Tier 2)"));
        assert!(!prompt.contains("## EVIDENCE (Tier 6)"));
        // Mode and approval are not inlined — they travel as
        // request-time runtime metadata.
        assert!(!prompt.contains("Mode: Agent"));
        assert!(!prompt.contains("Approval Policy:"));
    }

    /// `constitution.md` is the single hand-maintained source of the compact
    /// constitutional core. This replaces the old 600-line policy tail: a
    /// hand-edit that drops a core section or reorders the skeleton fails the
    /// build instead of silently shipping a malformed prompt.
    #[test]
    fn constitution_md_carries_required_structure() {
        let md = BASE_PROMPT;
        assert!(md.contains("## CodeWhale"), "missing title");
        let mut cursor = 0usize;
        for needle in [
            "## CodeWhale",
            "### Ground truth",
            "### Verify before you claim",
            "### Do what's asked",
            "### Restraint",
            "### Whose word wins",
        ] {
            let pos = md
                .find(needle)
                .unwrap_or_else(|| panic!("ordering check: {needle:?} not found"));
            assert!(
                pos >= cursor,
                "cache-stable ordering broken: {needle:?} at {pos} precedes a previous section at {cursor}"
            );
            cursor = pos + needle.len();
        }
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
        let prompt = compose_prompt(Personality::Calm);
        let base_pos = prompt.find("## CodeWhale").unwrap();
        let article_pos = prompt.find("### Ground truth").unwrap();

        assert!(base_pos < article_pos);
    }

    #[test]
    fn base_prompt_is_mode_agnostic() {
        // Mode and approval text are no longer inlined into compose_prompt —
        // they travel as request-time runtime metadata.
        let prompt = compose_prompt(Personality::Calm);
        assert!(!prompt.contains("Mode: Agent"));
        assert!(!prompt.contains("Mode: YOLO"));
        assert!(!prompt.contains("Mode: Plan"));
        assert!(!prompt.contains("Approval Policy:"));
        // Base prompt carries the 0.9.0 compact Constitution.
        assert!(prompt.contains("You are CodeWhale"));
        assert!(prompt.contains("Take the work seriously. Don't take"));
    }

    #[test]
    fn mode_prompts_remain_small_deltas_not_base_policy_copies() {
        for (name, prompt) in [
            ("agent", AGENT_MODE),
            ("plan", PLAN_MODE),
            ("yolo", YOLO_MODE),
        ] {
            // Measure semantic size on LF so Windows autocrlf checkouts do not
            // inflate char/3 token estimates via extra `\r` bytes.
            let normalized = prompt.replace("\r\n", "\n").replace('\r', "\n");
            let word_count = normalized.split_whitespace().count();
            let estimated_tokens =
                crate::compaction::estimate_text_tokens_conservative(&normalized);
            let max_words = if name == "agent" { 800 } else { 350 };
            let max_tokens = if name == "agent" { 1600 } else { 700 };

            assert!(
                word_count <= max_words,
                "{name} mode prompt should remain a delta, got {word_count} words"
            );
            assert!(
                estimated_tokens <= max_tokens,
                "{name} mode prompt should remain compact, got {estimated_tokens} estimated tokens"
            );
            for forbidden in [
                "## CodeWhale",
                "## STATUTES (Tier 2)",
                "## REGULATIONS (Tier 3)",
                "## EVIDENCE (Tier 6)",
                "## Context Management",
                "## Runtime Policy Reference",
            ] {
                assert!(
                    !normalized.contains(forbidden),
                    "{name} mode prompt duplicated shared base section {forbidden:?}"
                );
            }
        }
    }

    #[test]
    fn mode_prompts_do_not_inline_full_approval_policy_overlays() {
        for (name, mode_prompt) in [
            ("agent", AGENT_MODE),
            ("plan", PLAN_MODE),
            ("yolo", YOLO_MODE),
        ] {
            for (approval_name, approval_prompt) in [
                ("auto", AUTO_APPROVAL),
                ("suggest", SUGGEST_APPROVAL),
                ("never", NEVER_APPROVAL),
            ] {
                assert!(
                    !mode_prompt.contains(approval_prompt.trim()),
                    "{name} mode prompt must not inline the full {approval_name} approval overlay"
                );
            }
        }

        assert!(
            PLAN_MODE.contains("All writes and patches are blocked"),
            "Plan may summarize the user-facing mode delta"
        );
        assert!(
            NEVER_APPROVAL.contains("This approval policy is a Tier 2 Statute"),
            "the approval overlay keeps the policy authority explanation"
        );
    }

    #[test]
    fn approval_policy_no_longer_inlined_in_base_prompt() {
        let prompt = compose_prompt(Personality::Calm);
        assert!(!prompt.contains("Mode: Agent"));
        assert!(!prompt.contains("Approval Policy:"));
        // The compact Constitutional preamble is still present.
        assert!(prompt.contains("You are CodeWhale"));
    }

    #[test]
    fn personality_is_folded_into_constitution() {
        // v4 has no separate personality tier. Voice and tone live in
        // the preamble, so both Calm and Playful compose_prompt calls
        // produce identical output (no personality overlay is appended).
        let calm = compose_prompt(Personality::Calm);
        let playful = compose_prompt(Personality::Playful);
        assert_eq!(
            calm, playful,
            "personality enum is a no-op — both produce identical output"
        );
        assert!(calm.contains("Take the work seriously. Don't take"));
        assert!(calm.contains("You are CodeWhale"));
    }

    #[test]
    fn compact_template_is_included_in_full_prompt() {
        let tmp = tempdir().expect("tempdir");
        let prompt = match system_prompt_for_mode_with_context(tmp.path(), None) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert!(prompt.contains("## Compaction Relay"));
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
    fn session_goal_is_injected_below_compact_template() {
        let tmp = tempdir().expect("tempdir");
        let prompt = match system_prompt_for_mode_with_context_skills_and_session(
            tmp.path(),
            Some("## Repo Working Set\nsrc/lib.rs"),
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: Some("Fix transcript corruption"),
                project_context_pack_enabled: true,
                locale_tag: "en",
                translation_enabled: false,
                model_id: "codewhale",
                context_window_override: None,
                show_thinking: true,
                verbosity: None,
                skills_scan_codewhale_only: false,
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };

        let goal_pos = prompt.find("<session_goal>").expect("goal block");
        let compact_pos = prompt.find("## Compaction Relay").expect("compact block");

        assert!(prompt.contains("Fix transcript corruption"));
        // Session goal is volatile content — it lives below the
        // volatile-content boundary (after the compact template) so
        // per-session goal changes don't bust the prefix cache for
        // static layers.
        assert!(compact_pos < goal_pos);
        assert!(!prompt.contains("src/lib.rs"));
    }

    #[test]
    fn empty_session_goal_is_not_injected() {
        let tmp = tempdir().expect("tempdir");
        let prompt = match system_prompt_for_mode_with_context_skills_and_session(
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: Some("   "),
                project_context_pack_enabled: true,
                locale_tag: "en",
                translation_enabled: false,
                model_id: "codewhale",
                context_window_override: None,
                show_thinking: true,
                verbosity: None,
                skills_scan_codewhale_only: false,
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };

        assert!(!prompt.contains("<session_goal>"));
        assert!(!prompt.contains("## Current Goal"));
    }

    #[test]
    fn agent_mode_tool_guidance_avoids_defensive_tool_suppression() {
        let prompt = compose_prompt(Personality::Calm);
        assert!(!prompt.contains("Tool Selection Guide"));
        assert!(AGENT_MODE.contains("Delegate only independent, fire-and-forget work"));
        assert!(AGENT_MODE.contains("Use `rlm_open`"));
        assert!(
            !AGENT_MODE.contains("When NOT to use certain tools"),
            "agent mode should steer tool choice without training the model to avoid available tools"
        );
        assert!(
            !AGENT_MODE.contains("Don't reach for"),
            "avoid defensive anti-tool wording in mode guidance"
        );
    }

    /// #588: after the 0.9.0 constitution reduction, language-mirroring
    /// reinforcement lives in its own static segment plus locale bookends.
    #[test]
    fn language_segment_present_outside_reduced_constitution() {
        let prompt = compose_prompt(Personality::Calm);
        assert!(
            !BASE_PROMPT.contains("## Language"),
            "0.9.0 constitution.md should stay reduced; language belongs in its own segment"
        );
        assert!(
            LANGUAGE_PROMPT.contains("## Language") && prompt.contains("## Language"),
            "default static prompt must still include the language segment"
        );
        assert!(
            LANGUAGE_PROMPT.contains("latest user message first")
                && LANGUAGE_PROMPT.contains("README.zh-CN.md")
                && LANGUAGE_PROMPT.contains("tool results")
                && LANGUAGE_PROMPT
                    .contains("even when the `lang` field in `## Environment` is `en`")
                && LANGUAGE_PROMPT.contains("Use the `lang` field only when"),
            "language segment must preserve the old default language-selection contract"
        );
        assert!(
            LANGUAGE_PROMPT.contains("reasoning_content")
                && prompt.contains("reasoning_content")
                && LOCALE_PREAMBLE_ZH_HANS.contains("reasoning_content")
                && LOCALE_CLOSER_ZH_HANS.contains("reasoning_content"),
            "language segment and locale bookends must keep the reasoning_content anchor"
        );
    }

    #[test]
    fn output_formatting_segment_present_outside_reduced_constitution() {
        let prompt = compose_prompt(Personality::Calm);
        assert!(
            !BASE_PROMPT.contains("## Output Formatting"),
            "0.9.0 constitution.md should stay reduced; output formatting belongs in its own segment"
        );
        assert!(OUTPUT_PROMPT.contains("## Output Formatting"));
        assert!(prompt.contains("## Output Formatting"));
        assert!(prompt.contains("terminal, not a browser"));
        assert!(prompt.contains("Markdown tables almost never render correctly"));
    }

    #[test]
    fn locale_bookends_resist_english_context_drift() {
        assert!(
            LOCALE_PREAMBLE_ZH_HANS.contains("reasoning_content")
                && LOCALE_CLOSER_ZH_HANS.contains("reasoning_content"),
            "locale bookends must keep the reasoning_content anchor"
        );
        assert!(
            LOCALE_CLOSER_ZH_HANS.contains("英文代码")
                && LOCALE_CLOSER_ZH_HANS.contains("用户的语言决定"),
            "closing locale bookend must explicitly resist English-context drift"
        );
        assert!(
            LOCALE_PREAMBLE_ZH_HANS.contains("代码、文件路径、工具名称"),
            "opening locale bookend must keep code/tool tokens untranslated"
        );
    }

    #[test]
    fn english_base_prompt_avoids_native_script_language_priming() {
        let prompt = compose_prompt(Personality::Calm);
        assert!(
            !contains_cjk(&prompt),
            "English base prompt should keep native-script reinforcement in locale bookends only"
        );
        assert!(
            !prompt.contains("multilingual coding agent"),
            "identity should not prime language switching; language belongs in runtime bookends"
        );
    }

    #[test]
    fn rlm_specialty_tool_guidance_present() {
        assert!(AGENT_MODE.contains("Large Context Tools"));
        for tool in [
            "rlm_open",
            "rlm_eval",
            "rlm_configure",
            "rlm_close",
            "handle_read",
        ] {
            assert!(
                AGENT_MODE.contains(tool),
                "AGENT_MODE should mention `{tool}`"
            );
        }

        let descriptions = [
            RlmOpenTool.description().to_string(),
            RlmEvalTool::new(None).description().to_string(),
            RlmConfigureTool.description().to_string(),
            RlmCloseTool.description().to_string(),
            HandleReadTool.description().to_string(),
        ]
        .join("\n");
        let rlm_count = descriptions.to_lowercase().matches("rlm").count();
        assert!(
            rlm_count >= 5,
            "RLM tool descriptions present: expected >= 5 mentions of 'rlm', got {rlm_count}"
        );
        assert!(
            !AGENT_MODE.contains("When NOT to use RLM"),
            "RLM guidance should explain fit and verification without telling the model to avoid the tool"
        );
    }

    /// Project instructions rank above memory, with the nearest scope winning
    /// over the broader. The embedder-injected-instructions case is covered
    /// by project law/instructions sitting above memory/handoffs.
    #[test]
    fn project_instructions_outrank_memory_in_whose_word_wins() {
        let prompt = compose_prompt(Personality::Calm);
        let project_at = prompt
            .find("3. Project law and instructions")
            .expect("Whose word wins must rank project instructions");
        let memory_at = prompt
            .find("5. Memory and previous-session handoffs.")
            .expect("Whose word wins must rank memory below project instructions");
        assert!(
            project_at < memory_at,
            "project instructions must outrank memory so embedder-injected \
             instructions are not treated as mere memory preferences"
        );
    }

    #[test]
    fn workspace_orientation_guidance_present() {
        let prompt = compose_prompt(Personality::Calm);
        assert!(prompt.contains("Project law and instructions"));
        assert!(
            prompt.contains("the nearest in\nscope winning over the broader")
                || prompt.contains("the nearest in scope winning over the broader"),
            "Whose word wins must keep the nearest-scope-wins rule for project instructions"
        );
    }

    #[test]
    fn prompt_uses_single_agent_and_rlm_surface() {
        for tool in [
            "rlm_open",
            "rlm_eval",
            "rlm_configure",
            "rlm_close",
            "handle_read",
        ] {
            assert!(
                AGENT_MODE.contains(tool),
                "AGENT_MODE should mention tool `{tool}`"
            );
        }
        assert!(AGENT_MODE.contains("sub-agent"));
    }

    #[test]
    fn prompt_documents_fork_context_prefix_cache_contract() {
        let source = include_str!("tools/subagent/mod.rs");
        for haystack in [AGENT_MODE, source] {
            assert!(haystack.contains("fork_context"));
            assert!(haystack.contains("byte-identical"));
            assert!(haystack.contains("DeepSeek prefix-cache reuse"));
        }
        assert!(AGENT_MODE.contains("Fresh sessions are the default"));
    }

    #[test]
    fn prompt_documents_explicit_subagent_model_strength() {
        let prompt = AGENT_MODE;
        assert!(prompt.contains("model_strength: \"same\""));
        assert!(prompt.contains("model_strength: \"faster\""));
        assert!(prompt.contains("type: \"explore\""));
        assert!(include_str!("tools/subagent/mod.rs").contains("Overrides model_strength"));
        assert!(prompt.contains("defaults to `model_strength: \"faster\"`"));
        assert!(prompt.contains("2-4 `type: \"explore\"` sub-agents"));
        assert!(prompt.contains("self-reports"));
    }

    #[test]
    fn prompt_documents_structured_subagent_briefs() {
        let prompt = AGENT_MODE;
        for field in [
            "Subagent Brief",
            "QUESTION",
            "SCOPE",
            "ALREADY_KNOWN",
            "EFFORT",
            "STOP_CONDITION",
            "VERDICT",
            "EVIDENCE",
            "GAPS",
            "NEXT",
        ] {
            assert!(
                prompt.contains(field),
                "main prompt should include Subagent Brief field `{field}`"
            );
        }
        assert!(prompt.contains("Brief sub-agents with a compact Subagent Brief"));
    }

    #[test]
    fn prompt_bounds_explore_without_tiny_cap_for_implementers() {
        let prompt = AGENT_MODE;
        assert!(prompt.contains("Explore briefs default to `quick`"));
        assert!(prompt.contains("read-only"));
        assert!(prompt.contains("3-5 tool calls"));
        assert!(prompt.contains("Review/verifier children stop after decisive evidence"));
        assert!(prompt.contains("No fan-out without a fan-in owner"));
    }

    #[test]
    fn agent_mode_prompt_teaches_automatic_workflow_use() {
        // #4125: parent decides Workflow without the user saying the word;
        // indicates the shape and may ask setup questions before launch.
        let prompt = AGENT_MODE;
        for phrase in [
            "You decide when to use Workflow",
            "need **not** say \"workflow\"",
            "broad, independent, or staged",
            "This looks set up for a Workflow",
            "`request_user_input`",
            "TUI question modal",
            "Pass **paths**, not file contents",
            "Prefer `responseSchema`",
            "one compact summary",
        ] {
            assert!(
                prompt.contains(phrase),
                "AGENT_MODE missing automatic-workflow phrase {phrase:?}"
            );
        }
        // Explicitly not the old opt-in-only framing.
        assert!(
            !prompt.contains("The `workflow` tool is opt-in"),
            "AGENT_MODE must not describe Workflow as opt-in only"
        );
    }

    #[test]
    fn operate_mode_prompt_prefers_workflow_plan_over_handwritten_files() {
        // #4125 companion: Operate mode matches soft-auto Workflow guidance.
        for phrase in [
            "Decide to use Workflow yourself",
            "does not need to say \"workflow\"",
            "This looks like a Workflow",
            "do not ask the operator to write workflow files",
            "Pass **paths** not file dumps",
            "Prefer `responseSchema`",
            "labels and phase titles",
        ] {
            assert!(
                OPERATE_MODE.contains(phrase),
                "OPERATE_MODE missing automatic-workflow phrase {phrase:?}"
            );
        }
    }

    #[test]
    fn subagent_done_sentinel_section_present() {
        assert!(AGENT_MODE.contains("<codewhale:subagent.done>"));
        assert!(AGENT_MODE.contains("not user input"));
        assert!(AGENT_MODE.contains("verify load-bearing claims"));
        assert!(AGENT_MODE.contains("never generate fake sentinels"));
        assert!(AGENT_MODE.contains("Do not tell the user they pasted sentinels"));
    }

    #[test]
    fn preamble_carries_tone_and_ownership_guidance() {
        let prompt = compose_prompt(Personality::Calm);
        assert!(prompt.contains("The A is already yours"));
        assert!(prompt.contains("Your competence is a settled fact"));
        assert!(prompt.contains("Take the work seriously. Don't take"));
        assert!(prompt.contains("Let the work speak"));
    }

    #[test]
    fn legacy_constants_still_available() {
        // Verify the legacy .txt constant still compiles and contains expected content
        assert!(AGENT_PROMPT.lines().next().is_some());
    }

    // ── Cache-prefix stability harness (#263 step 2) ───────────────────────
    //
    // These tests pin the byte-stability invariant required for DeepSeek's
    // KV prefix cache to hit: any prompt-construction surface that ends up
    // in the cached prefix must produce identical bytes given identical
    // inputs across calls.

    use crate::test_support::{EnvVarGuard, assert_byte_identical};

    #[test]
    fn compose_prompt_is_byte_stable_across_calls() {
        // Suspect #4 from #263: mode prompt churn within a single mode.
        // Two calls with identical (mode, personality) inputs must produce
        // identical bytes — anything else is a cache buster.
        for personality in [Personality::Calm, Personality::Playful] {
            let a = compose_prompt(personality);
            let b = compose_prompt(personality);
            assert_byte_identical(
                &format!("compose_prompt(personality={personality:?})"),
                &a,
                &b,
            );
        }
    }

    #[test]
    fn system_prompt_for_mode_with_context_is_byte_stable_for_unchanged_workspace() {
        // Same workspace, no working_set / skills churn between calls →
        // identical bytes. This pins the most representative production
        // surface (engine.rs builds the system prompt via this fn or
        // its sibling _and_skills variant on every turn).
        let _env_guard = crate::test_support::lock_test_env();
        let workspace_tmp = tempdir().expect("workspace tempdir");
        let home_tmp = tempdir().expect("home tempdir");
        let _home = EnvVarGuard::set("HOME", home_tmp.path().as_os_str());
        let _userprofile = EnvVarGuard::set("USERPROFILE", home_tmp.path().as_os_str());
        let _skills_dir = EnvVarGuard::remove("DEEPSEEK_SKILLS_DIR");
        let workspace = workspace_tmp.path();

        let a = match system_prompt_for_mode_with_context(workspace, None) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        let b = match system_prompt_for_mode_with_context(workspace, None) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert_byte_identical(
            "system_prompt_for_mode_with_context() on empty workspace",
            &a,
            &b,
        );
    }

    #[test]
    fn system_prompt_ignores_working_set_summary_argument() {
        // Working-set metadata is now injected into the latest user message
        // per turn. The legacy argument remains for call-site compatibility
        // but must not reintroduce volatile bytes into the system prompt.
        let _env_guard = crate::test_support::lock_test_env();
        let tmp = tempdir().expect("tempdir");
        let home_tmp = tempdir().expect("home tempdir");
        let _home = EnvVarGuard::set("HOME", home_tmp.path().as_os_str());
        let _userprofile = EnvVarGuard::set("USERPROFILE", home_tmp.path().as_os_str());
        let _skills_dir = EnvVarGuard::remove("DEEPSEEK_SKILLS_DIR");
        let workspace = tmp.path();
        let summary = "## Repo Working Set\nWorkspace: /tmp/x\n";

        let a = match system_prompt_for_mode_with_context(workspace, Some(summary)) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        let b = match system_prompt_for_mode_with_context(workspace, Some(summary)) {
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
        // rendered prompt must produce identical bytes. The relay block
        // lands below the static boundary in
        // `system_prompt_for_mode_with_context_and_skills`.
        let _env_guard = crate::test_support::lock_test_env();
        let tmp = tempdir().expect("tempdir");
        let home_tmp = tempdir().expect("home tempdir");
        let _home = EnvVarGuard::set("HOME", home_tmp.path().as_os_str());
        let _userprofile = EnvVarGuard::set("USERPROFILE", home_tmp.path().as_os_str());
        let _skills_dir = EnvVarGuard::remove("DEEPSEEK_SKILLS_DIR");
        let workspace = tmp.path();
        let handoff_dir = workspace.join(".deepseek");
        std::fs::create_dir_all(&handoff_dir).unwrap();
        std::fs::write(
            handoff_dir.join("handoff.md"),
            "# Session relay\n\n## Active task\nFinish #280.\n\n## Open blockers\n- [ ] none\n",
        )
        .unwrap();

        let a = match system_prompt_for_mode_with_context(workspace, None) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        let b = match system_prompt_for_mode_with_context(workspace, None) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert_byte_identical(
            "system_prompt_for_mode_with_context with constant handoff file",
            &a,
            &b,
        );
        assert!(a.contains(HANDOFF_BLOCK_MARKER), "relay must be embedded");
        assert!(a.contains("Finish #280."), "relay body must be present");
    }

    #[test]
    fn handoff_appears_after_static_blocks_without_working_set() {
        // Cache-prefix invariant: the relay block must come after static
        // `## Context Management` and the compaction relay template
        // (`## Compaction Relay`). Working-set metadata is per-turn user
        // metadata now, not a system-prompt tail block.
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path();
        let handoff_dir = workspace.join(".deepseek");
        std::fs::create_dir_all(&handoff_dir).unwrap();
        std::fs::write(handoff_dir.join("handoff.md"), "# handoff body\n").unwrap();

        let summary = "## Repo Working Set\nWorkspace: /tmp/x\n";
        let prompt = match system_prompt_for_mode_with_context(workspace, Some(summary)) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };

        let context_pos = prompt
            .find("## Context Management")
            .expect("Context Management section present in Agent mode");
        let compact_pos = prompt
            .find("## Compaction Relay")
            .expect("compaction relay template present");
        let handoff_pos = prompt
            .find(HANDOFF_BLOCK_MARKER)
            .expect("relay block present when fixture file exists");
        assert!(
            !prompt.contains("## Repo Working Set"),
            "working-set summary must stay out of the system prompt"
        );

        assert!(
            context_pos < handoff_pos,
            "## Context Management must precede the relay block"
        );
        assert!(
            compact_pos < handoff_pos,
            "## Compaction Relay must precede the relay block"
        );
    }

    #[test]
    fn render_instructions_block_returns_none_for_empty_input() {
        let empty: &[super::InstructionSource] = &[];
        assert!(super::render_instructions_block(empty).is_none());
    }

    #[test]
    fn render_instructions_block_skips_missing_files_with_warning() {
        let tmp = tempdir().expect("tempdir");
        let real = tmp.path().join("real.md");
        std::fs::write(&real, "real content here").unwrap();
        let bogus = tmp.path().join("does-not-exist.md");

        let block = super::render_instructions_block(&[bogus.clone().into(), real.clone().into()])
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

        let block = super::render_instructions_block(&[a.into(), b.into()]).expect("non-empty");
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

        let block =
            super::render_instructions_block(&[empty.into(), real.into()]).expect("non-empty");
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

        let block = super::render_instructions_block(&[big.into()]).expect("non-empty");
        assert!(block.contains("[…truncated:"), "truncation marker missing");
        // Block should be much smaller than the original file.
        assert!(
            block.len() < 110 * 1024,
            "block should be capped near 100 KiB"
        );
    }

    /// `InstructionSource::Inline` bypasses disk reads — the content is used
    /// directly and `name` becomes the `<instructions source="…">` attribute.
    /// Empty / oversize handling mirrors `File` variant.
    #[test]
    fn render_instructions_block_handles_inline_source() {
        let block = super::render_instructions_block(&[super::InstructionSource::Inline {
            name: "embedded:test/template".to_string(),
            content: "INLINE_MARKER_CONTENT".to_string(),
        }])
        .expect("non-empty");
        assert!(block.contains("INLINE_MARKER_CONTENT"));
        assert!(block.contains("source=\"embedded:test/template\""));

        // Empty inline → skipped just like empty file.
        let empty_inline = super::InstructionSource::Inline {
            name: "empty".to_string(),
            content: "   ".to_string(),
        };
        assert!(super::render_instructions_block(&[empty_inline]).is_none());

        // Oversize inline → truncated with elided marker.
        let big_inline = super::InstructionSource::Inline {
            name: "huge".to_string(),
            content: "Y".repeat(200 * 1024),
        };
        let trimmed = super::render_instructions_block(&[big_inline]).expect("non-empty");
        assert!(trimmed.contains("[…truncated:"));

        // File + Inline 混用,顺序保持。
        let tmp = tempdir().expect("tempdir");
        let file_path = tmp.path().join("file-first.md");
        std::fs::write(&file_path, "FILE_MARKER").unwrap();
        let mixed = super::render_instructions_block(&[
            file_path.into(),
            super::InstructionSource::Inline {
                name: "inline-second".to_string(),
                content: "INLINE_MARKER".to_string(),
            },
        ])
        .expect("non-empty");
        let file_pos = mixed.find("FILE_MARKER").expect("file rendered");
        let inline_pos = mixed.find("INLINE_MARKER").expect("inline rendered");
        assert!(file_pos < inline_pos, "声明顺序必须保留(File then Inline)");
    }

    #[test]
    fn instructions_block_appears_in_system_prompt_when_configured() {
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path();
        let extra = workspace.join("extra-instructions.md");
        std::fs::write(&extra, "EXTRA_INSTRUCTIONS_MARKER_BODY").unwrap();

        let extra_source: super::InstructionSource = extra.clone().into();
        let prompt = match super::system_prompt_for_mode_with_context_and_skills(
            workspace,
            None,
            None,
            Some(std::slice::from_ref(&extra_source)),
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

    #[test]
    fn verbosity_concise_appends_discipline_block() {
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path();
        let prompt = match super::system_prompt_for_mode_with_context_skills_session_and_approval(
            workspace,
            None,
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: None,
                project_context_pack_enabled: false,
                locale_tag: "en",
                translation_enabled: false,
                model_id: "codewhale",
                context_window_override: None,
                show_thinking: true,
                verbosity: Some(" Concise "),
                skills_scan_codewhale_only: false,
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };

        assert!(
            prompt.contains("## Concise Output Discipline"),
            "Concise Output Discipline should be appended"
        );
    }

    /// #2953 — the Calm overlay (calm.md) stays out of the default
    /// model-prompt path to keep the static prefix slim. Voice and tone
    /// guidance travels via the constitution preamble instead.
    #[test]
    fn default_prompt_does_not_include_calm_personality_overlay() {
        let prompt = compose_prompt(Personality::Calm);
        let calm_text = include_str!("prompts/personalities/calm.md");
        let first_calm_line = calm_text.lines().find(|l| !l.is_empty()).unwrap_or("");
        assert!(
            !prompt.contains(first_calm_line),
            "default agent prompt must not include calm.md overlay"
        );
    }

    #[test]
    fn default_prompt_stays_under_2953_static_baseline() {
        const ISSUE_2953_BASELINE_CHARS: usize = 30_461;
        let prompt = compose_prompt(Personality::Calm);

        assert!(
            prompt.chars().count() < ISSUE_2953_BASELINE_CHARS,
            "default static prompt should stay below the #2953 baseline"
        );
    }
}
