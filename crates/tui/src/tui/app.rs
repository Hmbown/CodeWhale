//! Application state for the `DeepSeek` TUI.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use ratatui::layout::Rect;
use ratatui::style::Color;
use serde_json::Value;

use codewhale_config::{ProviderChain, route::RouteLimits};

use crate::artifacts::ArtifactRecord;
use crate::client::{CacheWarmupKey, PromptInspection};
use crate::compaction::CompactionConfig;
use crate::config::{
    ApiProvider, ApprovalPolicyControl, Config, DEFAULT_TEXT_MODEL, has_api_key, has_api_key_for,
};
use crate::config_ui::ConfigUiMode;
use crate::core::authority::{ModeSessionPrefs, base_policy_for_mode};
use crate::core::events::TurnRoute;
use crate::hooks::{HookContext, HookEvent, HookExecutor, HookResult};
use crate::localization::{Locale, MessageId, resolve_locale, tr};
use crate::models::{Message, SystemPrompt, Tool, Usage};
use crate::palette::{self, UiTheme};
use crate::pricing::{CostCurrency, CostEstimate};
use crate::resource_telemetry::TokenThroughput;
use crate::session_manager::{SessionContextReference, SessionMetadata, SessionWorkState};
use crate::settings::{InlineDiffMode, Settings};
use crate::tools::plan::{PlanState, SharedPlanState, new_shared_plan_state};
use crate::tools::shell::new_shared_shell_manager;
use crate::tools::spec::RuntimeToolServices;
use crate::tools::subagent::{AgentWorkerStatus, SubAgentResult};
use crate::tools::todo::{SharedTodoList, TodoList, new_shared_todo_list};
use crate::tui::active_cell::ActiveCell;
use crate::tui::approval::ApprovalMode;
use crate::tui::clipboard::{ClipboardContent, ClipboardHandler};
use crate::tui::file_mention::ContextReference;
use crate::tui::history::{HistoryCell, TranscriptActionOwner, TranscriptRenderOptions};
use crate::tui::hotbar::HotbarActionRegistry;
use crate::tui::motion::MotionPolicy;
use crate::tui::paste_burst::{FlushResult, PasteBurst};
use crate::tui::scrolling::{MouseScrollState, TranscriptLineMeta, TranscriptScroll};
use crate::tui::selection::{SelectionAutoscroll, TranscriptSelection};
use crate::tui::shell_key_routing::Focus;
use crate::tui::streaming::StreamingState;
use crate::tui::transcript::TranscriptViewCache;
use crate::tui::views::ViewStack;

mod composer;
mod init;
mod status;
mod types;

pub use composer::ComposerHistorySearch;
pub(crate) use composer::{InputHistoryDraft, char_count};
#[cfg(test)]
pub(crate) use composer::{
    MAX_SUBMITTED_INPUT_CHARS, next_grapheme_boundary, prev_grapheme_boundary,
};
pub(crate) use status::StatusToastKind;
pub use status::{StatusToast, StatusToastLevel};
pub use types::{
    AppAction, AppMode, AppModeUi, AutomationAction, ComposerDensity, ComposerSubmitAction,
    ComposerSubmitChord, InitialInput, McpUiAction, QueuedMessage, ReasoningEffort, ScreenMode,
    SettingSelection, ShellJobAction, SubmitDisposition, TaskPanelEntry, TaskPanelEntryKind,
    ToolCollapseMode, ToolDetailRecord, TranscriptSpacing, TuiOptions, VimMode,
};
pub(crate) use types::{
    CacheReplayTarget, EffectiveReasoningEffort, GoalControlIntent, PendingGoalControl,
    WORKFLOW_DRAFT_INSTRUCTION_PREFIX,
};

// === Types ===

/// Lifecycle identity retained until the matching `TurnComplete` arrives.
///
/// This survives local cancellation clearing the visible runtime status, so
/// observer records still carry a stable id, start time, and effective route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveTurnMetadata {
    pub turn_id: String,
    pub created_at: DateTime<Utc>,
    pub route: Option<TurnRoute>,
    /// Auto decision metadata captured with this exact authoritative route.
    pub auto_route_receipt: Option<crate::model_routing::AutoRouteReceipt>,
    /// Non-secret proof of the exact endpoint + credential this turn launched
    /// against, adopted at `TurnStarted` from the engine's route receipt — not
    /// re-resolved from mutable config. Only populated for routes that can
    /// produce a follow-up prompt suggestion; see
    /// [`crate::tui::prompt_suggestion::capture_route_authority`].
    pub suggestion_authority: Option<crate::tui::prompt_suggestion::SuggestionRouteAuthority>,
}

/// Identity of the compaction pass currently rewriting session context.
///
/// The event id prevents a delayed terminal event from clearing a newer pass,
/// while `auto` selects the truthful live label in the phase strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActiveCompaction {
    pub(crate) id: String,
    pub(crate) auto: bool,
}

/// Per-message context estimates used by the render-time context meter.
/// Messages are append-only in the steady state; only the streaming tail is
/// mutable, so the tail is refreshed while older entries remain cached.
#[derive(Debug, Default)]
pub(crate) struct ContextTokenCache {
    pub(crate) message_tokens: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompletedAssistantOutputReceipt {
    history_index: usize,
    text: String,
}

impl ContextTokenCache {
    pub(crate) fn clear(&mut self) {
        self.message_tokens.clear();
    }
}

/// State machine for onboarding new users: one decision per screen, and
/// only the decisions this install genuinely needs (#3938).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingState {
    Welcome,
    /// Pick the UI locale — shown only when it cannot be confidently
    /// inferred from settings or `LC_ALL` / `LANG` (#566). Explicit picks
    /// land in the persisted settings.toml via `Settings::set("locale", …)`.
    Language,
    /// Choose a provider/model route — shown only when no usable local,
    /// authenticated, or BYOK route is configured. The canonical provider
    /// picker modal carries the choice itself.
    Provider,
    /// Trust the workspace — shown only when a trust decision is required.
    TrustDirectory,
    /// "You're ready." Enter opens the real composer pre-seeded with a
    /// first task for this folder. Never an educational surface.
    Ready,
    None,
}

pub(crate) fn resolve_skills_dir(
    workspace: &Path,
    global_skills_dir: &Path,
    config: &Config,
) -> PathBuf {
    if config.skills_config().scan_codewhale_only() {
        if config.skills_dir.is_some() {
            return global_skills_dir.to_path_buf();
        }
        if let Some(codewhale_skills_dir) = crate::skills::codewhale_workspace_skills_dir(workspace)
        {
            return codewhale_skills_dir;
        }
        return global_skills_dir.to_path_buf();
    }

    let agents_skills_dir = workspace.join(".agents").join("skills");
    if agents_skills_dir.exists() {
        return agents_skills_dir;
    }

    let local_skills_dir = workspace.join("skills");
    if local_skills_dir.exists() {
        return local_skills_dir;
    }

    if config.skills_dir.is_none()
        && let Some(global_agents) = crate::skills::agents_global_skills_dir()
        && global_agents.exists()
    {
        return global_agents;
    }

    global_skills_dir.to_path_buf()
}

pub(crate) fn looks_like_slash_command_input(input: &str) -> bool {
    let trimmed = input.trim_start();
    // `$skillname` at the start of input is treated like a slash command so the
    // skill-completion menu appears.
    let Some(rest) = trimmed
        .strip_prefix('/')
        .or_else(|| trimmed.strip_prefix('$'))
    else {
        return false;
    };
    if rest.chars().next().is_some_and(|ch| ch.is_whitespace()) {
        return false;
    }
    let Some(command) = rest.split_whitespace().next() else {
        return rest.is_empty();
    };

    !command.contains('/')
}

pub(crate) fn shell_command_from_bang_input(input: &str) -> Result<Option<&str>, &'static str> {
    let Some(rest) = input.trim_start().strip_prefix('!') else {
        return Ok(None);
    };
    let command = rest.trim();
    if command.is_empty() {
        return Err("Usage: ! <shell command>");
    }

    Ok(Some(command))
}

pub(crate) fn is_stop_word(input: &str, stop_words: &[String]) -> Option<String> {
    let trimmed = input.trim();
    let after_prefix = trimmed
        .strip_prefix('+')
        .or_else(|| trimmed.strip_prefix('!'))
        .map_or(trimmed, str::trim_start);
    let word = after_prefix.trim_end_matches(|c: char| c.is_ascii_punctuation());
    if word.is_empty() || word.chars().any(char::is_whitespace) {
        return None;
    }
    let lower = word.to_ascii_lowercase();
    stop_words
        .iter()
        .find(|stop_word| stop_word.to_ascii_lowercase() == lower)
        .cloned()
}

fn initial_onboarding_state(
    skip_onboarding: bool,
    was_onboarded: bool,
    _needs_language: bool,
    needs_api_key: bool,
    needs_workspace_trust: bool,
) -> OnboardingState {
    if skip_onboarding || (was_onboarded && !needs_api_key && !needs_workspace_trust) {
        return OnboardingState::None;
    }

    if was_onboarded && needs_api_key {
        // Missing-key recovery uses the canonical provider picker so it can
        // preserve the configured provider, endpoint, and model route before
        // asking for a replacement secret.
        OnboardingState::Provider
    } else if was_onboarded && needs_workspace_trust {
        OnboardingState::TrustDirectory
    } else {
        // First paint is the composer. Language, provider, and trust stay in
        // /setup. A 5-gate wizard must not block the first keystroke.
        OnboardingState::None
    }
}

fn onboarding_is_workspace_trust_gate(
    skip_onboarding: bool,
    was_onboarded: bool,
    needs_api_key: bool,
    needs_workspace_trust: bool,
) -> bool {
    !skip_onboarding && was_onboarded && !needs_api_key && needs_workspace_trust
}

/// Resolve the launch onboarding state and the missing-key-recovery flag in one
/// place. When the active xAI OAuth credential is missing (`xai_oauth_needs_reauth`),
/// the user already chose xAI and only needs to re-authenticate it — so the
/// generic provider picker must NOT reopen (returns `OnboardingState::None` and
/// `missing_key_recovery = false`); the caller surfaces a re-auth message (#5032).
fn launch_onboarding_decision(
    skip_onboarding: bool,
    was_onboarded: bool,
    needs_language: bool,
    needs_api_key: bool,
    needs_workspace_trust: bool,
    xai_oauth_needs_reauth: bool,
) -> (OnboardingState, bool) {
    let onboarding = if xai_oauth_needs_reauth && was_onboarded {
        OnboardingState::None
    } else {
        initial_onboarding_state(
            skip_onboarding,
            was_onboarded,
            needs_language,
            needs_api_key,
            needs_workspace_trust,
        )
    };
    let missing_key_recovery =
        !skip_onboarding && was_onboarded && needs_api_key && !xai_oauth_needs_reauth;
    (onboarding, missing_key_recovery)
}

/// One row in the per-turn cache-telemetry ring (`/cache` debug surface, #263).
#[derive(Debug, Clone)]
pub struct TurnCacheRecord {
    /// API provider used for the turn. This is recorded so cache misses can be
    /// correlated with provider/model route changes.
    pub provider: Option<ApiProvider>,
    /// Exact non-secret configured route key. This distinguishes named custom
    /// providers which all share [`ApiProvider::Custom`].
    pub provider_identity: Option<String>,
    /// Concrete model used for the turn. For auto-model turns this is the
    /// routed model, not the literal `auto` setting.
    pub model: Option<String>,
    /// Whether the route came from the auto-model selector.
    pub auto_model: bool,
    /// Provider-reported total input tokens for the turn (cache-hit +
    ///   cache-miss + uncategorized). Useful for sanity-checking that hits +
    ///   misses sum back to roughly the prompt size.
    pub input_tokens: u32,
    /// Provider-reported output tokens.
    pub output_tokens: u32,
    /// `prompt_cache_hit_tokens` from DeepSeek's usage payload. `None` when
    ///   the model in use does not report cache telemetry (see
    ///   `Capabilities::cache_telemetry_supported`).
    pub cache_hit_tokens: Option<u32>,
    /// `prompt_cache_miss_tokens`. `None` when the provider did not report it
    ///   — in that case the `/cache` formatter infers the miss as
    ///   `input_tokens − cache_hit_tokens`.
    pub cache_miss_tokens: Option<u32>,
    /// Cache-creation tokens (`cache_creation_input_tokens` on Anthropic-style
    ///   payloads). Billed at a premium where the provider publishes one, so
    ///   they are recorded as their own class rather than folded into misses.
    pub cache_write_tokens: Option<u32>,
    /// Reasoning tokens the provider reported. **Informational only**: every
    ///   provider counts these inside `output_tokens`, so they are never added
    ///   to billable output.
    pub reasoning_tokens: Option<u32>,
    /// The turn's cost with its provenance and per-class completeness, taken
    ///   from the same call that fed the session total. `None` for records made
    ///   without route provenance (legacy rows, synthetic test rows).
    pub cost_audit: Option<crate::pricing::TurnCostAudit>,
    /// Approximate tokens spent re-sending prior `reasoning_content` on
    ///   V4-thinking tool-calling turns (chars/3 heuristic). Helps separate
    ///   cache misses caused by reasoning-replay churn from misses caused by
    ///   real prefix instability.
    pub reasoning_replay_tokens: Option<u32>,
    /// Local timestamp the turn telemetry was recorded.
    pub recorded_at: Instant,
}

/// Browsing context captured when the `/model` picker is dismissed (#4109).
/// Plain data so `App` does not depend on the picker's internal view enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPickerMemory {
    /// True when the user left the picker in the full-catalog view
    /// (`A` toggle), false for the configured-only default view.
    ///
    /// Kept for backward compatibility with older dismiss events; prefer
    /// [`Self::view`] when present (#4115).
    pub catalog_view: bool,
    /// Named catalog view left open (`configured` / `catalog` / `recent` /
    /// `coding` / `cheap` / `long_context`). When `None`, [`Self::catalog_view`]
    /// is the fallback.
    pub view: Option<String>,
    /// Model row id highlighted at dismissal, if it was a real row.
    pub selected_row_id: Option<String>,
}

/// Browsing context captured when the `/provider` picker is dismissed.
/// Mirrors [`ModelPickerMemory`] so reopen restores view + highlight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderPickerMemory {
    /// True when the user left the picker in the full-catalog view
    /// (`A` toggle), false for the configured-only default view.
    pub catalog_view: bool,
    /// Provider id highlighted at dismissal, if it was a real row.
    pub selected_provider_id: Option<String>,
}

/// Bounded status vocabulary for the per-agent current-activity projection.
///
/// This is presentation state derived from structured worker/mailbox events;
/// renderers map these variants to labels but never infer them from strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentCurrentActivityStatus {
    Queued,
    Starting,
    Running,
    ModelWait,
    RunningTool,
    Waiting,
    Done,
    Failed,
    Canceled,
    Interrupted,
}

impl From<AgentWorkerStatus> for AgentCurrentActivityStatus {
    fn from(status: AgentWorkerStatus) -> Self {
        match status {
            AgentWorkerStatus::Queued => Self::Queued,
            AgentWorkerStatus::Starting => Self::Starting,
            AgentWorkerStatus::Running => Self::Running,
            AgentWorkerStatus::WaitingForUser => Self::Waiting,
            AgentWorkerStatus::ModelWait => Self::ModelWait,
            AgentWorkerStatus::RunningTool => Self::RunningTool,
            AgentWorkerStatus::Completed => Self::Done,
            AgentWorkerStatus::Failed => Self::Failed,
            AgentWorkerStatus::Cancelled => Self::Canceled,
            AgentWorkerStatus::Interrupted => Self::Interrupted,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCurrentActivity {
    pub status: AgentCurrentActivityStatus,
    /// Safe bounded context, never a raw child transcript or tool result.
    pub detail: Option<String>,
    /// Safe display name for the one tool currently executing.
    pub current_tool: Option<String>,
    pub step: Option<u32>,
}

impl AgentCurrentActivity {
    #[must_use]
    pub fn bounded(
        status: AgentCurrentActivityStatus,
        detail: Option<String>,
        current_tool: Option<String>,
        step: Option<u32>,
    ) -> Self {
        fn bounded_nonempty(value: Option<String>) -> Option<String> {
            value
                .map(|value| bound_agent_activity_text(&value))
                .filter(|value| !value.trim().is_empty())
        }

        Self {
            status,
            detail: bounded_nonempty(detail),
            current_tool: bounded_nonempty(current_tool),
            step,
        }
    }
}

/// Convert untrusted child-agent text into a compact UI-safe projection.
/// Full transcript artifacts remain the source of truth; only summaries that
/// can enter the parent transcript/sidebar pass through this seam.
pub(crate) fn bound_agent_activity_text(value: &str) -> String {
    let mut visible = String::with_capacity(value.len());
    crate::tui::osc8::strip_ansi_into(value, &mut visible);
    let redacted = codewhale_config::persistence::redact_secrets(&visible);
    crate::tui::history::summarize_tool_output(&redacted)
}

/// One bounded, structured tool outcome for the Agent Details projection.
///
/// This is populated only from `ToolCallCompleted` mailbox envelopes. It is
/// deliberately not inferred from free-form progress text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRecentAction {
    pub tool: String,
    pub step: u32,
    pub ok: bool,
}

impl AgentRecentAction {
    #[must_use]
    pub fn bounded(tool: &str, step: u32, ok: bool) -> Self {
        Self {
            tool: bound_agent_activity_text(tool),
            step,
            ok,
        }
    }
}

pub(crate) const MAX_AGENT_RECENT_ACTIONS: usize = 3;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentProgressMeta {
    pub parent_run_id: Option<String>,
    pub spawn_depth: u32,
    /// Structured, bounded answer to "what is this agent doing now?".
    pub current_activity: Option<AgentCurrentActivity>,
    /// Last tool observed running for this child. Cleared by the matching
    /// completion envelope so Work never presents a settled tool as live.
    pub current_tool: Option<String>,
    /// Successful file mutations observed for this child in this session.
    pub files_touched: u32,
    /// At most three tool outcomes observed through structured lifecycle
    /// envelopes, oldest to newest.
    pub recent_actions: VecDeque<AgentRecentAction>,
    /// Effective route facts observed from the child's installed spawn route
    /// or a later provider usage envelope. The launch event carries the exact
    /// model frozen into the child runtime; usage may confirm it while also
    /// supplying provider identity.
    pub resolved_provider: Option<String>,
    pub resolved_model: Option<String>,
    /// Tokens this child has *used* (input + output), accumulated across its
    /// own usage envelopes — the same total the worker budget tracks.
    /// `None` until the provider actually reports usage: a sub-agent whose
    /// spend is unknown renders no token figure at all rather than a
    /// fabricated `0`.
    pub received_tokens: Option<u64>,
    /// Unsettled items on this child's own To-do list, from the latest
    /// `WorkState` envelope. `None` until a real list is published — the
    /// strip never invents a `0 left` chip for agents with no checklist.
    pub todos_remaining: Option<u32>,
}

/// Per-turn LSP repair-loop summary for the Turn Inspector (#4107).
/// Observable state only — no raw diagnostic text or prompt internals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspRepairState {
    pub diagnostics_found: usize,
    pub files_touched: usize,
    pub injected: bool,
    pub repair_attempted: bool,
    /// "resolved" | "still_failing" | "unknown" | "unavailable"
    pub latest: &'static str,
}

impl Default for LspRepairState {
    fn default() -> Self {
        Self {
            diagnostics_found: 0,
            files_touched: 0,
            injected: false,
            repair_attempted: false,
            latest: "unavailable",
        }
    }
}

/// One recent session for the startup card's recent-work list (PRD 4.1).
/// Loaded once with the launch state — never on the render path — and
/// refreshed whenever the card is restored after a picker closes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchRecentSession {
    pub id: String,
    pub title: String,
    pub updated_at: DateTime<Utc>,
    pub message_count: usize,
}

/// Identity of one interactive row on the startup card. The card's rows are
/// a single ordered list — the prominent new-session entry first, then
/// recent work, then the see-all overflow — so keyboard, mouse, and paint
/// share one indexing through
/// [`crate::tui::underwater::launch_card_rows`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchRowId {
    NewSession,
    Recent(String),
    SeeAll,
}

/// How many recent sessions the startup card lists inline before the
/// see-all overflow opens the full picker.
pub(crate) const LAUNCH_RECENT_INLINE_LIMIT: usize = 5;

/// Pre-session launch menu state for the underwater shell.
///
/// This is deliberately separate from onboarding and from the post-launch
/// empty session. It selects a fresh session or recent work before the
/// transcript and composer become active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchState {
    pub visible: bool,
    pub status: Option<String>,
    /// Canonical workspace this launch state is scoped to. Recent work is
    /// the workspace's own sessions (archived and empty auto-created ones
    /// excluded, like the resume picker); the row hitboxes below are
    /// refreshed with it.
    pub workspace: PathBuf,
    /// Recent workspace sessions, most recent first, capped at
    /// [`LAUNCH_RECENT_INLINE_LIMIT`].
    pub recent: Vec<LaunchRecentSession>,
    /// All workspace sessions behind the inline list; when this exceeds
    /// `recent.len()` the card paints the see-all overflow row.
    pub total_workspace_sessions: usize,
    /// Whether launch keys type into the pre-session composer. The composer
    /// is the launch screen's one focus owner, so this is `true` from first
    /// paint. The composer itself is the session `App`'s own
    /// `ComposerState` — this flag only decides where keystrokes go.
    pub composer_focus: bool,
    /// Composer input-row hitbox from the most recent launch render (the
    /// docked strip below the option strip). A click here focuses the
    /// composer, exactly like the Tab key.
    pub composer_area: Option<Rect>,
    /// Send-glyph hitbox inside the composer row. A click here submits the
    /// composed message through the normal dispatch path.
    pub send_area: Option<Rect>,
    /// Clickable rects for the card's rows from the most recent launch
    /// render, in the same order as
    /// [`crate::tui::underwater::launch_card_rows`].
    pub row_hitboxes: Vec<(LaunchRowId, Rect)>,
    /// Card row under the pointer, if any (index into `row_hitboxes`).
    /// Painted with the shared selected-row treatment so every clickable
    /// element responds visibly on hover.
    pub hovered_row: Option<usize>,
    /// The launch card's highlighted row (index into the rows the card
    /// paints). `None` until the user arrows onto the list: nothing is
    /// pre-selected, so a reflexive Enter at launch does nothing rather
    /// than starting or resuming a session by accident (founder live-test,
    /// 2026-09-02). Esc clears it again.
    pub menu_selected: Option<usize>,
    /// Ambient-clock millisecond reading when the card began dissolving, if
    /// it has. The first keystroke or a launched command dissolves the card
    /// (founder decision, 2026-09-02).
    pub dissolve_started_ms: Option<u128>,
    /// Claude Code config was detected on this host (probed once at
    /// construction); drives the launch card's migration notice line.
    pub claude_code_detected: bool,
    /// Sixel-tier plumbing (`MarkTier::Sixel`), all `None` until used:
    /// - `sixel_cell_px`: the terminal's cell size in pixels, measured once
    ///   at startup so the raster encodes to the block's exact pixels.
    /// - `sixel_terminal_bg`: the probed terminal background, for
    ///   transparent (`Reset`) theme stages whose ground the terminal owns.
    /// - `sixel_mark_area`: the block the last launch render reserved, in
    ///   stage coordinates; reset every frame by the frame renderer.
    /// - `sixel_emitted`: the live image's block, in the same stage
    ///   coordinates (identical to screen cells in fullscreen), or `None`
    ///   when nothing is drawn. Compared against the reservation so the
    ///   event loop re-emits only on moves and clears on tier exit.
    pub sixel_cell_px: Option<(u16, u16)>,
    pub sixel_terminal_bg: Option<Color>,
    pub sixel_mark_area: Option<Rect>,
    pub sixel_emitted: Option<Rect>,
}

/// The launch card's dissolve motion budget. One bounded motion; reduced
/// motion dissolves instantly (same drawing at its endpoint).
pub(crate) const LAUNCH_CARD_DISSOLVE_MS: u128 = 240;

/// Load the startup card's recent-work list: the workspace's own sessions,
/// most recent first (`list_sessions` already sorts that way), skipping
/// archived sessions and empty auto-created ones exactly like the resume
/// picker and `--continue` do. Returns the inline-capped list plus the
/// total behind it for the see-all overflow.
fn load_launch_recent(workspace: &std::path::Path) -> (Vec<LaunchRecentSession>, usize) {
    let sessions = crate::session_manager::SessionManager::default_location()
        .and_then(|manager| manager.list_sessions())
        .unwrap_or_default();
    let mut scoped: Vec<LaunchRecentSession> = sessions
        .into_iter()
        .filter(|session| {
            !session.archived
                && !crate::session_manager::is_empty_auto_created_session(session)
                && crate::session_manager::workspace_scope_matches(&session.workspace, workspace)
        })
        .map(|session| LaunchRecentSession {
            id: session.id,
            title: session.title,
            updated_at: session.updated_at,
            message_count: session.message_count,
        })
        .collect();
    let total = scoped.len();
    scoped.truncate(LAUNCH_RECENT_INLINE_LIMIT);
    (scoped, total)
}

impl LaunchState {
    #[must_use]
    pub fn new(visible: bool, workspace: &std::path::Path) -> Self {
        let (recent, total_workspace_sessions) = load_launch_recent(workspace);
        // The launch card's migration notice is only painted when it is true:
        // Claude Code leaves its sessions under `~/.claude/projects`. One
        // stat at construction, never on the render path.
        let claude_code_detected = std::env::var_os("HOME")
            .as_ref()
            .map(|home| {
                std::path::Path::new(home)
                    .join(".claude")
                    .join("projects")
                    .is_dir()
            })
            .unwrap_or(false);
        Self {
            visible,
            status: None,
            workspace: workspace.to_path_buf(),
            recent,
            total_workspace_sessions,
            composer_focus: true,
            composer_area: None,
            send_area: None,
            row_hitboxes: Vec::new(),
            hovered_row: None,
            menu_selected: None,
            dissolve_started_ms: None,
            claude_code_detected,
            sixel_cell_px: None,
            sixel_terminal_bg: None,
            sixel_mark_area: None,
            sixel_emitted: None,
        }
    }

    /// Re-read the recent-work list from disk (same filter as
    /// construction). Called when the card is restored after a picker
    /// closes so a session created or renamed behind the picker shows up.
    pub fn refresh_recent(&mut self) {
        let (recent, total) = load_launch_recent(&self.workspace.clone());
        self.recent = recent;
        self.total_workspace_sessions = total;
    }

    /// Begin the card dissolve once (idempotent). The first keystroke or a
    /// launched command dissolves the launch card.
    pub fn dissolve_card(&mut self, now_ms: u128) {
        if self.dissolve_started_ms.is_none() {
            self.dissolve_started_ms = Some(now_ms);
        }
    }

    /// Bring the card back after a launch flow (the sessions picker) is
    /// left with Esc: every launch path has a way back to the card, so a
    /// dismissed picker never strands the user on an empty stage. The list
    /// comes back with nothing highlighted, and the recent-work list is
    /// re-read so sessions created behind the picker show up.
    pub fn restore_card(&mut self) {
        self.dissolve_started_ms = None;
        self.menu_selected = None;
        self.hovered_row = None;
        self.status = None;
        self.refresh_recent();
    }

    /// How far the card has dissolved, `[0.0 intact ..= 1.0 gone]`. Reduced
    /// motion dissolves instantly: the same drawing at its endpoint.
    #[must_use]
    pub fn card_dissolve_progress(&self, now_ms: u128, motion_allowed: bool) -> f32 {
        match self.dissolve_started_ms {
            None => 0.0,
            Some(_) if !motion_allowed => 1.0,
            Some(started) => {
                let elapsed = now_ms.saturating_sub(started);
                (elapsed as f32 / LAUNCH_CARD_DISSOLVE_MS as f32).clamp(0.0, 1.0)
            }
        }
    }
}

/// Cached @-mention completion results to avoid re-walking the filesystem when
/// the cursor moves inside the same mention token.
#[derive(Debug, Clone)]
pub struct MentionCompletionCache {
    /// Workspace root used for this completion walk.
    pub workspace: PathBuf,
    /// Process cwd captured for cwd-relative completion entries.
    pub cwd: Option<PathBuf>,
    /// The partial text after `@` that triggered this completion.
    pub partial: String,
    /// Candidate limit used for this completion walk.
    pub limit: usize,
    /// Workspace depth limit used for this completion walk. Included so live
    /// config changes invalidate cached popup results.
    pub walk_depth: usize,
    /// Completion behavior used for this walk. Included so live config changes
    /// invalidate cached popup results.
    pub behavior: String,
    /// Whether symlink following was enabled for this completion walk.
    /// Included so live config changes invalidate cached popup results.
    pub follow_links: bool,
    /// Cached completion entries.
    pub entries: Vec<String>,
}

/// Composer input state — grouped fields for the text input area.
pub struct ComposerState {
    /// Current composer text content.
    pub input: String,
    /// Cursor position within `input` (in characters).
    pub cursor_position: usize,
    /// Single-entry kill buffer for emacs-style `Ctrl+K` cut / `Ctrl+Y` yank.
    pub kill_buffer: String,
    pub paste_burst: PasteBurst,
    /// When a large paste is consolidated at submit time, the file @mention
    /// is stored here so it can be appended to the submitted text without
    /// replacing the visible composer content (#3263).
    pub(crate) pending_paste_reference: Option<String>,
    /// When composer content is oversized, the full text is stored here
    /// while `self.input` shows a truncated preview. At submit time the
    /// full text is restored for model submission (#3263).
    pub(crate) oversized_paste_full_text: Option<String>,
    pub input_history: Vec<String>,
    pub draft_history: VecDeque<String>,
    pub clear_undo_buffer: Option<String>,
    pub history_index: Option<usize>,
    pub(crate) history_navigation_draft: Option<InputHistoryDraft>,
    pub composer_history_search: Option<ComposerHistorySearch>,
    pub selected_attachment_index: Option<usize>,
    pub slash_menu_selected: usize,
    pub slash_menu_hidden: bool,
    pub mention_menu_selected: usize,
    pub mention_menu_hidden: bool,
    /// Cached @-mention completions to avoid re-walking the filesystem when
    /// the cursor moves inside the same mention token.
    pub mention_completion_cache: Option<MentionCompletionCache>,
    /// Serialized background discovery and its bounded candidate cache. All
    /// filesystem traversal for composer completions lives behind this owner.
    pub(crate) mention_discovery: crate::tui::mention_completion::MentionDiscovery,
    /// Launch directory captured once so rendering a completion popup never
    /// needs to call `getcwd` on the UI thread.
    pub(crate) mention_cwd: Option<PathBuf>,
    /// Whether vim modal editing is enabled for this composer.
    /// Sourced from `Settings::composer_vim_mode` at startup.
    pub vim_enabled: bool,
    /// Current vim editing mode.  Only meaningful when `vim_enabled` is true.
    pub vim_mode: VimMode,
    /// Pending `d` prefix for the `dd` delete-line operator.  Set when the
    /// user presses `d` in Normal mode; cleared on the next key (either `d`
    /// to complete `dd`, or any other key to cancel).
    pub vim_pending_d: bool,
    /// When set, the cursor is the active end of a text selection and
    /// `selection_anchor` is the fixed end.  Both are char-indexed.
    /// `None` means no selection is active.
    pub selection_anchor: Option<usize>,
}

impl Default for ComposerState {
    fn default() -> Self {
        Self {
            input: String::new(),
            cursor_position: 0,
            kill_buffer: String::new(),
            paste_burst: PasteBurst::default(),
            pending_paste_reference: None,
            oversized_paste_full_text: None,
            input_history: Vec::new(),
            draft_history: VecDeque::new(),
            clear_undo_buffer: None,
            history_index: None,
            history_navigation_draft: None,
            composer_history_search: None,
            selected_attachment_index: None,
            slash_menu_selected: 0,
            slash_menu_hidden: false,
            mention_menu_selected: 0,
            mention_menu_hidden: false,
            mention_completion_cache: None,
            mention_discovery: crate::tui::mention_completion::MentionDiscovery::default(),
            mention_cwd: std::env::current_dir().ok(),
            vim_enabled: false,
            vim_mode: VimMode::Normal,
            vim_pending_d: false,
            selection_anchor: None,
        }
    }
}

/// Compatibility name retained for the first Tideline header slice. New
/// surfaces register [`crate::tui::tideline::InteractionAction`] directly.
pub type HeaderActionTarget = crate::tui::tideline::InteractionAction;

/// A header target painted in the latest frame.
///
/// The visible chrome owns placement; input owns dispatch. Keeping the
/// rectangular target alongside its typed action gives mouse and keyboard
/// routes one shared destination without a second navigation system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeaderHitbox {
    pub area: Rect,
    pub target: HeaderActionTarget,
}

/// Viewport/scroll state — fields related to transcript scrolling and caching.
pub struct ViewportState {
    pub transcript_scroll: TranscriptScroll,
    pub pending_scroll_delta: i32,
    pub mouse_scroll: MouseScrollState,
    pub transcript_cache: TranscriptViewCache,
    pub transcript_selection: TranscriptSelection,
    pub selection_autoscroll: Option<SelectionAutoscroll>,
    pub transcript_scrollbar_dragging: bool,
    pub last_transcript_area: Option<Rect>,
    pub last_composer_area: Option<Rect>,
    /// Selectable targets from the latest painted frame. Cleared before every
    /// render so resized or hidden controls can never swallow a click.
    pub interaction_targets: crate::tui::tideline::InteractionRegistry,
    /// Last left-click trace over the composer, for double/triple-click
    /// word/line selection (crossterm does not decode click counts).
    pub composer_click_trace: Option<crate::tui::mouse_ui::ComposerClickTrace>,
    /// Painted band occupied by the active inline approval. Stored so wheel
    /// routing can prefer the visible card over side surfaces underneath it.
    pub last_approval_area: Option<Rect>,
    /// WorkflowPanel rect above the composer (#4121), for mouse toggle/cancel.
    pub last_workflow_panel_area: Option<Rect>,
    pub last_workflow_cancel_area: Option<Rect>,
    /// Info-line segment rects (Tideline shell, spec §6), recorded at render so
    /// hover and — in a follow-up slice — click routing can hit-test the
    /// painted cells. Mirrors the workflow-panel cancel-area storage pattern.
    pub last_infoline_hitboxes: Vec<crate::tui::infoline::InfoLineHitbox>,
    /// Live plugin CTA row above the composer, plus review/dismiss hitboxes.
    pub last_plugin_cta_area: Option<Rect>,
    pub last_plugin_cta_review_area: Option<Rect>,
    pub last_plugin_cta_dismiss_area: Option<Rect>,
    pub last_transcript_top: usize,
    pub last_transcript_visible: usize,
    pub last_transcript_total: usize,
    pub last_transcript_padding_top: usize,
    pub jump_to_latest_button_area: Option<Rect>,
    /// Inner content rect of the composer (excluding border/padding),
    /// stored at render time for mouse coordinate mapping.
    pub last_composer_content: Option<Rect>,
    /// Number of rendered text lines scrolled off the top of the composer,
    /// stored at render time for mouse coordinate mapping.
    pub last_composer_scroll_offset: usize,
    /// Vertical padding above the first text line in the composer,
    /// stored at render time for mouse coordinate mapping.
    pub last_composer_top_padding: usize,
    /// Slash-autocomplete rows painted inside the composer on the latest
    /// frame. Cleared and rewritten during `ComposerWidget::render` so a
    /// resized or closed menu cannot swallow a click. Index is the entry
    /// index into the visible slash menu (same as `slash_menu_selected`).
    pub last_slash_menu_hitboxes: RefCell<Vec<(usize, Rect)>>,
}

impl Default for ViewportState {
    fn default() -> Self {
        Self {
            transcript_scroll: TranscriptScroll::to_bottom(),
            pending_scroll_delta: 0,
            mouse_scroll: MouseScrollState::new(),
            transcript_cache: TranscriptViewCache::new(),
            transcript_selection: TranscriptSelection::default(),
            selection_autoscroll: None,
            transcript_scrollbar_dragging: false,
            last_transcript_area: None,
            last_composer_area: None,
            interaction_targets: crate::tui::tideline::InteractionRegistry::default(),
            composer_click_trace: None,
            last_approval_area: None,
            last_workflow_panel_area: None,
            last_workflow_cancel_area: None,
            last_infoline_hitboxes: Vec::new(),
            last_plugin_cta_area: None,
            last_plugin_cta_review_area: None,
            last_plugin_cta_dismiss_area: None,
            last_transcript_top: 0,
            last_transcript_visible: 0,
            last_transcript_total: 0,
            last_transcript_padding_top: 0,
            jump_to_latest_button_area: None,
            last_composer_content: None,
            last_composer_scroll_offset: 0,
            last_composer_top_padding: 0,
            last_slash_menu_hitboxes: RefCell::new(Vec::new()),
        }
    }
}

/// Host-side tracking state for the active thread goal. Mirrors the
/// engine's authoritative `SharedGoalState` snapshot (`GoalUpdated`) so the
/// sidebar, top bar, and system prompt all read one truth.
#[derive(Debug, Clone, Default)]
pub struct HostGoalState {
    pub objective: Option<String>,
    pub token_budget: Option<u32>,
    pub tokens_used: u64,
    pub time_used_seconds: u64,
    pub continuation_count: u32,
    /// Why an unfinished goal is paused. Kept separate from the four-state
    /// status so usage, budget, and run-limit stops stay distinguishable.
    pub pause_reason: Option<crate::tools::goal::GoalPauseReason>,
    pub started_at: Option<Instant>,
    /// When the goal reached a terminal status (Complete/Blocked).
    /// While `None`, elapsed time keeps growing; once set, the sidebar freezes
    /// the timer at `finished_at - started_at` so completed goals stop ticking.
    pub finished_at: Option<Instant>,
    pub status: crate::tools::goal::GoalStatus,
}

/// Session cost and token telemetry state.
#[derive(Debug, Clone)]
pub struct SessionState {
    pub session_cost: f64,
    pub session_cost_cny: f64,
    /// Priced estimate accumulated from the in-flight turn's per-step
    /// `TurnUsage` receipts, so the cost surfaces move while a long agentic
    /// turn is still running. Display-only: cleared at `TurnComplete`, which
    /// re-prices the whole turn's cumulative usage authoritatively into
    /// `session_cost`. Never persisted.
    pub pending_turn_cost: f64,
    pub pending_turn_cost_cny: f64,
    /// Display-only per-step token deltas for the active turn. These are
    /// cleared at `TurnComplete` before authoritative cumulative totals land.
    pub pending_turn_total_tokens: u32,
    pub pending_turn_input_tokens: u32,
    pub pending_turn_output_tokens: u32,
    pub pending_turn_cache_hit_tokens: u32,
    pub pending_turn_cache_miss_tokens: u32,
    pub pending_turn_cache_write_tokens: u32,
    pub subagent_cost: f64,
    pub subagent_cost_cny: f64,
    /// Redacted provider-response identities already accrued. The same
    /// fingerprints are persisted by the session and worker projections.
    pub subagent_usage_sources: HashSet<String>,
    pub displayed_cost_high_water: f64,
    pub displayed_cost_high_water_cny: f64,
    pub last_prompt_tokens: Option<u32>,
    pub last_completion_tokens: Option<u32>,
    pub last_output_throughput: Option<TokenThroughput>,
    pub last_prompt_cache_hit_tokens: Option<u32>,
    pub last_prompt_cache_miss_tokens: Option<u32>,
    pub last_reasoning_replay_tokens: Option<u32>,
    pub total_tokens: u32,
    pub total_conversation_tokens: u32,
    /// Accumulated token breakdown for the session.
    pub total_input_tokens: u32,
    pub total_cache_hit_tokens: u32,
    pub total_cache_miss_tokens: u32,
    /// Cache-creation (cache-write) tokens across the session. Tracked as its
    /// own class because providers that publish a write premium bill it above
    /// the ordinary input rate, so folding it into misses understated spend.
    pub total_cache_write_tokens: u32,
    pub total_output_tokens: u32,
    /// Turns whose route was money-metered and produced an authoritative
    /// price. These are exactly the turns inside `session_cost`.
    pub cost_priced_turns: u32,
    /// Turns whose route was money-metered — or of unknown billing basis — but
    /// produced no authoritative price, so they are missing from `session_cost`
    /// entirely. `/cost` reports this instead of presenting the subtotal as a
    /// complete figure.
    pub cost_unpriced_turns: u32,
    /// CNY-specific coverage. Most providers publish USD only, so these cannot
    /// share the USD counters without falsely calling a mixed-route CNY subtotal
    /// complete.
    pub cost_cny_priced_turns: u32,
    pub cost_cny_unpriced_turns: u32,
    /// Stable reason labels for the unpriced turns, in sorted order.
    ///
    /// `String` rather than `&'static str` because this state round-trips
    /// through a saved session: a label read back from disk was written by some
    /// build's vocabulary, not necessarily this one's.
    pub cost_unpriced_reasons: BTreeSet<String>,
    pub cost_cny_unpriced_reasons: BTreeSet<String>,
    /// Token classes used on some route this session that carry no published
    /// price. Their turns fail closed rather than under-report.
    pub cost_unpriced_classes: BTreeSet<String>,
    /// Provenance labels of the pricing rows behind the priced turns
    /// (`models_dev_bundled`, `provider_live`, `provider_docs`, …).
    pub cost_pricing_provenances: BTreeSet<String>,
    /// Live-pricing downgrade receipts: a live catalog row that could not be
    /// verified for the endpoint that served a turn, so the bundled snapshot was
    /// used instead of claiming authoritative live provenance.
    pub cost_live_pricing_defects: BTreeSet<String>,
    /// Live-pricing defects for which no bundled row could produce a price.
    pub cost_live_pricing_unusable_defects: BTreeSet<String>,
    /// One redacted receipt per distinct audited route:
    /// provider, configured identity, wire model, billing surface, endpoint
    /// fingerprint, billing mode, currency. Never a URL, credential, or filesystem path.
    pub cost_route_receipts: BTreeSet<String>,
    /// True when the restored session has no coverage state at all.
    ///
    /// Sessions written before coverage was tracked deserialize their new fields
    /// from serde defaults, which look exactly like "0 priced, 0 unpriced" — i.e.
    /// a complete total covering nothing. That reading is false, so the load path
    /// marks the session explicitly unknown and `/cost` says so rather than
    /// presenting fabricated completeness, even for an all-zero record (#4318).
    pub cost_coverage_unknown_legacy: bool,
    pub turn_cache_history: VecDeque<TurnCacheRecord>,
    pub last_cache_inspection: Option<PromptInspection>,
    pub last_warmup_key: Option<CacheWarmupKey>,
    /// Tool catalog from the most recent model request.
    ///
    /// `/cache inspect` uses this to inspect the same tool schema bytes
    /// that were eligible for the provider's prefix cache.
    pub last_tool_catalog: Option<Vec<Tool>>,
    /// Exact tool field captured at the latest model request seam.
    pub last_tool_request_snapshot: Option<crate::tool_inspection::ToolInspectionSnapshot>,
    /// API base URL used by the most recent model request or cache warmup.
    pub last_base_url: Option<String>,
}

/// Sidebar hover state for mouse tooltip support.
#[derive(Debug, Clone, Default)]
pub struct SidebarHoverState {
    /// Rendered sections with their areas and full-text lines.
    pub sections: Vec<SidebarHoverSection>,
}

/// Per-row metadata for sidebar detail popovers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarRowAction {
    Command(String),
    /// Put a destructive command in the composer instead of executing it.
    /// The user confirms with Enter or cancels by editing/clearing the draft.
    #[allow(dead_code)] // destructive confirm path; mouse_ui already matches it (TUI-DOG-008)
    PrefillCommand(String),
    /// Select the persistent Agents panel. This is deliberately a navigation
    /// action rather than a modal: the Subagents summary is a group door, so
    /// it should reveal the standing register instead of fabricating a detail
    /// page for the count itself.
    ShowSubagentsPanel,
    /// Open the child's bounded, safe status projection. Exact transcript
    /// evidence is a separate explicit action (#2889).
    OpenAgentDetail {
        agent_id: String,
    },
    /// Open the child's artifact-first exact transcript. This is separate
    /// from the safe default details projection (#2889).
    OpenAgentTranscript {
        agent_id: String,
    },
    CancelAgent {
        agent_id: String,
    },
    /// Open the Work Graph inspector in the shared pager. Any lifecycle stop
    /// action is carried into that inspector instead of consuming row width.
    InspectWork {
        title: String,
        body: String,
        stop_action: Option<Box<SidebarRowAction>>,
    },
}

impl SidebarRowAction {
    #[must_use]
    pub fn as_command(&self) -> Option<&str> {
        match self {
            Self::Command(command) => Some(command.as_str()),
            Self::PrefillCommand(_)
            | Self::ShowSubagentsPanel
            | Self::OpenAgentDetail { .. }
            | Self::OpenAgentTranscript { .. }
            | Self::CancelAgent { .. }
            | Self::InspectWork { .. } => None,
        }
    }
}

/// Per-row metadata for sidebar detail popovers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarHoverRow {
    /// Absolute row position in the terminal.
    pub row_y: u16,
    /// Text shown in the compact sidebar row.
    pub display_text: String,
    /// Full untruncated text for the popover.
    pub full_text: String,
    /// Optional additional detail line.
    pub detail: Option<String>,
    /// Whether the compact row lost information.
    pub is_truncated: bool,
    /// Slash command to execute when this row is clicked (#3028).
    /// `shell_*` job ids route through `/jobs` (e.g. `/jobs cancel
    /// shell_abc123`); task-manager ids route through `/task` (e.g.
    /// `/task show task_abc123`).
    pub click_action: Option<SidebarRowAction>,
    /// Optional narrower stop target for rows that show an inline `[x]`.
    pub stop_action: Option<SidebarRowAction>,
    pub stop_zone_start_col: Option<u16>,
    pub stop_zone_end_col: Option<u16>,
}

/// Per-section metadata for sidebar hover detection.
#[derive(Debug, Clone)]
pub struct SidebarHoverSection {
    /// Content area within the section (inside border + padding).
    pub content_area: Rect,
    /// Full original text for each content line rendered.
    pub lines: Vec<String>,
    /// Per-row metadata for rich hover popovers.
    pub rows: Vec<SidebarHoverRow>,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            session_cost: 0.0,
            session_cost_cny: 0.0,
            pending_turn_cost: 0.0,
            pending_turn_cost_cny: 0.0,
            pending_turn_total_tokens: 0,
            pending_turn_input_tokens: 0,
            pending_turn_output_tokens: 0,
            pending_turn_cache_hit_tokens: 0,
            pending_turn_cache_miss_tokens: 0,
            pending_turn_cache_write_tokens: 0,
            subagent_cost: 0.0,
            subagent_cost_cny: 0.0,
            subagent_usage_sources: HashSet::new(),
            displayed_cost_high_water: 0.0,
            displayed_cost_high_water_cny: 0.0,
            last_prompt_tokens: None,
            last_completion_tokens: None,
            last_output_throughput: None,
            last_prompt_cache_hit_tokens: None,
            last_prompt_cache_miss_tokens: None,
            last_reasoning_replay_tokens: None,
            total_tokens: 0,
            total_conversation_tokens: 0,
            total_input_tokens: 0,
            total_cache_hit_tokens: 0,
            total_cache_miss_tokens: 0,
            total_cache_write_tokens: 0,
            total_output_tokens: 0,
            cost_priced_turns: 0,
            cost_unpriced_turns: 0,
            cost_cny_priced_turns: 0,
            cost_cny_unpriced_turns: 0,
            cost_unpriced_reasons: BTreeSet::new(),
            cost_cny_unpriced_reasons: BTreeSet::new(),
            cost_unpriced_classes: BTreeSet::new(),
            cost_pricing_provenances: BTreeSet::new(),
            cost_live_pricing_defects: BTreeSet::new(),
            cost_live_pricing_unusable_defects: BTreeSet::new(),
            cost_route_receipts: BTreeSet::new(),
            cost_coverage_unknown_legacy: false,
            turn_cache_history: VecDeque::new(),
            last_cache_inspection: None,
            last_warmup_key: None,
            last_tool_catalog: None,
            last_tool_request_snapshot: None,
            last_base_url: None,
        }
    }
}

impl SessionState {
    /// Reset the accumulated token breakdown fields to zero.
    pub fn reset_token_breakdown(&mut self) {
        self.total_input_tokens = 0;
        self.total_cache_hit_tokens = 0;
        self.total_cache_miss_tokens = 0;
        self.total_cache_write_tokens = 0;
        self.total_output_tokens = 0;
        self.clear_pending_turn_usage();
        self.last_output_throughput = None;
    }

    /// Add one provider-reported model-call receipt to the display-only
    /// in-flight ledger. The cache split mirrors the authoritative
    /// `TurnComplete` accounting path.
    pub fn accrue_pending_turn_usage(&mut self, usage: &Usage) {
        self.pending_turn_total_tokens = self
            .pending_turn_total_tokens
            .saturating_add(usage.input_tokens.saturating_add(usage.output_tokens));
        self.pending_turn_input_tokens = self
            .pending_turn_input_tokens
            .saturating_add(usage.input_tokens);
        self.pending_turn_output_tokens = self
            .pending_turn_output_tokens
            .saturating_add(usage.output_tokens);
        if usage.prompt_cache_hit_tokens.is_some()
            || usage.prompt_cache_miss_tokens.is_some()
            || usage.prompt_cache_write_tokens.is_some()
        {
            let classes = crate::pricing::token_usage_for_pricing(usage);
            self.pending_turn_cache_hit_tokens = self
                .pending_turn_cache_hit_tokens
                .saturating_add(u32::try_from(classes.cache_read).unwrap_or(u32::MAX));
            self.pending_turn_cache_miss_tokens = self
                .pending_turn_cache_miss_tokens
                .saturating_add(u32::try_from(classes.input).unwrap_or(u32::MAX));
            self.pending_turn_cache_write_tokens = self
                .pending_turn_cache_write_tokens
                .saturating_add(u32::try_from(classes.cache_write).unwrap_or(u32::MAX));
        }
    }

    /// Clear the active turn's display-only token deltas before the
    /// authoritative cumulative usage is reconciled.
    pub fn clear_pending_turn_usage(&mut self) {
        self.pending_turn_total_tokens = 0;
        self.pending_turn_input_tokens = 0;
        self.pending_turn_output_tokens = 0;
        self.pending_turn_cache_hit_tokens = 0;
        self.pending_turn_cache_miss_tokens = 0;
        self.pending_turn_cache_write_tokens = 0;
    }

    pub fn displayed_total_tokens(&self) -> u32 {
        self.total_tokens
            .saturating_add(self.pending_turn_total_tokens)
    }

    pub fn displayed_total_conversation_tokens(&self) -> u32 {
        self.total_conversation_tokens
            .saturating_add(self.pending_turn_total_tokens)
    }

    pub fn displayed_total_input_tokens(&self) -> u32 {
        self.total_input_tokens
            .saturating_add(self.pending_turn_input_tokens)
    }

    pub fn displayed_total_output_tokens(&self) -> u32 {
        self.total_output_tokens
            .saturating_add(self.pending_turn_output_tokens)
    }

    pub fn displayed_total_cache_hit_tokens(&self) -> u32 {
        self.total_cache_hit_tokens
            .saturating_add(self.pending_turn_cache_hit_tokens)
    }

    pub fn displayed_total_cache_miss_tokens(&self) -> u32 {
        self.total_cache_miss_tokens
            .saturating_add(self.pending_turn_cache_miss_tokens)
    }

    pub fn displayed_total_cache_write_tokens(&self) -> u32 {
        self.total_cache_write_tokens
            .saturating_add(self.pending_turn_cache_write_tokens)
    }
}

/// Evidence collected during a turn for the post-turn receipt.
#[derive(Debug, Clone)]
pub struct ToolEvidence {
    pub tool_name: String,
    pub summary: String,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingProviderSwitch {
    pub previous_provider: ApiProvider,
    pub previous_model: String,
    pub previous_model_ids_passthrough: bool,
    pub previous_route_limits: Option<RouteLimits>,
    pub previous_route_base_url: String,
    pub previous_context_window_source: crate::route_runtime::ContextWindowSource,
    pub previous_context_window_override: Option<u32>,
    pub previous_config: Config,
    pub previous_onboarding: OnboardingState,
    pub previous_onboarding_needs_api_key: bool,
    pub previous_api_key_env_only: bool,
}

/// Opaque completion returned by a spawned dispatch task. It carries the
/// captured data needed to apply success or rollback on the event loop.
pub type DispatchApplyFn = Box<
    dyn FnOnce(
            &mut App,
            &crate::core::engine::EngineHandle,
            &crate::config::Config,
        ) -> anyhow::Result<()>
        + Send,
>;

/// Global UI state for the TUI.
#[allow(clippy::struct_excessive_bools)]
/// A route change made in-session that the user has not yet decided how to
/// save. Route changes are temporary by default; persisting them requires an
/// explicit choice (Update this Fleet / Save as a new Fleet / Remember as my
/// default / Keep for this session only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRouteSave {
    /// Provider identity the session is now on.
    pub provider_identity: String,
    /// Exact model id the session is now on.
    pub model: String,
    /// The selected Fleet at change time, when one exists.
    pub fleet: Option<(String, crate::fleet::store::FleetScope)>,
}

/// Write `provider_identity`/`model` to `settings.toml` as the route the next
/// launch should open with, and return the line to show the operator.
///
/// `default_provider` is what `App::new` consults first, so pinning it is the
/// half that actually survives a restart; the provider-scoped entry carries the
/// model. `default_model` is a DeepSeek-only legacy key and is written only for
/// those providers, matching how startup reads it back.
fn persist_route_as_startup_default(provider_identity: &str, model: &str) -> String {
    let route = format!("{provider_identity}/{model}");
    match try_persist_route_as_startup_default(provider_identity, model) {
        Ok(()) => format!("Remembered {route} as the startup default (settings.toml)."),
        Err(err) => format!("Save failed: {err}"),
    }
}

fn try_persist_route_as_startup_default(
    provider_identity: &str,
    model: &str,
) -> anyhow::Result<()> {
    crate::settings::Settings::transact(|settings| {
        settings.default_provider = Some(provider_identity.to_string());
        settings.set_model_for_provider(provider_identity, model);
        if matches!(
            crate::config::ApiProvider::parse(provider_identity),
            Some(crate::config::ApiProvider::Deepseek)
                | Some(crate::config::ApiProvider::DeepseekCN)
        ) {
            settings.set("default_model", model)?;
        }
        Ok(())
    })
}

pub struct App {
    pub mode: AppMode,
    /// Registered hotbar actions available for future slot config/render layers.
    #[allow(dead_code)]
    pub hotbar_actions: HotbarActionRegistry,
    /// Composer sub-state (input, cursor, history, menus).
    pub composer: ComposerState,
    /// Viewport sub-state (scroll, cache, selection).
    pub viewport: ViewportState,
    /// Ocean work-surface state. Kept separate from transcript/sidebar state
    /// so the replacement shell can be removed or promoted as one unit.
    pub work_surface: crate::tui::work_surface::WorkSurfaceState,
    /// Goal sub-state.
    pub goal: HostGoalState,
    /// Session sub-state (cost, tokens, telemetry).
    pub session: SessionState,
    /// Active tool restriction from custom slash command frontmatter.
    /// `None` means the current turn may use the normal tool set.
    pub active_allowed_tools: Option<Vec<String>>,
    /// True when the active custom slash command opted into pause/resume.
    pub pausable: bool,
    /// A route change made in-session awaits an explicit save decision. When
    /// set, the next key press opens the route-save prompt unless a modal is
    /// already open.
    pub pending_route_save: Option<PendingRouteSave>,
    /// True after Esc paused a pausable command and before it is resumed or cancelled.
    pub paused: bool,
    /// Saved custom-command objective while the command is paused.
    pub paused_goal_objective: Option<String>,
    pub history: Vec<HistoryCell>,
    pub history_version: u64,
    /// Bumped when destructive reindexing could make a cached Space owner name
    /// a different cell before redraw; ordinary streaming revisions preserve it.
    pub(crate) transcript_identity_epoch: u64,
    /// Per-cell revision counter, kept in lockstep with `history`.
    pub history_revisions: Vec<u64>,
    /// Cached tool-run grouping for transcript collapse. The detector is
    /// keyed by the same mutation generation that invalidates transcript
    /// cells, so idle frames do not rescan the full history.
    pub(crate) tool_run_cache: ToolRunCache,
    /// Monotonic counter used to issue fresh per-cell revisions.
    pub next_history_revision: u64,
    pub api_messages: Vec<Message>,
    /// User-visible assistant text that crossed typed completion boundaries.
    /// Receipts are aligned to transcript cells because provider context can
    /// be compacted or purged without changing what remains visible.
    completed_assistant_outputs: Vec<CompletedAssistantOutputReceipt>,
    pub(crate) context_token_cache: RefCell<ContextTokenCache>,
    /// Typed account-owned browser relay for this exact TUI session.
    pub remote_control: crate::remote_control::RemoteControlController,
    pub start_remote_control_on_launch: bool,
    pub is_loading: bool,
    /// Sender for spawned dispatch tasks to report completion back to the
    /// event loop. The closure is called with `&mut App` so the async phase
    /// never needs `&mut App` while awaiting network I/O (#4605).
    pub dispatch_completion_tx: Option<tokio::sync::mpsc::Sender<DispatchApplyFn>>,
    /// True while a spawned dispatch task is in flight (#4605). Set in the
    /// sync prepare phase and cleared when the completion closure runs, so a
    /// submit after an Esc-cancel (which clears `is_loading`) still queues
    /// instead of spawning a second dispatch that could reorder ops.
    pub dispatch_in_flight: bool,
    /// Timestamp of the most recent Enter while the engine was busy.
    /// Used by `enter_with_double_tap()` / `double_tap_window_open()` to
    /// detect a second Enter inside [`Self::DOUBLE_TAP_WINDOW`].
    pub last_enter_instant: Option<Instant>,
    /// Whether the once-per-turn provider-wait incident (#3095) has already
    /// been logged for the current turn.
    pub provider_wait_incident_logged: bool,
    /// Ghost-text follow-up suggestion shown in the composer when empty.
    /// Generated asynchronously after each completed turn; cleared on new input.
    pub prompt_suggestion: Option<String>,
    /// Monotonic turn counter for stale-suggestion protection. Incremented on
    /// each TurnStarted; background suggestion tasks capture the token and
    /// discard their result if the token no longer matches.
    pub prompt_suggestion_gen: std::sync::atomic::AtomicU64,
    /// Degraded connectivity mode; new user inputs are queued for later retry.
    pub offline_mode: bool,
    /// Whether an `EngineEvent::Error` has already been posted for the
    /// current turn. Suppresses the redundant "Turn failed:" status line
    /// that `TurnComplete { error: .. }` would otherwise emit on top of
    /// the in-transcript error cell.
    pub turn_error_posted: bool,
    /// Legacy status text sink retained for compatibility with existing call sites.
    pub status_message: Option<String>,
    /// Recent status toasts (ephemeral, newest at back).
    pub status_toasts: VecDeque<StatusToast>,
    /// Header chip label (e.g. `↑ v0.9.5`) set once by the fire-and-forget
    /// startup version check when a newer stable release exists. Drives the
    /// small persistent update chip in the header so the affordance survives
    /// the transient toast without nagging (FINISH-0.9.4 #14).
    pub update_available: Option<String>,
    /// Sticky status toast used for important warnings/errors.
    pub sticky_status: Option<StatusToast>,
    /// Last status text already promoted from `status_message` into toast state.
    pub last_status_message_seen: Option<String>,
    /// Prevents the same pressure condition from immediately re-arming after
    /// the operator explicitly dismisses its sticky warning. Reset when the
    /// pressure falls below the warning threshold or compaction starts.
    pub context_pressure_warning_dismissed: Option<crate::context_budget::PressureLevel>,
    /// Last on-disk plugin catalog stamp we already nudged `/plugin reload` for.
    pub plugin_reload_nudge_stamp: Option<crate::plugins::PluginCatalogStamp>,
    /// Plugin names already toasted for this session's prompt matching.
    pub plugin_prompt_suggest_names: HashSet<String>,
    pub plugin_prompt_suggest_count: u8,
    /// Last idle catalog fingerprint poll, so disk changes can surface between turns.
    pub last_plugin_catalog_poll: Option<Instant>,
    /// Live composer plugin CTA (debounce + one match, never auto-install).
    pub plugin_cta: crate::tui::plugin_suggestions::PluginCtaState,
    pub model: String,
    /// Persisted model selections by provider name. Loaded from settings so
    /// `/model` and the picker can surface saved provider-specific choices.
    pub provider_models: HashMap<String, String>,
    /// Additive provider-scoped model IDs enabled for the ordinary picker.
    /// The catalog remains separately discoverable and selecting from it adds
    /// to this set rather than replacing earlier enabled choices.
    pub enabled_provider_models: HashMap<String, Vec<String>>,
    /// Exact provider/model pins loaded from settings, in user order.
    pub pinned_models: Vec<crate::settings::PinnedModel>,
    /// When true, the model is auto-selected based on request complexity
    /// rather than using a fixed model. The `/model auto` command sets this.
    /// `dispatch_user_message` calls `auto_model_heuristic` to resolve the
    /// effective model for each outbound message.
    pub auto_model: bool,
    /// Last concrete model chosen while `auto_model` is active.
    pub last_effective_model: Option<String>,
    /// Provider that actually served the latest auto-routed turn.
    pub last_effective_provider: Option<ApiProvider>,
    /// Exact non-secret identity for the provider that served the latest Auto
    /// turn. This matters for named custom providers, which all share the
    /// `ApiProvider::Custom` enum variant.
    pub(crate) last_effective_provider_identity: Option<String>,
    /// Auto decision metadata for the most recently resolved Auto turn.
    pub(crate) last_auto_route_receipt: Option<crate::model_routing::AutoRouteReceipt>,
    /// Route selected for the next turn, retained for in-flight UI details
    /// until the engine confirms the authoritative `TurnStarted` route.
    pub pending_turn_route: Option<(ApiProvider, String, bool)>,
    /// Auto decision metadata waiting to be paired with `pending_turn_route`.
    pub(crate) pending_auto_route_receipt: Option<crate::model_routing::AutoRouteReceipt>,
    /// Authoritative lifecycle metadata attached to the most recent
    /// `TurnStarted`. Kept separate from `pending_turn_route` so a preceding
    /// compaction completion cannot consume the next model turn's route.
    pub active_turn: Option<ActiveTurnMetadata>,
    /// Current API provider (mirrors `Config::api_provider`).
    /// Updated by `/provider` switches so the UI/commands can read the
    /// active backend without re-deriving it from the live config.
    pub api_provider: ApiProvider,
    /// Exact configured provider key for persistence and route restoration.
    /// Built-ins use their canonical slug; named custom providers retain the
    /// user-owned key instead of collapsing to `custom`.
    pub(crate) provider_identity: String,
    /// Additive exact configured id for persistence. `None` preserves the
    /// legacy root-level custom route even when a same-key table appears.
    pub(crate) provider_exact_id: Option<String>,
    /// Primary provider plus configured fallback providers for this session.
    pub provider_chain: Option<ProviderChain>,
    /// Per-provider auth/local readiness snapshot for the fallback chain (#2574).
    ///
    /// Captured at startup alongside `provider_chain` (where the live `Config` is
    /// in scope). `advance_fallback` consults it to skip chain entries that
    /// cannot serve a turn — hosted providers missing a key — while local
    /// providers (Ollama/vLLM/SGLang) are always ready. Stored as `(provider,
    /// ready)` pairs; lookups fall back to "ready" for providers not present so
    /// an unknown entry is tried rather than silently skipped.
    provider_readiness: Vec<(ApiProvider, bool)>,
    /// Session-local evidence from real provider requests and verification
    /// probes. Unlike `provider_readiness` above, this never treats a saved key
    /// as proof that the endpoint is healthy.
    pub(crate) provider_health: crate::provider_readiness::ProviderReadinessSnapshot,
    /// Human-readable description of the last provider fallback event.
    pub last_fallback_reason: Option<String>,
    /// True when the active provider/base URL accepts arbitrary model IDs
    /// verbatim rather than DeepSeek-only aliases.
    pub model_ids_passthrough: bool,
    /// Resolved provider/model route limits for the active runtime route.
    pub active_route_limits: Option<RouteLimits>,
    /// Exact resolved endpoint for the active runtime route. This stays
    /// separate from persisted config so endpoint-sensitive compatibility
    /// (notably Kimi Code's bare `k3`) is never inferred from a provider name
    /// alone.
    pub active_route_base_url: String,
    /// Provenance for `active_route_limits`' effective context window. This
    /// is an operator-facing receipt, not a claim about provider billing.
    pub active_context_window_source: crate::route_runtime::ContextWindowSource,
    /// User-configured provider context-window override for the active route.
    pub active_context_window_override: Option<u32>,
    /// Pending provider transition for transactional rollback when the next
    /// auth failure indicates the new provider cannot be used.
    pub pending_provider_switch: Option<PendingProviderSwitch>,
    /// Current live reasoning-effort selection. Route changes may normalize
    /// this value; the raw user choice remains in
    /// [`Self::reasoning_effort_preference`].
    pub reasoning_effort: ReasoningEffort,
    /// Raw explicit user preference, before any fixed provider/model route
    /// normalizes it. `None` means the current live tier is an implicit route
    /// default or compatibility inference and must not constrain Auto routing.
    pub(crate) reasoning_effort_preference: Option<ReasoningEffort>,
    /// Last effective thinking receipt for the most recently accepted route.
    pub(crate) last_effective_reasoning_effort: Option<EffectiveReasoningEffort>,
    pub workspace: PathBuf,
    /// Effective `[workflow]` table for this session (`/workflow settings`).
    pub workflow_config: codewhale_config::WorkflowConfigToml,
    /// Effective `[goal] max_continuations` backstop; `0` means unlimited.
    pub goal_max_continuations: u32,
    /// Typed engine lifecycle state for the cancellable between-turn wait.
    pub goal_continuation_waiting: bool,
    /// Effective explicit/managed filesystem scope captured at startup. The
    /// named permission posture supplies the default when this is `None`.
    pub configured_sandbox_mode: Option<String>,
    /// Configured `sandbox_network_access`. `None`/`Some(false)` keep the
    /// workspace-write sandbox network-restricted.
    pub configured_sandbox_network: Option<bool>,
    /// The sandbox backend this platform+config can actually enforce with,
    /// resolved once at startup. `None` means there is NO enforcement
    /// available (default Linux without `prefer_bwrap`, and all Windows), so
    /// surfaces must not claim the session is sandboxed (2026-08-04 audit).
    pub sandbox_backend: Option<crate::sandbox::SandboxType>,
    /// Off-event-loop worker for durable Lane control writes. `/lane interrupt`
    /// submits here instead of tearing down a Runtime on the composer thread
    /// (#4022).
    pub lane_control: crate::lane_control::LaneControlQueue,
    /// Immutable plugin catalogue scoped to this App's effective workspace.
    pub plugin_registry: std::sync::Arc<crate::plugins::PluginRegistry>,
    pub config_path: Option<PathBuf>,
    pub config_profile: Option<String>,
    /// Legacy executable plugin-tool directory resolved from the already
    /// loaded configuration. Slash-command inventory must not reload the full
    /// config (and thereby re-read credential-bearing fields) merely to find
    /// this path.
    pub legacy_plugin_tools_dir: Option<PathBuf>,
    pub mcp_config_path: PathBuf,
    pub skills_dir: PathBuf,
    pub skills_scan_codewhale_only: bool,
    /// Whether the optional project context pack was enabled when this
    /// session loaded its configuration. Context diagnostics consult this
    /// source of truth even before the first system prompt is assembled.
    pub project_context_pack_enabled: bool,
    /// Path to the user-memory file (#489). Always populated; only
    /// consulted when `use_memory` is `true`.
    pub memory_path: PathBuf,
    /// Whether the user-memory feature is enabled (#489). Mirrors
    /// `Config::memory_enabled()` at app boot. Used by the `# foo`
    /// composer interception,
    /// the `/memory` slash command, and tool registration for
    /// `remember`.
    pub use_memory: bool,
    /// Screen the TUI is painting on right now. `/fullscreen` and `/inline`
    /// move it at runtime; `use_alt_screen()` is derived from it so no second
    /// flag can drift out of step with the live terminal.
    pub screen_mode: ScreenMode,
    /// Mouse capture as programmed on the live terminal. Re-derived from
    /// `mouse_capture_preference` and `screen_mode` on every screen switch.
    pub use_mouse_capture: bool,
    /// See [`TuiOptions::mouse_capture_preference`].
    pub mouse_capture_preference: bool,
    /// When true, plain Up/Down on an empty composer scroll the transcript
    /// instead of navigating input history.  Defaults to `true` when mouse
    /// capture is off: terminals that convert mouse-wheel events to arrow-key
    /// sequences (e.g. Windows CMD without `WT_SESSION`) get page-scrolling
    /// without any explicit config (#1443).
    pub composer_arrows_scroll: bool,
    /// Whether `composer_arrows_scroll` came from explicit configuration.
    pub composer_arrows_scroll_explicit: bool,
    /// Data-side cap for the `@`-mention popup. The renderer still limits the
    /// visible rows to available terminal height.
    pub mention_menu_limit: usize,
    /// Maximum workspace depth for `@`-mention completion walks. `0` means
    /// unlimited depth.
    pub mention_walk_depth: usize,
    /// `@`-mention completion behavior: fuzzy workspace search or deterministic
    /// directory browser.
    pub mention_menu_behavior: String,
    /// Follow symbolic links during workspace file discovery walks.
    /// When `true`, symlinked directories are traversed, enabling
    /// multi-project workspaces.
    pub workspace_follow_symlinks: bool,
    pub use_bracketed_paste: bool,
    pub use_paste_burst_detection: bool,
    /// Set to `true` the first time a real `Event::Paste` arrives during a
    /// session. Once set, `handle_paste_burst_key` short-circuits — there's
    /// no point running the rapid-keypress heuristic on a terminal that
    /// already delivers paste-as-event correctly. Avoids paste-burst false
    /// positives on Ghostty / iTerm2 / WezTerm / Windows Terminal where
    /// fast typing or IME commits could otherwise be mis-classified as a
    /// paste burst (#1322 follow-up).
    pub bracketed_paste_seen: bool,
    #[allow(dead_code)]
    pub system_prompt: Option<SystemPrompt>,
    pub auto_compact: bool,
    pub auto_compact_user_configured: bool,
    pub auto_compact_threshold_percent: f64,
    pub stopped_turn: bool,
    pub calm_mode: bool,
    pub low_motion: bool,
    pub constrained_frame_rate: bool,
    /// The ambient animation clock, in clamped milliseconds. Creature and
    /// water positions are pure functions of this value; advancing it by at
    /// most [`App::AMBIENT_MAX_STEP_MS`] per sampled frame keeps motion
    /// continuous when draws arrive in bursts (fast token streams previously
    /// sampled raw wall-clock time at irregular gaps, so fish "teleported"
    /// between frames — captains-log #16).
    pub ambient_clock_ms: u128,
    /// When the ambient clock last advanced; `None` until the first sample.
    pub ambient_clock_sampled_at: Option<Instant>,
    /// When the shell last became fully idle (no turn, no live sub-agents,
    /// no active durable tasks, completion exhale finished). After a short
    /// grace of gentle motion the aquarium settles to a genuinely still
    /// scene instead of repainting an idle screen forever.
    pub ambient_idle_since: Option<Instant>,
    /// Start of the underwater shell's one-shot successful-turn exhale.
    /// Kept separate from the ambient ocean clock so completion can settle
    /// once without restarting or repainting the transcript field.
    pub ocean_completion_started_at: Option<Instant>,
    /// History length at the current turn boundary. Successful completion
    /// uses this stable index to settle only the receipts produced by that
    /// turn, never old transcript rows.
    pub ocean_turn_history_start: usize,
    /// First committed history cell participating in the current one-shot
    /// receipt-settle cascade.
    pub ocean_receipt_settle_start: Option<usize>,
    /// Enables the authored underwater phase and ambient motion system.
    pub fancy_animations: bool,
    /// Typed appearance treatment; appearance is independent from motion
    /// settings, and every underwater treatment keeps ambient life.
    /// Focus-context texture prototype mode (#4823), parsed once from the
    /// `focus_texture` setting. `Off` by default; while off the modal render
    /// path is byte-identical to the pre-prototype path.
    pub focus_texture: crate::tui::focus_texture::FocusTextureMode,
    /// Distinct pre-session menu. Once dismissed, the normal idle ocean owns
    /// the empty session and this state stays hidden.
    pub launch: LaunchState,
    /// Mouse-selected launch action, consumed by the async UI loop.
    pub pending_launch_action: Option<crate::tui::underwater::LaunchAction>,
    /// Mouse click on the live composer's `[↑]` send target. The async UI loop
    /// consumes it through the same submit dispatcher as Enter.
    pub pending_composer_submit: Option<ComposerSubmitChord>,
    /// Mouse-selected hotbar slot, consumed by the async UI loop.
    pub pending_hotbar_slot: Option<u8>,
    /// Whether the renderer should wrap each frame in DEC mode 2026
    /// synchronized output. Resolved from `Settings::synchronized_output`
    /// at construction; `auto`/`on` → `true`, `off` → `false`. The Ptyxis
    /// auto-detect path in `Settings::apply_env_overrides` flips `auto`
    /// to `off` before App is built, so by the time we read this flag in
    /// the draw loop the decision is already made. See the
    /// `Settings::synchronized_output` doc for the user-facing knob.
    pub synchronized_output_enabled: bool,
    /// Header status-indicator chip mode. `"cw"` is the static default;
    /// `"whale"` and `"dots"` preserve the animated legacy choices, while
    /// `"off"` hides the chip. Loaded from settings and changed via
    /// `/config status_indicator <cw|whale|dots|off>`.
    pub status_indicator: String,
    pub show_thinking: bool,
    pub thinking_highlight: bool,
    pub thinking_default_expanded: bool,
    pub thinking_preview_lines: usize,
    pub help_expand_groups: bool,
    pub pin_last_prompt: bool,
    pub verbose_transcript: bool,
    pub show_tool_details: bool,
    /// Inline presentation mode for successful structured File mutations.
    /// Exact evidence remains attached to each mutation receipt in all modes.
    pub inline_diff_mode: InlineDiffMode,
    pub ui_locale: Locale,
    pub cost_currency: CostCurrency,
    /// Route payment truth. Model pricing alone cannot distinguish metered
    /// API calls from OAuth or token-plan quota.
    pub billing_presentation: crate::route_billing::BillingPresentation,
    pub composer_density: ComposerDensity,
    pub composer_border: bool,
    pub composer_multiline_mode: bool,
    /// Voice input state — toggled by `/voice` and the voice hotbar action.
    pub voice_enabled: bool,
    /// Auto-send after transcription when the transcript ends with an
    /// explicit send instruction ("send it" / "发送"). Toggled by `/voice-send`.
    pub voice_send_enabled: bool,
    /// AI-assisted dictation that sees the current composer text.
    /// Toggled by `/voice-control`.
    pub voice_control_enabled: bool,
    pub transcript_spacing: TranscriptSpacing,
    /// Prose wrap cap from `[transcript] prose_measure` (#5436). `None`
    /// means prose uses the full content width, like tool/status cells.
    pub(crate) prose_measure: Option<u16>,
    /// Sidebar hover state for mouse tooltip support.
    pub sidebar_hover: SidebarHoverState,
    /// Current hover tooltip text, if any.
    pub sidebar_hover_tooltip: Option<String>,
    /// Last successfully rendered Work panel summary. Transient mutex misses
    /// Browsing context from the last dismissed `/model` picker, so reopening
    /// restores the view mode and highlighted row instead of resetting to the
    /// top (#4109 picker memory). Session-scoped, never persisted.
    pub model_picker_memory: Option<ModelPickerMemory>,
    /// Browsing context from the last dismissed `/provider` picker.
    pub provider_picker_memory: Option<ProviderPickerMemory>,
    /// Last known mouse position for tooltip placement.
    pub last_mouse_pos: Option<(u16, u16)>,
    /// Whether the session-context panel is enabled (#504).
    pub context_panel: bool,
    /// Whether the persistent Sessions rail is enabled (#2934). Opt-in.
    pub sessions_rail: bool,
    /// Minimum number of consecutive safe tool cells needed for auto-collapse.
    ///
    /// Fixed at 3 for v0.9.x (#3256 decision): not a user setting. Rollups need
    /// enough cells to be readable; exposing a knob without UX for partial
    /// runs would just recreate the pre-collapse noise floor.
    pub tool_collapse_threshold: usize,
    /// Tool runs the user explicitly expanded. Stores original history indices.
    pub expanded_tool_runs: HashSet<usize>,
    /// Current dense tool-run collapse behavior.
    pub tool_collapse_mode: ToolCollapseMode,
    /// File-tree pane state. `None` when hidden; `Some` when visible.
    pub file_tree: Option<crate::tui::file_tree::FileTreeState>,
    /// Whether the file-tree pane was actually rendered in the last frame.
    /// Set false when the terminal is too narrow to show the tree.
    pub file_tree_visible: bool,
    #[allow(dead_code)]
    pub compact_threshold: usize,
    pub max_input_history: usize,
    pub allow_shell: bool,
    pub verbosity: Option<String>,
    pub max_subagents: usize,
    /// Per-SSE-chunk idle timeout for streamed turns, in seconds.
    pub stream_chunk_timeout_secs: u64,
    /// Cached sub-agent snapshots for UI views.
    pub subagent_cache: Vec<SubAgentResult>,
    /// First time this TUI observed each terminal sub-agent card.
    pub subagent_terminal_seen_at: HashMap<String, Instant>,
    /// Last known per-agent progress text for running sub-agents.
    pub agent_progress: HashMap<String, String>,
    /// Parent/depth metadata for live progress-only sub-agent rows.
    pub agent_progress_meta: HashMap<String, AgentProgressMeta>,
    /// In-transcript sub-agent card index by `agent_id` (issue #128).
    /// Maps each live sub-agent to the `HistoryCell::SubAgent` it renders
    /// into, so successive mailbox envelopes mutate the same cell rather
    /// than spawning duplicates.
    pub subagent_card_index: HashMap<String, usize>,
    /// History index of the most recent FanoutCard. Sibling sub-agents
    /// spawned by the same `rlm` invocation route into this card; reset
    /// when a fresh fanout-family tool call starts.
    pub last_fanout_card_index: Option<usize>,
    /// Most recently observed sub-agent dispatch tool name (set on
    /// `ToolCallStarted` for `agent` / `rlm` / etc., cleared
    /// after the first `Started` mailbox envelope routes through it).
    pub pending_subagent_dispatch: Option<String>,
    /// Animation anchor for status-strip active sub-agent spinner.
    pub agent_activity_started_at: Option<Instant>,
    /// Monotonic counter for stable agent labels (#3030).
    /// Incremented each time a sub-agent is spawned; used to generate
    /// "Agent 1", "Agent 2", etc.
    pub agent_counter: u64,
    /// Maps raw agent_id to a stable user-facing label (#3030).
    /// Populated when `AgentSpawned` fires; read by sidebar rendering.
    pub agent_label_map: HashMap<String, String>,
    /// The child whose full transcript currently owns the main conversation
    /// area and whose fork the composer addresses (`None` = main session).
    pub agent_focus: Option<crate::tui::agent_focus::AgentFocus>,
    /// Follow-ups a running child has not yet taken at its next round
    /// boundary (`agent_id` → count), from the latest `AgentList` refresh.
    pub agent_queued_follow_ups: HashMap<String, usize>,
    /// Receipts-only roster of every agent that ran this session (#5479).
    /// Refreshed wholesale on each `AgentList` event; rendered by `/agents`
    /// and, in a later slice, by the agents rail.
    pub agent_roster: Vec<crate::tui::agent_roster::AgentRosterRow>,
    /// `/agents list` asked for a one-shot transcript listing. Cleared by the
    /// `AgentList` handler that prints it.
    pub agent_roster_print_requested: bool,
    /// Per-role sequence counters for unnamed children (#3030). Two concurrent
    /// builders render as `builder · 1` and `builder · 2` instead of sharing a
    /// bare, indistinguishable role label.
    pub agent_role_counters: HashMap<String, u64>,
    /// Last time a sub-agent progress event triggered a redraw.
    /// Used to throttle redraws under high sub-agent concurrency (#3033).
    pub last_agent_progress_redraw: Option<Instant>,
    /// Last time a workflow `budget_updated` event was allowed to request a
    /// repaint. High-signal workflow events (task/run lifecycle) always paint;
    /// budget-only chatter is paced under fan-out (#4095 residual).
    pub last_workflow_budget_redraw: Option<Instant>,
    pub ui_theme: UiTheme,
    /// Parsed `background_color` setting, kept separately from `ui_theme` so
    /// an explicit override remains distinguishable even when it happens to
    /// equal the current named theme's default surface and can still carry
    /// into previews of other themes.
    pub background_color_override: Option<Color>,
    /// Active named theme. Drives the cell-level color remap in
    /// `tui::color_compat::ColorCompatBackend` so community presets
    /// (Catppuccin, Tokyo Night, Dracula, Gruvbox) propagate to every
    /// render site, not just the handful that read `app.ui_theme`.
    pub theme_id: palette::ThemeId,
    // Onboarding
    pub onboarding: OnboardingState,
    pub onboarding_needs_api_key: bool,
    pub onboarding_provider: ApiProvider,
    pub onboarding_workspace_trust_gate: bool,
    /// True when onboarding opened only because a returning user's configured
    /// provider is missing its key. Esc then exits to the offline composer
    /// instead of walking back through first-run steps.
    pub onboarding_missing_key_recovery: bool,
    /// True when the user explicitly chose "Explore offline" during onboarding
    /// (#3927). No provider was selected, no route was activated, and no secret
    /// was saved: the session browses with queued input until a route is
    /// activated later (`/provider`), which is the only thing that clears it.
    pub onboarding_explore_offline: bool,
    /// First-run route receipts: which required decisions this run contains.
    /// The surface title counts only these ("1 of 2"), never a fixed spine.
    pub onboarding_had_language_step: bool,
    pub onboarding_had_provider_step: bool,
    pub onboarding_had_trust_step: bool,
    /// True when the active credential was discovered only through an
    /// environment variable. Missing-key recovery and route rollback use this
    /// provenance to decide whether a durable provider slot still exists;
    /// credential drafts live exclusively inside `ProviderPickerView`.
    pub api_key_env_only: bool,
    // Hooks system
    pub hooks: HookExecutor,
    /// Lifecycle event outbox (`[lifecycle_outbox]` config). Disabled
    /// (all emits no-ops) when no path is configured.
    pub lifecycle_outbox: codewhale_hooks::LifecycleOutbox,
    #[allow(dead_code)]
    pub yolo: bool,
    /// One-shot YOLO→Act+Bypass migration notice for this session (#0.8.68 M6).
    yolo_compat_notified: bool,
    /// The single serialized owner of `settings.toml` startup-default writes
    /// (mode, thinking, model). Keeping one owner per `App` is what stops two
    /// rapid selections from interleaving their load/modify/save transactions
    /// and losing the newer one. Failures are drained by the event loop into a
    /// warning toast, so a settings write that did not land is never silently
    /// reverted on the next launch.
    pub startup_defaults: crate::tui::startup_defaults::StartupDefaultsWriter,
    /// One-shot Shift+Tab/Ctrl+T rebinding notice for this session (#0.8.68 M3).
    keybinding_migration_notified: bool,
    /// Durable Agent-era permission baseline that Plan/YOLO derive from and
    /// restore to (#3386). Refreshed from the live fields whenever the user
    /// leaves Agent mode; see [`base_policy_for_mode`] and `set_mode`.
    mode_prefs: ModeSessionPrefs,
    /// True when config/requirements supplied an approval policy. In that
    /// case the TUI-only Shift+Tab preference must not loosen it.
    approval_policy_locked: bool,
    /// True only when the controlling policy is the user's editable root
    /// config.toml key. An explicit Shift+Tab may migrate that key to the
    /// durable TUI posture; higher-precedence sources remain immutable.
    approval_policy_root_editable: bool,
    /// True only when an organization requirements file owns approval policy.
    /// Unlike a user-owned config key, this source cannot be edited in-app.
    approval_policy_requirements_managed: bool,
    /// True when the interactive shell switch is user-owned (unset or root
    /// config.toml). Profile / env / managed / project owners stay in charge
    /// even when a YOLO entry point asks for Full Access.
    shell_access_editable: bool,
    // Clipboard handler
    pub clipboard: ClipboardHandler,
    // Tool approval session allowlist
    pub approval_session_approved: HashSet<String>,
    /// Approval keys (or tool names) the user has denied or aborted in
    /// this session. Subsequent re-requests for the same approval key
    /// auto-deny without re-prompting (#360) — the model can retry a
    /// dangerous command after being told no, but the user shouldn't
    /// have to keep dismissing the same dialog.
    pub approval_session_denied: HashSet<String>,
    pub approval_mode: ApprovalMode,
    // Modal view stack (approval/help/etc.)
    pub view_stack: ViewStack,
    /// Last `request_user_input` prompt, retained so a failed modal submit can reopen (#1198).
    pub pending_user_input_prompt: Option<(String, crate::tools::user_input::UserInputRequest)>,
    /// Esc-Esc backtrack state machine (#133). `Inactive` by default; first
    /// Esc primes, second Esc opens the live-transcript overlay scoped to
    /// previous user messages so the user can rewind a turn.
    pub backtrack: crate::tui::backtrack::BacktrackState,
    /// Current session ID for auto-save updates
    pub current_session_id: Option<String>,
    /// Last non-contended Work snapshot captured in this App. The outer
    /// option distinguishes "never captured" from a captured empty state.
    pub(crate) last_known_work_state: Option<Option<SessionWorkState>>,
    /// Latest bounded runtime goal projection. Persistence stores this beside
    /// the owning saved session so a resumed process rebuilds the same goal
    /// control state instead of inferring it from transcript prose.
    pub(crate) last_known_goal_state: Option<crate::session_manager::SessionGoalState>,
    /// FIFO of accepted typed controls not yet reconciled by GoalUpdated.
    /// The durable desired state lives in `last_known_goal_state`; this queue
    /// preserves in-process ordering and mailbox retry state only.
    pub(crate) pending_goal_controls: VecDeque<PendingGoalControl>,
    /// Metadata for the active session, cached in memory so automatic
    /// checkpoints never synchronously reload and parse a growing JSON file on
    /// the UI thread.
    pub(crate) current_session_metadata: Option<SessionMetadata>,
    /// Metadata-only registry of large tool outputs produced in this session.
    pub session_artifacts: Vec<ArtifactRecord>,
    /// Trust mode - allow access outside workspace
    pub trust_mode: bool,
    /// Translation mode — when enabled, the model is instructed to respond in
    /// the current locale and a post-hoc translation layer replaces any
    /// remaining English output before it reaches the user.
    pub translation_enabled: bool,
    /// Mini-window (pinned always-on-top) layout preferences, sourced from
    /// `[mini_window]` in config.toml at startup and mutated live by
    /// `/config mini_window.keep_*`. The renderer reads this instead of the
    /// parsed Config so runtime changes apply without a restart.
    pub(crate) mini_window: crate::config::MiniWindowConfig,
    /// Ordered list of footer items the user wants visible. Sourced from
    /// `tui.status_items` in `~/.deepseek/config.toml` at startup; mutated
    /// live by `/statusline`. The renderer iterates this slice; no item is
    /// hardcoded in the footer code path.
    pub status_items: Vec<crate::config::StatusItem>,
    /// Optional header items enabled from `tui.header_items` in `config.toml`
    /// at startup. Built-in header content remains independent of this list.
    /// Unread since the classic header was superseded by the Tideline info
    /// line (2026-08-29): the info line carries the context meter by default and the
    /// token breakdown lives behind `/cost` (spec §3). The field stays so the
    /// config surface keeps parsing; its reader returns with the classic
    /// renderer deletion slice.
    #[allow(dead_code)]
    pub header_items: Vec<crate::config::HeaderItem>,
    /// Project documentation (AGENTS.md or CLAUDE.md)
    #[allow(dead_code)]
    pub project_doc: Option<String>,
    /// Plan state for tracking tasks
    pub plan_state: SharedPlanState,
    /// Todo list for the canonical `work_update` progress surface.
    pub todos: SharedTodoList,
    /// Durable runtime services exposed to model-visible task/automation tools.
    pub runtime_services: RuntimeToolServices,
    /// Latest bounded coordination receipt delivered by the engine. This is
    /// the same typed projection returned to headless inspection; the TUI does
    /// not parse tool text to reconstruct it.
    pub coordination_detail: Option<crate::tools::subagent::CoordinationDetailProjection>,
    /// Last MCP manager/discovery snapshot shown in the UI.
    pub mcp_snapshot: Option<crate::mcp::McpManagerSnapshot>,
    /// True while the engine-owned MCP boot connection pass is in flight.
    /// Configured rows render as connecting until its snapshot lands.
    pub mcp_initializing: bool,
    /// Latest engine-owned MCP event generation applied to the UI.
    pub mcp_snapshot_generation: u64,
    /// The direct snapshot from a successful `/mcp` action supersedes any
    /// queued event at `mcp_snapshot_generation`, but not a later generation.
    pub mcp_snapshot_generation_invalidated: bool,
    /// Enabled servers that have not settled in the current boot pass.
    pub mcp_connecting: Vec<String>,
    /// Number of MCP servers declared in the user's config at app boot.
    /// Used by the footer chip (#502) so a count is visible even before
    /// the user runs `/mcp` for the first time. `0` hides the chip.
    pub mcp_configured_count: usize,
    /// Set after in-TUI MCP config edits because the engine caches its MCP pool.
    pub mcp_reload_required: bool,
    /// Tool execution log
    pub tool_log: Vec<String>,
    /// Active skill to apply to next user message
    pub active_skill: Option<String>,
    /// Content-bound plugin authority carried with `active_skill`, when the
    /// selected skill came from a reviewed plugin bundle.
    pub active_skill_provenance: Option<crate::plugins::types::PluginAuthority>,
    /// Cached (name, description) pairs from the skill registry.
    /// Populated once at startup and refreshed on install/uninstall so
    /// the slash menu can show skills without filesystem I/O on every keystroke.
    pub cached_skills: Vec<(String, String)>,
    /// Tool call cells by tool id (for cells already finalized in `history`).
    /// While a tool call is in flight inside `active_cell`, it is tracked by
    /// `active_tool_entries` instead and migrated here at flush time.
    pub tool_cells: HashMap<String, usize>,
    /// Full tool input/output keyed by history cell index.
    pub tool_details_by_cell: HashMap<usize, ToolDetailRecord>,
    /// Linked context references keyed by the visible user history cell that
    /// introduced them.
    pub context_references_by_cell: HashMap<usize, Vec<SessionContextReference>>,
    /// Session-wide context references persisted with saved sessions.
    pub session_context_references: Vec<SessionContextReference>,
    /// In-flight tool/exec group for the current turn. Mutated in place as
    /// parallel tool calls start and complete; flushed into `history` on
    /// `TurnComplete`.
    pub active_cell: Option<ActiveCell>,
    /// Revision counter for `active_cell`. Combined with `active_cell.revision`
    /// when feeding the transcript cache so cached lines for the synthetic
    /// active-cell row are invalidated on every mutation.
    pub active_cell_revision: u64,
    /// Pending tool details for entries that live inside `active_cell`.
    /// Keyed by tool id rather than cell index because the active cell's
    /// virtual index can shift (orphan completions push real cells in
    /// between). Migrated into `tool_details_by_cell` on flush.
    pub active_tool_details: HashMap<String, ToolDetailRecord>,
    /// Completion timestamps for entries still living inside `active_cell`.
    /// The transcript keeps completed entries until turn flush, but the
    /// sidebar can use these timestamps to let settled live rows expire.
    pub active_tool_entry_completed_at: HashMap<usize, Instant>,
    /// Active exploring cell entry index (within `active_cell.entries`).
    /// `None` once the active cell flushes or no exploring entry exists.
    pub exploring_cell: Option<usize>,
    /// Mapping of exploring tool ids to `(entry index in active_cell, entry
    /// within ExploringCell)`. Used to update individual exploring entries
    /// when their tools complete.
    pub exploring_entries: HashMap<String, (usize, usize)>,
    /// Tool calls that should be ignored by the UI
    pub ignored_tool_calls: HashSet<String>,
    /// Last exec wait command shown (for duplicate suppression)
    pub last_exec_wait_command: Option<String>,
    /// Current streaming assistant cell
    pub streaming_message_index: Option<usize>,
    /// Provenance for append-only changes to the current streaming cell.
    /// Revisions are raw `history_revisions`; the widget maps them through its
    /// cache-key transform before handing the receipt to the transcript cache.
    pub(crate) streaming_source_receipt: Option<crate::tui::transcript::StreamingSourceReceipt>,
    /// True after a local cancel key has been handled and before the engine's
    /// authoritative TurnComplete arrives. Stream events already queued for
    /// the cancelled turn are ignored so text does not keep appearing after
    /// Ctrl+C/Esc returns focus to the composer.
    pub suppress_stream_events_until_turn_complete: bool,
    /// Index into `active_cell.entries` of the thinking entry currently being
    /// streamed. `None` when no thinking block is in flight. P2.3 routes
    /// thinking into the active cell so it groups visually with tool calls
    /// until the next assistant prose chunk flushes the group into history.
    pub streaming_thinking_active_entry: Option<usize>,
    /// Instant of the last throttled active-cell revision bump for the
    /// in-flight thinking stream (#1620). Reasoning chunks arrive faster than
    /// the eye can read, and each bump invalidates the active cell's wrap
    /// cache, forcing a full re-wrap. We debounce intermediate bumps to a
    /// time window so high-frequency thinking deltas no longer trigger a
    /// re-render per character. `None` means "no bump since the last
    /// finalize" so the first chunk of a block always renders immediately.
    pub thinking_revision_last_bump_at: Option<Instant>,
    /// Newline-gated streaming collector state.
    pub streaming_state: StreamingState,
    /// Live approximate output tokens for the current assistant stream.
    pub streaming_output_token_estimate: u64,
    /// Provider-billed prompt tokens from the most recent parent model call
    /// (per-step `TurnUsage`). The context meter takes the max of this and
    /// the local estimate — the same rule the auto-compaction trigger uses —
    /// so the two can never disagree about pressure (#5577). Cleared when
    /// compaction rewrites history, since the receipt describes the
    /// pre-compaction context.
    pub last_billed_input_tokens: Option<u32>,
    /// Last successful compaction, so `/context` and the inspector can name
    /// the path and the last-round floor instead of looking empty.
    pub last_compaction: Option<crate::compaction::LastCompactionSnapshot>,
    /// Accumulated reasoning text
    pub reasoning_buffer: String,
    /// Live reasoning header extracted from bold text
    pub reasoning_header: Option<String>,
    /// Last completed reasoning block
    pub last_reasoning: Option<String>,
    /// Tool calls captured for the pending assistant message
    pub pending_tool_uses: Vec<(String, String, Value)>,
    /// One-line permission receipts (`tool_id`, text) for decisions nobody
    /// was prompted for, held until that tool's card completes so the note
    /// lands directly under the card instead of splitting a running tool run.
    pub pending_gate_receipts: Vec<(String, String)>,
    /// Permission receipts for child (sub-agent) tool calls, keyed by agent
    /// id then `(tool_id, text)`, rendered under the matching tool card when
    /// that child is focused. Session-resident only.
    pub child_gate_receipts: std::collections::HashMap<String, Vec<(String, String)>>,
    /// User messages queued while a turn is running
    pub queued_messages: VecDeque<QueuedMessage>,
    /// Draft queued message being edited
    pub queued_draft: Option<QueuedMessage>,
    /// Legacy pending-steer bucket retained for session compatibility. New
    /// in-flight input uses Ctrl+Enter for same-turn steering and Enter for
    /// queued follow-ups; Esc only cancels the active turn.
    pub pending_steers: VecDeque<QueuedMessage>,
    /// Engine-rejected steers (e.g. a tool was already running and couldn't be
    /// cancelled cleanly). Surfaced in the pending-input preview so the user
    /// knows the steer was deferred to end-of-turn. Today no engine path
    /// produces these; the field is scaffolding for a future signalling
    /// channel and the bucket renders with a rejected-steer label when
    /// populated.
    pub rejected_steers: VecDeque<String>,
    /// Legacy resend flag for pending steer recovery.
    pub submit_pending_steers_after_interrupt: bool,
    /// Start time for current turn
    pub turn_started_at: Option<Instant>,
    /// Most recent engine event observed for the current turn. This is
    /// separate from `turn_started_at` because the latter drives elapsed-time
    /// UI and must not be reset during long but healthy turns.
    pub turn_last_activity_at: Option<Instant>,
    /// Sum of completed turn durations for this `App` instance (#448
    /// follow-up). Drives the footer's `worked Nh Mm` chip so the
    /// label reflects actual model work, not wall-clock since launch.
    /// Incremented on `TurnComplete` from the elapsed time of the
    /// just-finished turn. Resets per launch.
    pub cumulative_turn_duration: std::time::Duration,
    /// Session metrics strip accumulators (model-call/tool timings, TTFT,
    /// throughput). Sourced only from engine events; see
    /// [`crate::tui::session_metrics`].
    pub session_metrics: crate::tui::session_metrics::SessionMetrics,
    /// DeepSeek account balance, refreshed once per turn completion.
    /// Shared cell updated by background fetch tasks; read lock in the UI thread.
    pub balance_cell: std::sync::Arc<std::sync::Mutex<Option<crate::pricing::BalanceInfo>>>,
    /// Shared cell for async fleet-profile model-draft delivery. A background
    /// task fills it (model label + drafted profile or a failure reason) so
    /// the drafting network call never parks the event loop (#3757 review).
    #[allow(clippy::type_complexity)]
    /// Monotonic generation for model-draft requests. Bumped on each draft
    /// request and each setup/fleet wizard open, so a draft that lands after
    /// a superseding request or a wizard reopen is dropped rather than
    /// installed into the wrong (or a stale) wizard instance.
    pub draft_gen: std::sync::Arc<std::sync::atomic::AtomicU64>,
    #[allow(clippy::type_complexity)]
    pub fleet_draft_cell: std::sync::Arc<
        std::sync::Mutex<
            Option<(
                u64,
                String,
                // The `(provider, model)` route the operator picked when they
                // pressed `m` (#4093). Carried alongside the async draft so the
                // ratified profile keeps the picked cross-provider route even if
                // the model draft (which is always `provider: None`) omitted or
                // changed it. `None` for an `inherit` pick.
                Option<(String, String)>,
                // The reasoning tier selected when the operator pressed `m`
                // (#4137). `None` means inherit.
                Option<String>,
                Result<Box<crate::fleet::profile::FleetProfileDraft>, String>,
            )>,
        >,
    >,
    /// Shared cell for async constitution model-draft delivery (same pattern
    /// as `fleet_draft_cell`, so the drafting network call never parks the
    /// event loop).
    #[allow(clippy::type_complexity)]
    pub constitution_draft_cell: std::sync::Arc<
        std::sync::Mutex<
            Option<(
                u64,
                String,
                crate::localization::Locale,
                Result<Box<codewhale_config::UserConstitution>, String>,
            )>,
        >,
    >,
    /// Shared cell for async prompt suggestion delivery from background task.
    pub prompt_suggestion_cell: std::sync::Arc<std::sync::Mutex<Option<(u64, String)>>>,
    /// Tracks whether the initial balance fetch has been attempted for this session.
    pub balance_initiated: bool,
    /// Timestamp of the last balance fetch, used to debounce rapid requests.
    pub last_balance_fetch: Option<std::time::Instant>,
    /// Current runtime turn id (if known).
    pub runtime_turn_id: Option<String>,
    /// Current runtime turn status (if known).
    pub runtime_turn_status: Option<String>,
    /// Monotonic turn counter for stable user-facing labels (#3030).
    /// Incremented each time a new turn starts; displayed as "Turn N".
    pub turn_counter: u64,
    /// When the UI accepted a user message but has not observed `TurnStarted` yet.
    pub dispatch_started_at: Option<Instant>,

    /// Cached git context snapshot for the footer.
    pub workspace_context: Option<String>,
    /// Shared cell for async git context updates (#399 S1).
    pub workspace_context_cell: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    /// Timestamp for cached workspace context.
    pub workspace_context_refreshed_at: Option<Instant>,
    /// Cached size of the memory file, formatted for the Session sidebar.
    ///
    /// Rendered every frame the Session/Context panel is visible, so the
    /// `stat` behind it is refreshed on the workspace-context TTL tick
    /// instead of inside the draw closure (#3908) — tens of ms per frame on
    /// NFS/SSHFS/cloud-synced homes otherwise.
    pub memory_size_hint: Option<String>,
    /// Cached background tasks for sidebar rendering.
    pub task_panel: Vec<TaskPanelEntry>,
    /// Live scheduled-work projection for the activity band
    /// (AUTOMATION-VISIBILITY-SPEC §2.1), refreshed on the task-panel cadence
    /// by `refresh_automation_panel`. The band reads it;
    /// `background_indicator` never learns about automations.
    pub automation_panel: crate::tui::automation_panel::AutomationPanelState,
    /// The automation store scan in flight for `automation_panel`, if any.
    /// The scan reads every definition and run file, so it runs on a
    /// blocking thread and the tick folds it once it has finished — the
    /// async UI loop never parks behind the automation store's disk.
    pub automation_scan:
        Option<tokio::task::JoinHandle<crate::tui::automation_panel::AutomationScan>>,
    /// Session-local quieting and command detectors for event-driven tips.
    pub behavioral_tips: crate::tui::behavioral_tips::BehavioralTipState,
    /// Unified Workflow activity surface (#4121). Lives above the composer so
    /// phase/row progress does not flood the chat transcript. Preserved after
    /// completion until the next `RunStarted` replaces it.
    pub workflow_panel: Option<crate::tui::widgets::workflow_panel::WorkflowPanel>,
    /// Wall-clock time when this TUI session started. Used by the Work
    /// sidebar projection to hide completed durable tasks that finished
    /// before the current session (bug #1913).
    pub session_started_at: chrono::DateTime<chrono::Utc>,
    /// Whether the UI needs to be redrawn.
    pub needs_redraw: bool,
    /// A fleet mutation (`/fleet add|remove`, ⇧F, auto-enroll) landed on
    /// disk since the engine last received its roster. The event loop
    /// flushes it through `Op::SetFleetRoster` (`sync_fleet_roster`).
    pub fleet_roster_stale: bool,
    /// When true, the next draw will be a full repaint (terminal clear +
    /// all cells redrawn) instead of a ratatui incremental diff. Used by
    /// theme switches where the diff engine may miss color-only changes
    /// in sidebar cells that were previously rendered with palette constants.
    pub force_next_full_repaint: bool,
    /// When the current thinking block started (for duration tracking).
    pub thinking_started_at: Option<Instant>,
    /// Whether context compaction is currently in progress.
    pub is_compacting: bool,
    /// Typed identity retained from CompactionStarted until its matching
    /// CompactionCompleted/CompactionFailed event.
    pub(crate) active_compaction: Option<ActiveCompaction>,
    /// A manual compaction op accepted by the UI but waiting for the engine to
    /// finish the active turn and emit CompactionStarted.
    pub(crate) manual_compaction_queued: bool,
    /// Stable identity allocated before the manual request enters the engine
    /// mailbox, retained so Ctrl+C/Esc can cancel that exact queued pass.
    pub(crate) manual_compaction_id: Option<String>,
    /// A manual compaction request that found the bounded engine mailbox full.
    /// The event loop retries the send once a slot frees; any compaction that
    /// starts or settles in the meantime supersedes it. Inner value is the
    /// requested focus.
    pub(crate) deferred_manual_compaction: Option<Option<String>>,
    /// Whether context purge is currently in progress.
    pub is_purging: bool,
    /// Set when the user scrolls up/down during a streaming turn so subsequent
    /// streamed chunks don't yank the view back to the live tail. Cleared
    /// when the user explicitly returns to bottom or the turn completes.
    pub user_scrolled_during_stream: bool,
    /// Timestamp of the last user message send (for brief visual feedback).
    pub last_send_at: Option<Instant>,
    /// Most recent user prompt accepted for an active engine turn. Ctrl+C can
    /// restore this into an empty composer after cancelling that turn.
    pub last_submitted_prompt: Option<String>,
    /// Startup prompt should be submitted automatically after the engine is ready.
    pub auto_submit_initial_input: bool,
    /// Two-tap quit confirmation. When set, a prior Ctrl+C in idle state has
    /// armed the quit shortcut; a second Ctrl+C before this `Instant` exits
    /// the app, while expiry silently re-arms the prompt for next time.
    /// Stays `None` while a turn is in flight or a modal/picker is open so
    /// Ctrl+C keeps its current "interrupt this turn" semantics in those
    /// states. See [`App::arm_quit`] / [`App::quit_is_armed`].
    pub quit_armed_until: Option<Instant>,

    // === Prefix-Cache Stability Tracking ===
    /// Number of times the prefix (system prompt + tool specs) has changed.
    pub prefix_change_count: u64,
    /// Total number of prefix stability checks performed.
    pub prefix_checks_total: u64,
    /// Current prefix stability percentage, if known.
    pub prefix_stability_pct: Option<u32>,
    /// Description of the last prefix change, if any.
    pub last_prefix_change_desc: Option<String>,
    /// Current pinned prefix combined hash (SHA-256, 64 hex chars).
    /// Updated per-turn via PrefixCacheChange events; surfaced by
    /// `/cache stats` for cache-hit debugging.
    pub last_pinned_prefix_hash: Option<String>,
    /// Why the current KV-cache prefix pin exists (`initial`/`resume`/`change:*`).
    pub prefix_pin_reason: Option<String>,
    /// Explanation of the most recent expected cache miss.
    pub prefix_last_miss_reason: Option<String>,
    /// Undeclared prefix drifts this session (should stay 0 after the fix).
    pub prefix_drift_count: u64,
    /// `<context_update>` snapshots appended this session.
    pub prefix_context_updates: u64,

    // === Transcript filtering (#397) ===
    /// Transcript cells the user has collapsed (hidden from view).
    /// Stores **original** virtual cell indices (pre-filtering).
    pub collapsed_cells: HashSet<usize>,
    /// Thinking cells the user has folded (showing summary instead of full
    /// content). Stores **original** virtual cell indices. Toggled by Space
    /// when the composer is empty and the cursor is on a thinking cell.
    pub folded_thinking: HashSet<usize>,
    /// Mapping from filtered cell index → original virtual index.
    /// Populated during `ChatWidget::new` by filtering out collapsed cells.
    /// Used by `build_context_menu_entries` to convert line-meta indices
    /// back to original indices for the `HideCell` / `ShowCell` actions.
    pub collapsed_cell_map: Vec<usize>,

    /// Whether `/edit` has loaded the last user message into the composer and
    /// the next submit should replace (not append to) the last exchange.
    pub edit_in_progress: bool,

    /// Whether LSP diagnostics are currently enabled. Mirrors the config file
    /// `[lsp].enabled` setting. Toggled at runtime via `/lsp on|off`.
    pub lsp_enabled: bool,
    /// Current-turn LSP repair-loop summary for Ctrl-O Turn Inspector (#4107).
    pub lsp_repair: LspRepairState,
    /// Derived title for the current session shown in the composer border.
    /// Updated when `EngineEvent::SessionUpdated` fires or a saved session is loaded.
    pub session_title: Option<String>,

    /// User-configured tab/window title for the current session, shown as
    /// `[title] …` in front of the terminal window title. Set with the
    /// `/title` command and persisted on the saved session; distinct from
    /// [`session_title`](Self::session_title), which is the session *name*
    /// shown in the composer border and session picker.
    pub window_title: Option<String>,
    /// Default tab/window title from the `title` config key (or a profile
    /// overlay). Used when the current session has no explicit
    /// [`window_title`](Self::window_title).
    pub title_default: Option<String>,

    /// Post-turn receipt rendered as transient composer chrome.
    /// Set when a turn completes; cleared when a new turn starts or after expiry.
    pub receipt_text: Option<String>,
    pub receipt_started_at: Option<Instant>,
    /// Tool evidence collected during the current turn for the receipt.
    pub tool_evidence: Vec<ToolEvidence>,
}

pub(crate) struct ToolRunCache {
    pub(crate) history_version: u64,
    pub(crate) active_cell_revision: u64,
    pub(crate) active_len: usize,
    pub(crate) threshold: usize,
    pub(crate) mode: ToolCollapseMode,
    pub(crate) calm_mode: bool,
    pub(crate) runs: Vec<crate::tui::history::ToolRun>,
}

impl Default for ToolRunCache {
    fn default() -> Self {
        Self {
            history_version: u64::MAX,
            active_cell_revision: u64::MAX,
            active_len: usize::MAX,
            threshold: usize::MAX,
            mode: ToolCollapseMode::Expanded,
            calm_mode: false,
            runs: Vec::new(),
        }
    }
}

// === Deref to ComposerState for backward compat ===

impl std::ops::Deref for App {
    type Target = ComposerState;
    fn deref(&self) -> &Self::Target {
        &self.composer
    }
}

impl std::ops::DerefMut for App {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.composer
    }
}

// === App State ===

pub(crate) fn default_composer_arrows_scroll(use_mouse_capture: bool) -> bool {
    default_composer_arrows_scroll_for_platform(use_mouse_capture, cfg!(windows))
}

fn default_composer_arrows_scroll_for_platform(use_mouse_capture: bool, _is_windows: bool) -> bool {
    !use_mouse_capture
}

fn push_enabled_provider_model(
    enabled: &mut HashMap<String, Vec<String>>,
    provider: &str,
    model: &str,
) {
    let provider = provider.trim();
    let model = model.trim();
    if provider.is_empty() || model.is_empty() || model.eq_ignore_ascii_case("auto") {
        return;
    }
    let models = enabled.entry(provider.to_string()).or_default();
    if !models
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(model))
    {
        models.push(model.to_string());
    }
}

impl App {
    /// Who owns the keyboard right now.
    ///
    /// The single derivation of [`Focus`], mirroring the order in which the
    /// event loop actually offers a key to each surface. Shell bindings ask
    /// this; only genuine composer *editing* keys may ask whether the
    /// composer has text.
    #[must_use]
    pub fn focus(&self) -> Focus {
        if let Some(kind) = self.view_stack.top_kind() {
            return Focus::Modal(kind);
        }
        if self.onboarding != OnboardingState::None {
            return Focus::Onboarding;
        }
        if self.launch.visible {
            return Focus::Launch;
        }
        if self.work_surface.focused
            || self
                .workflow_panel
                .as_ref()
                .is_some_and(|panel| panel.keyboard_focus)
        {
            return Focus::Panel;
        }
        Focus::Composer
    }

    /// Whether the live terminal is on the alternate screen buffer.
    ///
    /// Derived from [`App::screen_mode`] rather than stored, so every
    /// pause/resume/teardown site reads the mode the terminal is actually in
    /// after a `/fullscreen` or `/inline` switch.
    #[must_use]
    pub const fn use_alt_screen(&self) -> bool {
        self.screen_mode.uses_alt_screen()
    }

    /// Persist the pending session route as the explicit choice (`/fleet save`,
    /// `/fleet save-as`, `/model save-default`). Returns the receipt
    /// message naming the exact file written — or an error message when the
    /// write failed. Nothing is ever written without this explicit call.
    pub fn apply_route_save_choice(
        &mut self,
        choice: crate::tui::views::route_save_prompt::RouteSaveChoice,
    ) -> String {
        use crate::fleet::store::{FleetFile, FleetOperator, save_fleet, set_selected};
        use crate::tui::views::route_save_prompt::RouteSaveChoice;
        let Some(pending) = self.pending_route_save.take() else {
            return "No pending route change to save.".to_string();
        };
        let route = format!("{}/{}", pending.provider_identity, pending.model);
        match choice {
            RouteSaveChoice::UpdateFleet => {
                let Some((name, scope)) = pending.fleet.clone() else {
                    return "Nothing to update — no Fleet is selected. Use /fleet save-as to \
                             save this route as a new Fleet."
                        .to_string();
                };
                match crate::fleet::store::load_fleet_in_scope(&name, scope, &self.workspace) {
                    Ok((mut fleet, _source_path)) => {
                        fleet.operator = Some(FleetOperator {
                            provider: pending.provider_identity.clone(),
                            model: pending.model.clone(),
                            reasoning: fleet.operator.as_ref().and_then(|op| op.reasoning.clone()),
                        });
                        match save_fleet(&fleet, scope, &self.workspace) {
                            Ok(path) => format!(
                                "Fleet `{}` now runs on {route} — wrote {}",
                                fleet.name,
                                path.display()
                            ),
                            Err(err) => format!("Fleet update failed: {err}"),
                        }
                    }
                    Err(err) => format!(
                        "Fleet update failed: {err} — the saved Fleet may have moved. Use \
                         /fleet save-as to persist the route."
                    ),
                }
            }
            RouteSaveChoice::SaveAsNewFleet => {
                let display = format!(
                    "{} {}",
                    crate::config::ApiProvider::parse(&pending.provider_identity)
                        .map(|p| p.display_name().to_string())
                        .unwrap_or_else(|| pending.provider_identity.clone()),
                    pending.model
                );
                let Ok(mut fleet) = FleetFile::new(
                    display.clone(),
                    Some("Saved from a session route choice.".to_string()),
                ) else {
                    return "Could not create the Fleet.".to_string();
                };
                fleet.operator = Some(FleetOperator {
                    provider: pending.provider_identity.clone(),
                    model: pending.model.clone(),
                    reasoning: None,
                });
                match save_fleet(
                    &fleet,
                    crate::fleet::store::FleetScope::Personal,
                    &self.workspace,
                ) {
                    Ok(path) => {
                        let selected_note = match set_selected(
                            &display,
                            crate::fleet::store::FleetScope::Personal,
                            &self.workspace,
                        ) {
                            Ok(sel_path) => format!(
                                " — selected as your user-global default; wrote {}",
                                sel_path.display()
                            ),
                            Err(err) => format!(" — selection failed: {err}"),
                        };
                        format!(
                            "Saved route {route} as new Fleet `{}` — wrote {}{selected_note}",
                            display,
                            path.display()
                        )
                    }
                    Err(err) => format!("Save failed: {err}"),
                }
            }
            RouteSaveChoice::SaveAsDefault => {
                persist_route_as_startup_default(&pending.provider_identity, &pending.model)
            }
            RouteSaveChoice::SessionOnly => {
                format!("Model {route} kept for this session only — nothing was written.")
            }
        }
    }

    /// Persist the route this session is *actually* running as the startup
    /// default.
    ///
    /// This reads the live route rather than [`Self::pending_route_save`] on
    /// purpose. The pending record is bookkeeping for the save *prompt*, and it
    /// is written by several different paths (same-provider apply, cross-
    /// provider `switch_provider`, `/model`). Cross-checking it before an
    /// explicit "make this my default" action meant that any ordering
    /// disagreement dropped the write with no error shown — the user saw a
    /// normal "Model: x → y" line and reasonably assumed it had stuck, then the
    /// next launch reopened the old route. An explicit request now always
    /// reports what it did.
    pub fn save_live_route_as_startup_default(&mut self) -> String {
        match self.try_save_live_route_as_startup_default() {
            Ok(receipt) => receipt,
            Err(err) => format!("Save failed: {err}"),
        }
    }

    /// Persist the live route with a typed failure for onboarding, whose next
    /// transition depends on knowing that the restart route actually landed.
    pub(crate) fn try_save_live_route_as_startup_default(&mut self) -> anyhow::Result<String> {
        let provider_identity = self.provider_identity_for_persistence().to_string();
        let model = if self.auto_model {
            "auto".to_string()
        } else {
            self.model.clone()
        };
        try_persist_route_as_startup_default(&provider_identity, &model)?;
        // Resolve the prompt only after the write lands. If persistence fails,
        // keep the retry available instead of discarding the operator's route.
        self.pending_route_save = None;
        Ok(format!(
            "Remembered {provider_identity}/{model} as the startup default (settings.toml)."
        ))
    }

    /// Record that the live session route changed to `provider_identity` /
    /// `model`. The change is temporary until the user explicitly chooses how
    /// to save it; nothing is written here.
    pub fn note_session_route_change(&mut self, provider_identity: &str, model: &str) {
        let fleet =
            crate::fleet::store::selected_fleet(&self.workspace).map(|sel| (sel.name, sel.scope));
        self.pending_route_save = Some(PendingRouteSave {
            provider_identity: provider_identity.to_string(),
            model: model.to_string(),
            fleet,
        });
    }

    /// One truthful chip for cumulative session cost surfaces.
    ///
    /// Session history wins over the *current* route: switching to an OAuth or
    /// local route must not hide spend already accrued on a metered route, and
    /// an unpriced turn turns a displayed amount into a subtotal rather than a
    /// complete total.
    #[must_use]
    pub fn cumulative_usage_chip(&self) -> crate::route_billing::UsageChip {
        let displayed = self.displayed_session_cost_for_currency(self.cost_currency);
        let (priced, unpriced) = match self.cost_display_currency(self.cost_currency) {
            CostCurrency::Usd => (
                self.session.cost_priced_turns,
                self.session.cost_unpriced_turns,
            ),
            CostCurrency::Cny => (
                self.session.cost_cny_priced_turns,
                self.session.cost_cny_unpriced_turns,
            ),
        };
        if self.session.cost_coverage_unknown_legacy {
            return if displayed.is_finite() && displayed > 0.0 {
                crate::route_billing::UsageChip::PricedSubtotal {
                    amount: self.format_cost_amount(displayed),
                    legacy: true,
                }
            } else {
                crate::route_billing::UsageChip::Unknown
            };
        }
        if unpriced > 0 {
            return if displayed.is_finite() && displayed > 0.0 {
                crate::route_billing::UsageChip::PricedSubtotal {
                    amount: self.format_cost_amount(displayed),
                    legacy: false,
                }
            } else {
                crate::route_billing::UsageChip::Unknown
            };
        }
        if priced > 0 {
            return if displayed.is_finite() && displayed > 0.0 {
                crate::route_billing::UsageChip::Money(self.format_cost_amount(displayed))
            } else {
                crate::route_billing::UsageChip::Hidden
            };
        }
        crate::route_billing::usage_chip(
            self.billing_presentation,
            self.api_provider,
            &self.model,
            displayed,
            self.cost_display_currency(self.cost_currency),
            None,
        )
    }

    pub fn enable_provider_model(&mut self, provider: &str, model: &str) {
        push_enabled_provider_model(&mut self.enabled_provider_models, provider, model);
    }

    #[must_use]
    pub fn provider_model_is_enabled(&self, provider: &str, model: &str) -> bool {
        self.enabled_provider_models
            .get(provider)
            .is_some_and(|models| {
                models
                    .iter()
                    .any(|enabled| enabled.eq_ignore_ascii_case(model))
            })
    }

    /// Advance and return the model-draft generation. Call when a draft is
    /// requested or a setup/fleet wizard opens; a spawned draft that captured
    /// an older generation is dropped on delivery.
    pub fn next_draft_gen(&self) -> u64 {
        self.draft_gen
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1
    }

    /// The current model-draft generation (delivery compares against this).
    #[must_use]
    pub fn current_draft_gen(&self) -> u64 {
        self.draft_gen.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Cap on the session turn-cache history. Holds enough turns to debug a long
    /// session without being so large the on-screen `/cache` table wraps.
    pub const TURN_CACHE_HISTORY_CAP: usize = 50;

    /// Append a per-turn cache-telemetry record, trimming the oldest entry once
    /// the ring exceeds [`Self::TURN_CACHE_HISTORY_CAP`].
    pub fn push_turn_cache_record(&mut self, record: TurnCacheRecord) {
        self.session.turn_cache_history.push_back(record);
        while self.session.turn_cache_history.len() > Self::TURN_CACHE_HISTORY_CAP {
            self.session.turn_cache_history.pop_front();
        }
    }

    pub(crate) fn clear_model_scoped_telemetry(&mut self) {
        self.session.last_prompt_tokens = None;
        self.session.last_completion_tokens = None;
        self.session.last_output_throughput = None;
        self.session.last_prompt_cache_hit_tokens = None;
        self.session.last_prompt_cache_miss_tokens = None;
        self.session.last_reasoning_replay_tokens = None;
        self.session.turn_cache_history.clear();
        self.pending_turn_route = None;
        self.pending_auto_route_receipt = None;
        self.active_turn = None;
        self.last_effective_model = None;
        self.last_effective_provider = None;
        self.last_effective_provider_identity = None;
        self.last_auto_route_receipt = None;
        self.last_pinned_prefix_hash = None;
        self.prefix_pin_reason = None;
        self.prefix_last_miss_reason = None;
        self.prefix_drift_count = 0;
        self.prefix_context_updates = 0;
    }

    /// Invalidate facts that were accepted under the previous reasoning
    /// request.
    ///
    /// A fixed model keeps the same concrete route when its reasoning tier
    /// changes, so only its effective-reasoning receipt becomes stale. Under
    /// Auto, reasoning is one of the classifier inputs; the previous concrete
    /// provider/model route therefore cannot be replayed or displayed as the
    /// route for the new request.
    pub(crate) fn invalidate_route_receipts_for_reasoning_change(&mut self) {
        self.last_effective_reasoning_effort = None;
        if self.auto_model {
            self.last_effective_model = None;
            self.last_effective_provider = None;
            self.last_effective_provider_identity = None;
            self.last_auto_route_receipt = None;
        }
    }

    pub fn tr(&self, id: MessageId) -> Cow<'static, str> {
        tr(self.ui_locale, id)
    }

    fn discover_cached_skills(
        workspace: &std::path::Path,
        skills_dir: &std::path::Path,
        scan_codewhale_only: bool,
        plugins: &crate::plugins::PluginRegistry,
    ) -> Vec<(String, String)> {
        crate::skills::discover_for_workspace_and_dir_with_mode_and_plugins(
            workspace,
            skills_dir,
            crate::skills::SkillDiscoveryMode::from_codewhale_only(scan_codewhale_only),
            Some(plugins),
        )
        .into_enabled()
        .list()
        .iter()
        .map(|s| (s.name.clone(), s.description.clone()))
        .collect()
    }

    pub fn refresh_skill_cache(&mut self) {
        crate::skills::clear_skill_discovery_cache();
        let skills_dir = self.skills_dir.clone();
        let cached_skills = Self::discover_cached_skills(
            &self.workspace,
            &skills_dir,
            self.skills_scan_codewhale_only,
            self.plugin_registry.as_ref(),
        );
        self.hotbar_actions.replace_skills(&cached_skills);
        self.cached_skills = cached_skills;
    }

    pub fn finish_onboarding_without_feature_intro(&mut self) {
        self.onboarding = OnboardingState::None;
        if let Err(err) = crate::tui::onboarding::mark_onboarded() {
            self.status_message = Some(format!("Failed to mark onboarding: {err}"));
        }
        self.needs_redraw = true;
    }

    /// Mark the first-run follow-up as seen without inserting a transcript
    /// message. The empty underwater launch surface owns setup guidance; a
    /// synthetic history cell would hide that surface before the user sends
    /// anything.
    pub fn maybe_show_feature_intro(&mut self) {
        if self.onboarding != OnboardingState::None {
            return;
        }
        // Never claim "setup is ready" when auth is still missing — e.g.
        // `--skip-onboarding` with no API key (#3985). Leave the flag unset so
        // the tip can appear after the user finishes provider setup.
        if self.onboarding_needs_api_key {
            return;
        }
        // One transaction: the "already shown?" read and the flag write must not
        // straddle another writer's whole-file save.
        let write = Settings::transact_opt(|settings| {
            if settings.feature_intro_shown {
                return Ok(None);
            }
            settings.feature_intro_shown = true;
            Ok(Some(()))
        });
        match write {
            Ok(None) => return,
            Ok(Some(())) => {}
            Err(err) => {
                self.status_message = Some(format!("Failed to save feature-intro flag: {err}"));
                // Still show the nudge; the flag write may simply retry next launch.
            }
        }
        self.status_message = Some(self.tr(MessageId::FleetReadyNotice).into_owned());
        self.needs_redraw = true;
    }

    /// Apply a locale tag selected from the onboarding language picker (#566).
    /// Persists the value to settings.toml and immediately
    /// re-resolves `ui_locale` so the rest of onboarding renders in the new
    /// language. `App` doesn't keep `Settings` resident — it loads on entry
    /// and rewrites on exit, mirroring the pattern used by the `/config`
    /// surface.
    pub fn set_locale_from_onboarding(&mut self, tag: &str) -> anyhow::Result<()> {
        let locale = Settings::transact(|settings| {
            settings.set("locale", tag)?;
            Ok(settings.locale.clone())
        })?;
        self.ui_locale = crate::localization::resolve_locale(&locale);
        self.needs_redraw = true;
        Ok(())
    }

    /// Locale tag currently persisted in settings.toml (or
    /// `"auto"` when no settings file exists). Used by the onboarding
    /// language picker to highlight the current selection without `App`
    /// having to keep `Settings` resident.
    pub fn current_locale_tag(&self) -> String {
        Settings::load()
            .map(|s| s.locale)
            .unwrap_or_else(|_| "auto".to_string())
    }

    pub fn set_mode(&mut self, mode: AppMode) -> bool {
        let previous_mode = self.mode;
        if previous_mode == mode && !self.yolo {
            return false;
        }

        self.mode = mode;
        // Mode chip lives in the header — skip redundant status/toast copy.

        // Mode cycling is untangled from permission policy (#3386). The user
        // only edits the durable permission surface while in Agent mode, so
        // refresh the baseline from the live mirrors whenever we leave Agent —
        // before any transient Plan/YOLO policy overwrites them. This subsumes
        // the old per-mode `YoloRestoreState`/`PlanRestoreState` snapshots:
        // cross-mode hops (Plan -> YOLO, YOLO -> Plan) do not touch the baseline,
        // so YOLO's elevated authority never bleeds into the restored Agent
        // surface (#3279).
        if previous_mode.uses_agent_baseline() && !self.yolo {
            self.mode_prefs = ModeSessionPrefs {
                agent_allow_shell: self.allow_shell,
                agent_trust_mode: self.trust_mode,
                agent_approval_mode: self.approval_mode,
            };
        }

        let policy = base_policy_for_mode(mode, &self.mode_prefs);
        self.allow_shell = policy.allow_shell;
        self.trust_mode = policy.trust_mode;
        self.approval_mode = policy.approval_mode;
        self.yolo = matches!(policy.approval_mode, ApprovalMode::Bypass);

        self.finish_mode_change(previous_mode);
        true
    }

    /// Legacy YOLO entry points (`--yolo` launch, Alt+Y, the `/mode` yolo
    /// alias, `/zidong`). YOLO is a permission change (Full Access + trust + shell),
    /// not a mode change: the installed mode stays Act and the elevated
    /// authority lives in transient full-access mirrors, never in the durable
    /// Agent baseline (#3386/#3279).
    pub fn set_mode_yolo_compat(&mut self) -> bool {
        // YOLO is a permission change. A locked approval policy must not be
        // sidestepped by --yolo, default_mode=yolo, /zidong, or Alt+Y.
        if self.approval_policy_locked() {
            return false;
        }
        let previous_mode = self.mode;
        // Same baseline-refresh rule as a mode hop: the elevation must not
        // bleed into the restored Agent surface.
        if previous_mode.uses_agent_baseline() && !self.yolo {
            self.mode_prefs = ModeSessionPrefs {
                agent_allow_shell: self.allow_shell,
                agent_trust_mode: self.trust_mode,
                agent_approval_mode: self.approval_mode,
            };
        }
        // The legacy alias always lands in Act, never in Plan or Operate.
        self.mode = AppMode::Agent;
        // Transient full-access mirrors; do not persist trust/shell elevation
        // into the durable Agent baseline.
        if self.shell_access_editable {
            self.allow_shell = true;
        }
        self.trust_mode = true;
        self.approval_mode = ApprovalMode::Bypass;
        self.yolo = true;
        self.notify_yolo_compat_once();
        self.finish_mode_change(previous_mode);
        true
    }

    /// Apply the legacy YOLO selection from a user-facing entry point
    /// (Alt+Y, the `/mode` yolo alias, `/zidong`). A locked approval policy owns the
    /// permission surface and refuses here; otherwise this behaves like
    /// [`Self::select_mode`] and persists the mode actually installed (Act).
    pub fn select_yolo_compat(&mut self) -> SettingSelection {
        if self.reject_setting_change_while_busy(MessageId::SettingSubjectMode) {
            return SettingSelection::Refused;
        }
        if self.approval_policy_locked() {
            self.push_status_toast(
                "Permissions are controlled by config or managed requirements".to_string(),
                StatusToastLevel::Warning,
                Some(6_000),
            );
            self.needs_redraw = true;
            return SettingSelection::Refused;
        }
        let changed = self.set_mode_yolo_compat();
        self.startup_defaults
            .spawn(crate::tui::startup_defaults::StartupDefaults::mode(
                self.mode,
            ));
        if changed {
            SettingSelection::Changed
        } else {
            SettingSelection::PersistedSame
        }
    }

    /// Shared tail of every mode transition: ModeChange hooks plus redraw.
    /// Built from `base_hook_context` so this event carries the same session
    /// id, workspace, model, and token total as every other event — it used
    /// to omit `DEEPSEEK_SESSION_ID` entirely, which made mode transitions
    /// uncorrelatable with the session they belonged to.
    fn finish_mode_change(&mut self, previous_mode: AppMode) {
        let context = self
            .base_hook_context()
            .with_mode(self.mode.label())
            .with_previous_mode(previous_mode.label());
        if let Err(error) = self.submit_hooks(HookEvent::ModeChange, context) {
            self.surface_observer_hook_submission_failure(error);
        }
        self.needs_redraw = true;
    }

    /// Apply a *user-facing* mode selection: change the live session mode and
    /// persist it as the startup default.
    ///
    /// This is the difference between [`Self::set_mode`] and this method.
    /// `set_mode` is the session-only primitive — session restore and preset
    /// application use it because they are re-installing a mode the user
    /// already chose elsewhere, and re-persisting there would let a restored
    /// session silently rewrite the startup default. Every interactive
    /// selector (Tab/Shift+Tab cycling, the Alt+A/P shortcuts, the hotbar
    /// mode actions) goes through here instead, so "I switched to Operate"
    /// survives a restart (reported by Hunter against v0.9.1). The legacy
    /// YOLO entry points go through [`Self::select_yolo_compat`] instead.
    ///
    /// The write is queued, not performed here: it is ordered behind every
    /// earlier selection by [`StartupDefaultsWriter`], and a failure surfaces
    /// through [`Self::drain_startup_default_failures`] rather than being
    /// dropped.
    ///
    /// What is persisted is `self.mode` — the mode `set_mode` actually
    /// installed — not the requested enum.
    ///
    /// The outcome is typed, not a bool, because three things can happen and
    /// only one of them means "nothing was saved":
    ///
    /// - [`SettingSelection::Changed`] — live mode moved *and* the startup
    ///   default was queued.
    /// - [`SettingSelection::PersistedSame`] — live mode was already the
    ///   requested one, but the startup default was still queued. This is a
    ///   real, reportable action: after a session restore the live mode and the
    ///   startup default routinely disagree.
    /// - [`SettingSelection::Refused`] — the #2982 turn lock rejected it and
    ///   nothing was written anywhere.
    ///
    /// A bool collapsed the last two, so every caller (slash `/mode`, the
    /// Alt+A/P/Y shortcuts, the hotbar mode rows) reported a refusal and a
    /// successful same-mode save identically — as "already in that mode", with
    /// no receipt for the write that did happen.
    ///
    /// [`StartupDefaultsWriter`]: crate::tui::startup_defaults::StartupDefaultsWriter
    pub fn select_mode(&mut self, mode: AppMode) -> SettingSelection {
        if self.reject_setting_change_while_busy(MessageId::SettingSubjectMode) {
            return SettingSelection::Refused;
        }
        let changed = self.set_mode(mode);
        // Persist an explicit selection even when it matches the live mode.
        // A restored session can be Operate while the startup default remains
        // Act; choosing Operate again is a request to make the visible state
        // durable, not a no-op.
        self.startup_defaults
            .spawn(crate::tui::startup_defaults::StartupDefaults::mode(
                self.mode,
            ));
        if changed {
            SettingSelection::Changed
        } else {
            SettingSelection::PersistedSame
        }
    }

    /// The receipt for an accepted selection that did not move live state.
    ///
    /// Without it a same-live selection is indistinguishable from a refusal on
    /// screen, even though it wrote the file the user was trying to change.
    #[must_use]
    pub fn mode_startup_default_receipt(&self, mode: AppMode) -> String {
        self.tr(MessageId::ModeAlreadyActiveSavedAsDefault)
            .replace("{mode}", mode.display_name())
    }

    /// Surface any startup-default write that failed since the last drain.
    /// Called once per event-loop iteration.
    pub fn drain_startup_default_failures(&mut self) {
        for failure in self.startup_defaults.drain_failures() {
            let message = self.startup_default_failure_message(&failure);
            self.push_status_toast(message, StatusToastLevel::Warning, Some(8_000));
        }
    }

    /// Translate a typed startup-default failure at the locale boundary.
    ///
    /// The writer runs on a blocking pool and knows nothing about the user's
    /// locale, so it reports `StartupDefaultSubject` values and a path-free
    /// detail. Turning those into a sentence is this side's job.
    #[must_use]
    pub fn startup_default_failure_message(
        &self,
        failure: &crate::tui::startup_defaults::StartupDefaultFailure,
    ) -> String {
        use crate::tui::startup_defaults::StartupDefaultSubject;

        let subject = if failure.subjects.is_empty() {
            self.tr(MessageId::StartupDefaultSubjectAll).into_owned()
        } else {
            failure
                .subjects
                .iter()
                .map(|subject| {
                    self.tr(match subject {
                        StartupDefaultSubject::Mode => MessageId::StartupDefaultSubjectMode,
                        StartupDefaultSubject::Thinking => MessageId::StartupDefaultSubjectThinking,
                        StartupDefaultSubject::Model => MessageId::StartupDefaultSubjectModel,
                    })
                    .into_owned()
                })
                .collect::<Vec<_>>()
                // A separator, not a word: composed in code per the crate's
                // localization rules.
                .join(" + ")
        };
        self.tr(MessageId::StartupDefaultNotSaved)
            .replace("{setting}", &subject)
            .replace("{error}", &failure.detail)
    }

    fn notify_yolo_compat_once(&mut self) {
        if self.yolo_compat_notified {
            return;
        }
        self.yolo_compat_notified = true;
        // Per-install suppression: check the persisted flag so the toast
        // appears exactly once across sessions, not every launch.
        if let Ok(settings) = crate::settings::Settings::load()
            && settings.yolo_deprecation_shown
        {
            return;
        }
        // Persist the flag best-effort; toast still fires even if the write
        // fails (retries on the next attempt).
        let _ = crate::settings::Settings::transact(|settings| {
            settings.yolo_deprecation_shown = true;
            Ok(())
        });
        self.push_status_toast(
            "Legacy full-access mode is deprecated — use Act + Full Access (Shift+Tab)".to_string(),
            StatusToastLevel::Warning,
            Some(8_000),
        );
    }

    /// One-release migration notice for the Shift+Tab/Ctrl+T rebinding: users
    /// pressing Shift+Tab expecting the old thinking cycle land here first.
    fn notify_keybinding_migration_once(&mut self) {
        if self.keybinding_migration_notified {
            return;
        }
        self.keybinding_migration_notified = true;
        self.push_status_toast(
            "Shift+Tab now cycles permissions — reasoning effort moved to Ctrl+T".to_string(),
            StatusToastLevel::Info,
            Some(8_000),
        );
    }

    /// Whether mode/thinking selection is locked because a turn is in flight.
    ///
    /// While `is_loading`, the model/permission surface the engine is acting on
    /// must not shift underneath it, so user-initiated mode and thinking changes
    /// are refused (#2982). Returns true (and posts a concise status message) if
    /// the change should be rejected — the caller leaves the selection unchanged
    /// so the chip "twitches" back instead of moving.
    ///
    /// `subject` is a `MessageId`, not a `&str`, so the refusal is translated
    /// as one sentence in the user's locale instead of splicing an English noun
    /// into a translated template.
    pub(crate) fn reject_setting_change_while_busy(&mut self, subject: MessageId) -> bool {
        if self.is_loading {
            let message = self.setting_locked_message(subject);
            self.status_message = Some(message);
            self.needs_redraw = true;
            true
        } else {
            false
        }
    }

    /// The localized "locked while a turn is running" sentence for `subject`.
    #[must_use]
    pub(crate) fn setting_locked_message(&self, subject: MessageId) -> String {
        self.tr(MessageId::SettingLockedDuringTurn)
            .replace("{setting}", self.tr(subject).as_ref())
    }

    /// Cycle through productive modes: Plan → Act → Operate → Plan.
    pub fn cycle_mode(&mut self) {
        let next = self.mode.next();
        let outcome = self.select_mode(next);
        self.report_mode_selection(next, outcome);
    }

    /// Cycle through modes in reverse.
    #[allow(dead_code)]
    pub fn cycle_mode_reverse(&mut self) {
        let next = self.mode.previous();
        let outcome = self.select_mode(next);
        self.report_mode_selection(next, outcome);
    }

    /// Show the startup-default receipt for a selection that did not move live
    /// mode. `Changed` and `Refused` already have their own messaging (the mode
    /// chip, and `reject_setting_change_while_busy` respectively).
    pub(crate) fn report_mode_selection(&mut self, mode: AppMode, outcome: SettingSelection) {
        if outcome == SettingSelection::PersistedSame {
            let receipt = self.mode_startup_default_receipt(mode);
            self.status_message = Some(receipt);
            self.needs_redraw = true;
        }
    }

    /// Cycle reasoning-effort through the active route's distinct tiers.
    ///
    /// Typed for the same reason as [`Self::select_mode`]: a bool could not tell
    /// the hotbar whether the turn lock refused the action or the provider
    /// simply exposes a single tier.
    pub fn cycle_effort(&mut self) -> SettingSelection {
        if self.reject_setting_change_while_busy(MessageId::SettingSubjectThinking) {
            return SettingSelection::Refused;
        }
        let previous = self.reasoning_effort;
        self.apply_reasoning_effort_cycle();
        if self.reasoning_effort == previous {
            SettingSelection::PersistedSame
        } else {
            SettingSelection::Changed
        }
    }

    /// Advance reasoning effort to the next tier for the active route and
    /// surface the change: set a status message and refresh the compaction
    /// budget. Auto routing retains the full provider-neutral vocabulary until
    /// dispatch; a concrete model walks the same ladder as `/model` and
    /// `/effort`. Shared by the Ctrl+T shortcut (`cycle_effort`) and the
    /// hotbar `reasoning.cycle` action so the two paths cannot drift.
    pub(crate) fn apply_reasoning_effort_cycle(&mut self) {
        let requested = self.next_reasoning_effort_for_active_route();
        self.commit_reasoning_effort(requested);
    }

    fn next_reasoning_effort_for_active_route(&self) -> ReasoningEffort {
        if self.auto_model {
            return self.reasoning_effort.cycle_next_for_auto_model();
        }
        let (provider, base_url, model) = match self.active_reasoning_route_truth() {
            Some((provider, _, endpoint, model)) => (provider, endpoint, model),
            None => (
                self.api_provider,
                self.active_route_base_url.as_str(),
                self.model.as_str(),
            ),
        };
        let efforts =
            crate::tui::model_picker::picker_efforts_for_route(provider, base_url, model, false);
        self.reasoning_effort.cycle_next_in(&efforts)
    }

    pub(crate) fn commit_reasoning_effort(&mut self, requested: ReasoningEffort) {
        let effective = self.effective_reasoning_effort_for_active_route(requested);
        let route_truth = self.active_reasoning_route_truth();
        let provider_kind = route_truth.map_or(self.api_provider, |(provider, _, _, _)| provider);
        let provider = route_truth.map_or_else(
            || self.provider_identity_for_persistence().to_string(),
            |(_, provider_identity, _, _)| provider_identity.to_string(),
        );
        let endpoint_identity = route_truth
            .map(|(_, _, endpoint, _)| crate::route_receipt::endpoint_identity(endpoint));
        let model = route_truth.map(|(_, _, _, model)| model.to_string());
        if let Some(work) = self.runtime_services.work.clone()
            && let Err(err) = work.record_reasoning_effort_change(
                self.current_session_id.as_deref(),
                requested.into(),
                effective.into(),
                provider_kind,
                &provider,
                endpoint_identity.as_deref(),
                model.as_deref(),
            )
        {
            self.status_message = Some(format!(
                "Reasoning effort unchanged: Work receipt failed ({err})"
            ));
            self.needs_redraw = true;
            return;
        }
        self.reasoning_effort = requested;
        self.reasoning_effort_preference = Some(requested);
        self.invalidate_route_receipts_for_reasoning_change();
        // Same persistence owner as the model/effort pickers, so Ctrl+T and the
        // hotbar `reasoning.cycle` action restore on restart exactly like a
        // picker selection does. Only the *requested* tier is persisted — the
        // effective tier is a per-turn route fact, not a user preference.
        self.startup_defaults.spawn(
            crate::tui::startup_defaults::StartupDefaults::reasoning_effort(requested.as_setting()),
        );
        self.update_model_compaction_budget();
        self.status_message = Some(format!(
            "Reasoning effort: {}",
            Self::reasoning_effort_resolution_label(requested, effective, self.api_provider)
        ));
        self.needs_redraw = true;
    }

    /// Cycle the durable Agent permission posture: Ask → Auto-Review → Bypass.
    pub fn cycle_approval_posture(&mut self) -> bool {
        let Some(next) = self.next_approval_posture(false) else {
            return false;
        };
        if self.approval_policy_locked() {
            self.push_status_toast(
                "Permissions are controlled by config or managed requirements".to_string(),
                StatusToastLevel::Warning,
                Some(6_000),
            );
            self.needs_redraw = true;
            return false;
        }
        if let Err(err) = Self::persist_permission_posture(next) {
            self.push_status_toast(
                format!("Permissions were not changed: could not save TUI posture ({err})"),
                StatusToastLevel::Warning,
                Some(8_000),
            );
            self.needs_redraw = true;
            return false;
        }
        self.finish_approval_posture_change(next);
        true
    }

    /// Cycle permissions when the only controlling source is the user's
    /// editable root `config.toml` key. Shift+Tab is an explicit request to
    /// adopt the TUI posture, so persist the next setting first, then remove
    /// the shadowing root key. Roll back the setting if that removal fails.
    pub fn cycle_root_approval_posture(&mut self) -> bool {
        let Some(next) = self.next_approval_posture(true) else {
            return false;
        };
        if !self.approval_policy_root_editable {
            self.push_status_toast(
                "Permissions are controlled by a non-editable policy source".to_string(),
                StatusToastLevel::Warning,
                Some(6_000),
            );
            self.needs_redraw = true;
            return false;
        }

        if let Err(reason) = self.adopt_root_approval_posture(next) {
            self.push_status_toast(
                format!("Permissions were not changed: {reason}"),
                StatusToastLevel::Warning,
                Some(8_000),
            );
            self.needs_redraw = true;
            return false;
        }

        true
    }

    /// Save a real TUI permission posture and release the user-owned root
    /// `approval_policy` that would otherwise shadow it. This is shared by
    /// Shift+Tab and the config choice editor so both surfaces make the same
    /// atomic transition from raw policy tokens to the three product postures.
    pub(crate) fn adopt_root_approval_posture(&mut self, next: ApprovalMode) -> Result<(), String> {
        if !self.approval_policy_root_editable {
            return Err("the root approval policy is not editable".to_string());
        }

        let active_config_path = crate::config::resolve_load_config_path(self.config_path.clone())
            .map_err(|error| error.to_string())?;
        // The posture commit, the root-key release, and the rollback are one
        // critical section. Two `Settings::transact` calls would expose the
        // uncommitted middle state — a concurrent writer (a queued startup-default
        // drain, say) could load the new posture, and the rollback save would then
        // also revert whatever that writer had committed in between.
        /// Why the critical section ended, carried out so every toast is
        /// pushed after the settings lock is released.
        enum RootPostureOutcome {
            Committed,
            Failed(String),
        }

        let posture = Self::approval_posture_setting(next).to_string();
        let outcome = crate::settings::with_settings_transaction(|transaction| {
            let mut settings = match transaction.load() {
                Ok(settings) => settings,
                Err(err) => {
                    return Ok(RootPostureOutcome::Failed(format!(
                        "could not load TUI settings ({err})"
                    )));
                }
            };
            let previous = settings.permission_posture.clone();
            settings.permission_posture = Some(posture);
            if let Err(err) = transaction.save(&settings) {
                return Ok(RootPostureOutcome::Failed(format!(
                    "could not save TUI posture ({err})"
                )));
            }

            if let Err(err) = crate::config_persistence::persist_unset_root_key(
                active_config_path.as_deref(),
                "approval_policy",
            ) {
                settings.permission_posture = previous;
                let rollback_note = transaction
                    .save(&settings)
                    .err()
                    .map(|rollback| format!("; settings rollback also failed: {rollback}"))
                    .unwrap_or_default();
                return Ok(RootPostureOutcome::Failed(format!(
                    "could not release root config policy ({err}){rollback_note}"
                )));
            }
            Ok(RootPostureOutcome::Committed)
        })
        .unwrap_or_else(|err| {
            RootPostureOutcome::Failed(format!("could not lock TUI settings ({err})"))
        });
        if let RootPostureOutcome::Failed(reason) = outcome {
            return Err(reason);
        }

        self.clear_saved_approval_policy_lock();
        self.finish_approval_posture_change(next);
        Ok(())
    }

    fn next_approval_posture(&mut self, allow_root_policy: bool) -> Option<ApprovalMode> {
        if self.reject_setting_change_while_busy(MessageId::SettingSubjectPermissions) {
            return None;
        }
        if self.mode == AppMode::Plan {
            self.push_status_toast(
                "Plan is Read Only; switch to Act to change permissions".to_string(),
                StatusToastLevel::Info,
                Some(5_000),
            );
            self.needs_redraw = true;
            return None;
        }
        if allow_root_policy && !self.approval_policy_root_editable {
            return None;
        }
        Some(self.mode_prefs.agent_approval_mode.cycle_permission_next())
    }

    fn approval_posture_setting(mode: ApprovalMode) -> &'static str {
        match mode {
            ApprovalMode::Suggest => "ask",
            ApprovalMode::Auto => "auto-review",
            ApprovalMode::Bypass => "full-access",
            ApprovalMode::Never => "never",
        }
    }

    /// Persist the Shift+Tab permission posture.
    ///
    /// Synchronous on purpose: `cycle_approval_posture` only moves the live
    /// posture if this succeeded, so the keystroke already required the write.
    /// It runs inside [`Settings::transact`] so it cannot interleave with a
    /// queued mode/thinking write — the two used to load the same bytes and the
    /// later save reverted the other's field.
    fn persist_permission_posture(next: ApprovalMode) -> anyhow::Result<()> {
        Settings::transact(|settings| {
            settings.permission_posture = Some(Self::approval_posture_setting(next).to_string());
            Ok(())
        })
    }

    fn finish_approval_posture_change(&mut self, next: ApprovalMode) {
        self.set_agent_approval_posture(next);
        self.needs_redraw = true;
        // Footer permission chip is canonical — no status toast for the new
        // value, only the one-shot rebinding notice.
        self.notify_keybinding_migration_once();
    }

    /// Replace the complete durable Act baseline and project it onto the live
    /// runtime when the current mode uses that baseline. Keeping these three
    /// fields together prevents setup presets from updating a live mirror while
    /// leaving the next Plan → Act transition stale.
    pub fn set_agent_runtime_baseline(
        &mut self,
        allow_shell: bool,
        trust_mode: bool,
        approval_mode: ApprovalMode,
    ) {
        self.mode_prefs = ModeSessionPrefs {
            agent_allow_shell: allow_shell,
            agent_trust_mode: trust_mode,
            agent_approval_mode: approval_mode,
        };
        if self.mode.uses_agent_baseline() {
            let policy = base_policy_for_mode(self.mode, &self.mode_prefs);
            self.allow_shell = policy.allow_shell;
            self.trust_mode = policy.trust_mode;
            self.approval_mode = policy.approval_mode;
            self.yolo = matches!(policy.approval_mode, ApprovalMode::Bypass);
        }
    }

    #[must_use]
    pub(crate) fn agent_trust_baseline(&self) -> bool {
        self.mode_prefs.agent_trust_mode
    }

    /// Update the durable Act shell choice without disturbing trust or
    /// approval. The live mirror changes only while Act owns the runtime.
    pub fn set_agent_shell_access(&mut self, allow_shell: bool) {
        self.set_agent_runtime_baseline(
            allow_shell,
            self.mode_prefs.agent_trust_mode,
            self.mode_prefs.agent_approval_mode,
        );
    }

    /// Host path for `/auto`: persist Auto-Review as the TUI permission
    /// posture without inventing a second runtime. Same write as Shift+Tab
    /// landing on Auto-Review; Plan stays read-only and only the Act baseline
    /// moves.
    pub fn apply_auto_review_posture(&mut self) -> Result<(), String> {
        if self.reject_setting_change_while_busy(MessageId::SettingSubjectPermissions) {
            return Err(self.setting_locked_message(MessageId::SettingSubjectPermissions));
        }
        if self.approval_policy_locked() {
            return Err("Permissions are controlled by config or managed requirements".to_string());
        }
        Self::persist_permission_posture(ApprovalMode::Auto)
            .map_err(|err| format!("could not save TUI posture ({err})"))?;
        self.set_agent_approval_posture(ApprovalMode::Auto);
        self.needs_redraw = true;
        Ok(())
    }

    /// Update the durable Act approval choice. Entering Full Access enables
    /// trust mode; leaving it removes that implicit elevation while preserving
    /// an independently enabled trust baseline in other posture transitions.
    /// Plan remains read-only.
    pub fn set_agent_approval_posture(&mut self, next: ApprovalMode) {
        let trust_mode = if next == ApprovalMode::Bypass {
            true
        } else if self.mode_prefs.agent_approval_mode == ApprovalMode::Bypass {
            false
        } else {
            self.mode_prefs.agent_trust_mode
        };
        self.set_agent_runtime_baseline(self.mode_prefs.agent_allow_shell, trust_mode, next);
    }

    #[must_use]
    pub fn approval_policy_locked(&self) -> bool {
        self.approval_policy_locked
    }

    #[cfg(test)]
    #[must_use]
    pub fn approval_policy_requirements_managed(&self) -> bool {
        self.approval_policy_requirements_managed
    }

    /// Session transitions must never detach live runtime producers. Late
    /// engine, compaction, purge, or background-task events could otherwise
    /// contaminate the replacement session after clear/load/new.
    #[must_use]
    pub fn session_transition_blocked(&self) -> bool {
        self.is_loading
            || self.runtime_turn_status.as_deref() == Some("in_progress")
            || self.is_compacting
            || self.manual_compaction_queued
            || self.is_purging
            || self
                .task_panel
                .iter()
                .any(|task| matches!(task.status.as_str(), "queued" | "running"))
    }

    /// Whether the interface is asking the user to make a decision. Ambient
    /// motion yields across the whole frame while this is true; freezing one
    /// task marker still leaves distracting movement in peripheral vision.
    #[must_use]
    pub fn attention_hold_active(&self) -> bool {
        !self.view_stack.is_empty()
            || self.pending_user_input_prompt.is_some()
            || self
                .task_panel
                .iter()
                .any(|task| matches!(task.status.as_str(), "waiting" | "needs_user"))
    }

    pub fn mark_approval_policy_locked(&mut self) {
        self.approval_policy_locked = true;
        self.approval_policy_root_editable = true;
    }

    pub fn clear_saved_approval_policy_lock(&mut self) {
        if !self.approval_policy_requirements_managed {
            self.approval_policy_locked = false;
            self.approval_policy_root_editable = false;
        }
    }

    /// Execute hooks for a specific event with the given context
    pub fn execute_hooks(&self, event: HookEvent, context: &HookContext) -> Vec<HookResult> {
        self.hooks.execute(event, context)
    }

    /// Submit observer hooks off the terminal event loop. Foreground in hook
    /// configuration still means ordered/awaited within the worker; it no
    /// longer means the UI waits on the child process.
    pub fn submit_hooks(&self, event: HookEvent, context: HookContext) -> Result<(), String> {
        self.hooks.submit_observer(event, context)
    }

    /// Preserve a lost observer event independently of the ordinary status
    /// line. Agent lifecycle handlers immediately replace `status_message`
    /// with their normal progress text, so a submission failure belongs in
    /// the toast queue instead of that transient slot.
    pub fn surface_observer_hook_submission_failure(&mut self, error: String) {
        tracing::warn!(target: "hooks", %error, "observer hook was not submitted");
        self.push_status_toast(error, StatusToastLevel::Error, Some(12_000));
        self.needs_redraw = true;
    }

    /// Create a hook context with common fields pre-populated
    pub fn base_hook_context(&self) -> HookContext {
        HookContext::new()
            .with_mode(self.mode.label())
            .with_workspace(self.workspace.clone())
            .with_model(&self.model)
            .with_session_id(self.hooks.session_id())
            .with_tokens(self.session.total_tokens)
    }

    /// Soft cap on [`Self::history`] length. When history exceeds this count,
    /// the oldest cells are folded into a single placeholder to bound memory
    /// and render cost (#399 S2). The cap is generous — 5000 cells is more
    /// than enough to keep the visible transcript intact across sessions.
    pub const HISTORY_SOFT_CAP: usize = 5_000;

    /// Number of oldest cells to fold when the soft cap fires. Folding in
    /// batches amortizes the cost instead of triggering on every push.
    const HISTORY_FOLD_BATCH: usize = 1_000;

    pub fn add_message(&mut self, msg: HistoryCell) {
        // An in-flight tool is bound to a *virtual* index, `history.len() +
        // entry_index`, resolved against `history.len()` at completion time.
        // Growing history here without re-basing those bindings makes each of
        // them silently mean a different cell, so the completion lands on the
        // wrong one and the real row spins forever (#5478 — reproduced by
        // `/rename` mid-turn, but every command that reports a message hits it).
        self.rebase_active_cell_bindings(1);
        let rev = self.fresh_history_revision();
        self.history.push(msg);
        self.history_revisions.push(rev);
        self.history_version = self.history_version.wrapping_add(1);

        // Bound history length: when the soft cap fires, fold the oldest
        // batch into a single ArchivedContext placeholder.
        self.maybe_fold_history();
        let selection_has_range = self
            .viewport
            .transcript_selection
            .ordered_endpoints()
            .is_some_and(|(start, end)| start != end);
        if self.viewport.transcript_scroll.is_at_tail()
            && !self.viewport.transcript_selection.dragging
            && !selection_has_range
            && !self.user_scrolled_during_stream
            // While a worker's transcript owns the conversation area, its
            // pin governs the visible viewport: main-conversation activity
            // must not yank the user's read position in the focused
            // transcript (same stick-to-bottom rule as the main pane).
            && self
                .agent_focus
                .as_ref()
                .is_none_or(|focus| focus.scroll_top.is_none())
        {
            self.scroll_to_bottom();
        }
    }

    /// Add `delta` to the parent-turn session cost and bump the displayed
    /// high-water mark so the footer total never reverses (#244).
    #[allow(dead_code)]
    pub fn accrue_session_cost(&mut self, delta: f64) {
        self.accrue_session_cost_estimate(CostEstimate::usd_only(delta));
    }

    /// Record what a turn's pricing attempt actually produced.
    ///
    /// Called with the same audit that feeds [`Self::accrue_session_cost_estimate`],
    /// so the completeness counters can never drift from the running total.
    /// Routes that do not meter money at all (OAuth, token plans, local models)
    /// are not counted in either bucket — there is no dollar figure to be
    /// incomplete about.
    pub fn record_turn_cost_audit(&mut self, audit: &crate::pricing::TurnCostAudit) {
        // Provenance is recorded for every audited turn, priced or not: knowing
        // *which* row a total was built from is part of explaining the total.
        if let Some(provenance) = audit.provenance.as_ref() {
            self.session
                .cost_pricing_provenances
                .insert(provenance.label().to_string());
        }
        if let Some(defect) = audit.live_pricing_defect.as_ref() {
            if audit.estimate.is_some() {
                self.session
                    .cost_live_pricing_defects
                    .insert(defect.label().to_string());
            } else {
                self.session
                    .cost_live_pricing_unusable_defects
                    .insert(defect.label().to_string());
            }
        }
        // An exactly non-metered route has no dollar figure to be incomplete
        // about, so it joins neither coverage bucket. Everything else does,
        // including a route whose billing basis could not be established.
        if !audit.counts_toward_money_coverage() {
            return;
        }
        for class in &audit.unpriced_classes {
            self.session
                .cost_unpriced_classes
                .insert(class.label().to_string());
        }
        if !audit.usd_priced
            && let Some(reason) = audit.unpriced_reason
        {
            self.session
                .cost_unpriced_reasons
                .insert(reason.label().to_string());
        }
        if !audit.cny_priced {
            self.session.cost_cny_unpriced_reasons.insert(
                audit
                    .unpriced_reason
                    .map_or("currency_not_published", |reason| reason.label())
                    .to_string(),
            );
        }
        if audit.usd_priced {
            self.session.cost_priced_turns = self.session.cost_priced_turns.saturating_add(1);
        } else {
            self.session.cost_unpriced_turns = self.session.cost_unpriced_turns.saturating_add(1);
        }
        if audit.cny_priced {
            self.session.cost_cny_priced_turns =
                self.session.cost_cny_priced_turns.saturating_add(1);
        } else {
            self.session.cost_cny_unpriced_turns =
                self.session.cost_cny_unpriced_turns.saturating_add(1);
        }
    }

    /// Record the route a turn's cost was resolved against, redacted.
    pub fn record_turn_cost_route_receipt(&mut self, receipt: String) {
        // Bound the set so a session that rotates routes cannot grow it without
        // limit; the first 32 distinct routes are more than enough to explain a
        // total, and the cap is reported rather than silently truncating.
        const MAX_ROUTE_RECEIPTS: usize = 32;
        if self.session.cost_route_receipts.len() < MAX_ROUTE_RECEIPTS {
            self.session.cost_route_receipts.insert(receipt);
        } else {
            self.session
                .cost_route_receipts
                .insert("…additional routes not recorded (receipt cap reached)".to_string());
        }
    }

    /// Fold a drained background-cost pool's coverage into the session's.
    ///
    /// The caller has already added `pool.estimate` to the running total; this
    /// adds the counters and provenance that qualify it, from the same drained
    /// value, so the two can never disagree.
    pub fn absorb_background_cost_coverage(
        &mut self,
        pool: &crate::cost_status::PendingBackgroundCost,
    ) {
        self.session.cost_priced_turns = self
            .session
            .cost_priced_turns
            .saturating_add(pool.priced_turns);
        self.session.cost_unpriced_turns = self
            .session
            .cost_unpriced_turns
            .saturating_add(pool.unpriced_turns);
        self.session.cost_cny_priced_turns = self
            .session
            .cost_cny_priced_turns
            .saturating_add(pool.cny_priced_turns);
        self.session.cost_cny_unpriced_turns = self
            .session
            .cost_cny_unpriced_turns
            .saturating_add(pool.cny_unpriced_turns);
        for reason in &pool.unpriced_reasons {
            self.session
                .cost_unpriced_reasons
                .insert((*reason).to_string());
        }
        for reason in &pool.cny_unpriced_reasons {
            self.session
                .cost_cny_unpriced_reasons
                .insert((*reason).to_string());
        }
        for class in &pool.unpriced_classes {
            self.session
                .cost_unpriced_classes
                .insert((*class).to_string());
        }
        for provenance in &pool.pricing_provenances {
            self.session
                .cost_pricing_provenances
                .insert((*provenance).to_string());
        }
        for defect in &pool.live_pricing_defects {
            self.session
                .cost_live_pricing_defects
                .insert((*defect).to_string());
        }
        for defect in &pool.live_pricing_unusable_defects {
            self.session
                .cost_live_pricing_unusable_defects
                .insert((*defect).to_string());
        }
        for receipt in &pool.route_receipts {
            self.record_turn_cost_route_receipt(receipt.clone());
        }
    }

    /// Fold one atomic background batch into the live session projection.
    /// Returns whether the batch carried runtime-owned response identities and
    /// therefore needs a fresh durable snapshot (it may have landed after the
    /// ordinary TurnComplete save).
    pub fn absorb_pending_background_cost(
        &mut self,
        pool: &crate::cost_status::PendingBackgroundCost,
    ) -> bool {
        let runtime_usage_arrived = !pool.usage_source_fingerprints.is_empty();
        self.session
            .subagent_usage_sources
            .extend(pool.usage_source_fingerprints.iter().cloned());
        if pool.estimate.is_positive() {
            self.accrue_subagent_cost_estimate(pool.estimate);
        }
        self.absorb_background_cost_coverage(pool);
        runtime_usage_arrived
    }

    /// Clear every live cost-coverage counter.
    ///
    /// Used by `/new` and by the session-load path: loading a session must not
    /// leave the previous session's priced/unpriced turns attached to a total
    /// that no longer contains them (#4318).
    pub fn reset_cost_coverage(&mut self) {
        self.session.cost_priced_turns = 0;
        self.session.cost_unpriced_turns = 0;
        self.session.cost_cny_priced_turns = 0;
        self.session.cost_cny_unpriced_turns = 0;
        self.session.cost_unpriced_reasons.clear();
        self.session.cost_cny_unpriced_reasons.clear();
        self.session.cost_unpriced_classes.clear();
        self.session.cost_pricing_provenances.clear();
        self.session.cost_live_pricing_defects.clear();
        self.session.cost_live_pricing_unusable_defects.clear();
        self.session.cost_route_receipts.clear();
        self.session.cost_coverage_unknown_legacy = false;
    }

    /// Add a dual-currency parent-turn cost estimate.
    pub fn accrue_session_cost_estimate(&mut self, estimate: CostEstimate) {
        let total = CostEstimate {
            usd: self.session.session_cost,
            cny: self.session.session_cost_cny,
        }
        .saturating_add(estimate);
        self.session.session_cost = total.usd;
        self.session.session_cost_cny = total.cny;
        self.refresh_displayed_cost_high_water();
    }

    /// Fold one in-flight model call's priced receipt into the pending-turn
    /// estimate so cost surfaces move during a long agentic turn rather than
    /// only when it completes. The turn's authoritative price still lands via
    /// `accrue_session_cost_estimate` at `TurnComplete`; callers must
    /// `clear_pending_turn_cost` there first so nothing counts twice.
    pub fn accrue_pending_turn_cost_estimate(&mut self, estimate: CostEstimate) {
        let total = CostEstimate {
            usd: self.session.pending_turn_cost,
            cny: self.session.pending_turn_cost_cny,
        }
        .saturating_add(estimate);
        self.session.pending_turn_cost = total.usd;
        self.session.pending_turn_cost_cny = total.cny;
        self.refresh_displayed_cost_high_water();
    }

    /// Drop the in-flight turn's provisional estimate. Called at
    /// `TurnComplete` (any outcome) immediately before the authoritative
    /// cumulative price accrues; the display stays monotonic through the
    /// swap via the high-water mark (#244).
    pub fn clear_pending_turn_cost(&mut self) {
        self.session.pending_turn_cost = 0.0;
        self.session.pending_turn_cost_cny = 0.0;
    }

    /// Add `delta` to the running sub-agent cost and bump the displayed
    /// high-water mark so the footer total never reverses (#244).
    #[allow(dead_code)]
    pub fn accrue_subagent_cost(&mut self, delta: f64) {
        self.accrue_subagent_cost_estimate(CostEstimate::usd_only(delta));
    }

    /// Add a dual-currency sub-agent/background cost estimate.
    pub fn accrue_subagent_cost_estimate(&mut self, estimate: CostEstimate) {
        let total = CostEstimate {
            usd: self.session.subagent_cost,
            cny: self.session.subagent_cost_cny,
        }
        .saturating_add(estimate);
        self.session.subagent_cost = total.usd;
        self.session.subagent_cost_cny = total.cny;
        self.refresh_displayed_cost_high_water();
    }

    /// Copy current session/subagent cost accumulators into session metadata
    /// for persistence.
    pub fn sync_cost_to_metadata(&self, metadata: &mut crate::session_manager::SessionMetadata) {
        metadata.cost.session_cost_usd = self.session.session_cost;
        metadata.cost.session_cost_cny = self.session.session_cost_cny;
        metadata.cost.subagent_cost_usd = self.session.subagent_cost;
        metadata.cost.subagent_cost_cny = self.session.subagent_cost_cny;
        metadata.cost.displayed_cost_high_water_usd = self.session.displayed_cost_high_water;
        metadata.cost.displayed_cost_high_water_cny = self.session.displayed_cost_high_water_cny;
        // Coverage travels with the money it qualifies. A restored total without
        // these fields cannot say what it covers, and its serde defaults read as
        // a *complete* total covering zero turns — so they are persisted together
        // and `coverage_recorded` marks that this writer actually knew (#4318).
        metadata.cost.priced_turns = self.session.cost_priced_turns;
        metadata.cost.unpriced_turns = self.session.cost_unpriced_turns;
        metadata.cost.cny_priced_turns = self.session.cost_cny_priced_turns;
        metadata.cost.cny_unpriced_turns = self.session.cost_cny_unpriced_turns;
        metadata.cost.unpriced_reasons = self.session.cost_unpriced_reasons.clone();
        metadata.cost.cny_unpriced_reasons = self.session.cost_cny_unpriced_reasons.clone();
        metadata.cost.unpriced_classes = self.session.cost_unpriced_classes.clone();
        metadata.cost.pricing_provenances = self.session.cost_pricing_provenances.clone();
        metadata.cost.live_pricing_defects = self.session.cost_live_pricing_defects.clone();
        metadata.cost.live_pricing_unusable_defects =
            self.session.cost_live_pricing_unusable_defects.clone();
        metadata.cost.route_receipts = self.session.cost_route_receipts.clone();
        metadata.cost.usage_source_fingerprints = self
            .session
            .subagent_usage_sources
            .iter()
            .cloned()
            .collect();
        // A session restored as legacy-unknown stays unknown when re-saved:
        // re-writing it as "recorded" would launder the missing evidence into an
        // apparently complete zero.
        metadata.cost.coverage_recorded = !self.session.cost_coverage_unknown_legacy;
        // Persist cumulative turn duration so the footer "worked" chip
        // survives session save/restore (#2038).
        metadata.cumulative_turn_secs = self.cumulative_turn_duration.as_secs();
    }

    /// Recompute the displayed cost high-water mark. Called any time a cost
    /// counter is mutated; never decreases.
    pub fn refresh_displayed_cost_high_water(&mut self) {
        let current = CostEstimate {
            usd: self.session.session_cost,
            cny: self.session.session_cost_cny,
        }
        .saturating_add(CostEstimate {
            usd: self.session.pending_turn_cost,
            cny: self.session.pending_turn_cost_cny,
        })
        .saturating_add(CostEstimate {
            usd: self.session.subagent_cost,
            cny: self.session.subagent_cost_cny,
        });
        if current.usd > self.session.displayed_cost_high_water {
            self.session.displayed_cost_high_water = current.usd;
        }
        if current.cny > self.session.displayed_cost_high_water_cny {
            self.session.displayed_cost_high_water_cny = current.cny;
        }
    }

    /// Read the visible session+sub-agent cost. Guaranteed monotonic across
    /// reconciliation events (cache adjustments, provisional → final swaps)
    /// for the lifetime of one session (#244).
    #[allow(dead_code)]
    pub fn displayed_session_cost(&self) -> f64 {
        self.displayed_session_cost_for_currency(CostCurrency::Usd)
    }

    /// Read the visible session+sub-agent cost in the chosen currency.
    pub fn displayed_session_cost_for_currency(&self, currency: CostCurrency) -> f64 {
        match self.cost_display_currency(currency) {
            CostCurrency::Usd => {
                let current = CostEstimate {
                    usd: self.session.session_cost,
                    cny: 0.0,
                }
                .saturating_add(CostEstimate {
                    usd: self.session.pending_turn_cost,
                    cny: 0.0,
                })
                .saturating_add(CostEstimate {
                    usd: self.session.subagent_cost,
                    cny: 0.0,
                })
                .usd;
                current.max(self.session.displayed_cost_high_water)
            }
            CostCurrency::Cny => {
                let current = CostEstimate {
                    usd: 0.0,
                    cny: self.session.session_cost_cny,
                }
                .saturating_add(CostEstimate {
                    usd: 0.0,
                    cny: self.session.pending_turn_cost_cny,
                })
                .saturating_add(CostEstimate {
                    usd: 0.0,
                    cny: self.session.subagent_cost_cny,
                })
                .cny;
                current.max(self.session.displayed_cost_high_water_cny)
            }
        }
    }

    /// The session's own display share: settled turns plus the in-flight
    /// turn's provisional estimate (the running turn's money is session
    /// money, so the sidebar breakdown keeps summing to the displayed total
    /// mid-turn).
    pub fn session_cost_for_currency(&self, currency: CostCurrency) -> f64 {
        match self.cost_display_currency(currency) {
            CostCurrency::Usd => self.session.session_cost + self.session.pending_turn_cost,
            CostCurrency::Cny => self.session.session_cost_cny + self.session.pending_turn_cost_cny,
        }
    }

    pub fn subagent_cost_for_currency(&self, currency: CostCurrency) -> f64 {
        match self.cost_display_currency(currency) {
            CostCurrency::Usd => self.session.subagent_cost,
            CostCurrency::Cny => self.session.subagent_cost_cny,
        }
    }

    pub fn format_cost_amount(&self, amount: f64) -> String {
        crate::pricing::format_cost_amount(amount, self.cost_display_currency(self.cost_currency))
    }

    /// A [`CostEstimate`] in the session's display currency — the same rule
    /// `format_cost_amount` applies, so a turn receipt and the session total
    /// never disagree on `$` versus `¥`.
    pub fn format_cost_estimate(&self, estimate: CostEstimate) -> String {
        crate::pricing::format_cost_estimate(
            estimate,
            self.cost_display_currency(self.cost_currency),
        )
    }

    /// Price is one number, everywhere (design §2.11 item 5): the session
    /// cost as the footer chip, the price view, and the roster print it.
    /// Money routes print the amount; subscription, local, and unknown
    /// routes print the same words the chip uses; a metered route that has
    /// not spent yet prints `$0.00` rather than vanishing between turns.
    #[must_use]
    pub fn session_cost_label(&self) -> String {
        let chip = self.cumulative_usage_chip();
        crate::route_billing::format_usage_chip(&chip).unwrap_or_else(|| {
            self.format_cost_amount(self.displayed_session_cost_for_currency(self.cost_currency))
        })
    }

    pub fn format_cost_amount_precise(&self, amount: f64) -> String {
        crate::pricing::format_cost_amount_precise(
            amount,
            self.cost_display_currency(self.cost_currency),
        )
    }

    pub(crate) fn cost_display_currency(&self, currency: CostCurrency) -> CostCurrency {
        if currency == CostCurrency::Cny
            && self.session.cost_cny_priced_turns == 0
            && self.session.cost_priced_turns > 0
        {
            CostCurrency::Usd
        } else {
            currency
        }
    }

    /// Fold the oldest [`Self::HISTORY_FOLD_BATCH`] cells into a single
    /// `ArchivedContext` placeholder when history exceeds the soft cap.
    /// Called from [`Self::add_message`]; the caller is responsible for
    /// also removing the folded range from any auxiliary per-cell maps.
    fn maybe_fold_history(&mut self) {
        if self.history.len() <= Self::HISTORY_SOFT_CAP {
            return;
        }

        let fold_count = Self::HISTORY_FOLD_BATCH.min(self.history.len());
        // Don't fold into the very last cell(s) — keep a buffer of
        // non-folded cells so the visible transcript tail stays intact.
        let keep_tail = Self::HISTORY_SOFT_CAP.saturating_sub(Self::HISTORY_FOLD_BATCH);
        if self.history.len().saturating_sub(fold_count) < keep_tail {
            return;
        }

        // Gather the range of cell indices we are folding.
        let folded: Vec<HistoryCell> = self.history.drain(..fold_count).collect();
        let folded_revs: Vec<u64> = self.history_revisions.drain(..fold_count).collect();
        let _ = folded_revs; // revisions are discarded with the cells

        // Shift all per-cell index maps down by `fold_count`.
        self.shift_history_maps_down(fold_count);

        // Build a single placeholder cell summarizing the folded range.
        let total_folded = folded.len();
        let summary = format!(
            "{total_folded} older transcript cells folded to bound memory. \
             Use /sessions to load a prior session snapshot if needed."
        );
        let placeholder = HistoryCell::ArchivedContext {
            level: 0,
            range: format!("cells 0-{}", total_folded.saturating_sub(1)),
            tokens: String::new(),
            density: String::new(),
            model: String::new(),
            timestamp: String::new(),
            summary,
        };

        // Insert the placeholder at the front.
        let rev = self.fresh_history_revision();
        self.history.insert(0, placeholder);
        self.history_revisions.insert(0, rev);
        self.transcript_identity_epoch = self.transcript_identity_epoch.wrapping_add(1);
        self.history_version = self.history_version.wrapping_add(1);
        self.needs_redraw = true;
    }

    /// Shift all per-cell index maps down by `n` after removing the first
    /// `n` history cells. Every map key >= n is mapped to key - n; keys < n
    /// are dropped.
    fn shift_history_maps_down(&mut self, n: usize) {
        // A folded-range placeholder is inserted at index 0 immediately
        // after this shift, so surviving completed-output receipts move down
        // by `n` and then forward by one.
        self.completed_assistant_outputs.retain_mut(|receipt| {
            if receipt.history_index >= n {
                receipt.history_index = receipt.history_index - n + 1;
                true
            } else {
                false
            }
        });

        // tool_cells: HashMap<String, usize>
        self.tool_cells.retain(|_, idx| {
            if *idx >= n {
                *idx -= n;
                true
            } else {
                false
            }
        });

        // tool_details_by_cell: HashMap<usize, ToolDetailRecord>
        self.tool_details_by_cell = std::mem::take(&mut self.tool_details_by_cell)
            .into_iter()
            .filter_map(|(idx, detail)| {
                if idx >= n {
                    Some((idx - n, detail))
                } else {
                    None
                }
            })
            .collect();

        // context_references_by_cell
        self.context_references_by_cell = std::mem::take(&mut self.context_references_by_cell)
            .into_iter()
            .filter_map(|(idx, refs)| {
                if idx >= n {
                    Some((idx - n, refs))
                } else {
                    None
                }
            })
            .collect();
        self.rebuild_session_context_references();

        // subagent_card_index
        self.subagent_card_index.retain(|_, idx| {
            if *idx >= n {
                *idx -= n;
                true
            } else {
                false
            }
        });

        // last_fanout_card_index
        if let Some(ref mut idx) = self.last_fanout_card_index {
            if *idx >= n {
                *idx -= n;
            } else {
                self.last_fanout_card_index = None;
            }
        }

        // collapsed_cells
        self.collapsed_cells = std::mem::take(&mut self.collapsed_cells)
            .into_iter()
            .filter_map(|idx| if idx >= n { Some(idx - n) } else { None })
            .collect();
        self.folded_thinking.clear();
        self.expanded_tool_runs = std::mem::take(&mut self.expanded_tool_runs)
            .into_iter()
            .filter_map(|idx| if idx >= n { Some(idx - n) } else { None })
            .collect();
        self.collapsed_cell_map.clear();
    }

    /// Resolve the dispatch/session name for an agent. `None` when the agent
    /// is unnamed (the manager seeds `name` with the raw id) or absent from
    /// the cache — the raw id is a lookup handle, never a display name.
    fn agent_session_name(&self, agent_id: &str) -> Option<String> {
        let agent = self
            .subagent_cache
            .iter()
            .find(|agent| agent.agent_id == agent_id)?;
        let name = agent.name.trim();
        (!name.is_empty() && name != agent.agent_id).then(|| name.to_string())
    }

    /// Resolve the most specific member/role token for an agent, in priority
    /// order: resolved profile id, advisory assignment role, requested alias,
    /// canonical route role, then Fleet type. `None` only for a
    /// progress-only agent whose dispatch metadata has not arrived yet.
    fn agent_role_label(&self, agent_id: &str) -> Option<String> {
        let agent = self
            .subagent_cache
            .iter()
            .find(|agent| agent.agent_id == agent_id)?;
        agent
            .child_route
            .as_ref()
            .and_then(|route| route.resolved_profile_id.as_deref())
            .map(str::trim)
            .filter(|profile| !profile.is_empty())
            .map(str::to_string)
            .or_else(|| {
                agent
                    .assignment
                    .role
                    .as_deref()
                    .map(str::trim)
                    .filter(|role| !role.is_empty())
                    .map(str::to_string)
            })
            .or_else(|| {
                agent
                    .child_route
                    .as_ref()
                    .and_then(|route| route.requested_profile.as_deref())
                    .map(str::trim)
                    .filter(|profile| !profile.is_empty())
                    .map(str::to_string)
            })
            .or_else(|| {
                agent
                    .child_route
                    .as_ref()
                    .map(|route| route.canonical_role.trim())
                    .filter(|role| !role.is_empty())
                    .map(str::to_string)
            })
            .or_else(|| {
                let role = agent.agent_type.as_str().trim();
                (!role.is_empty()).then(|| role.to_string())
            })
            .map(|role| crate::fleet::role::public_role_label(&role))
    }

    /// `true` for the `Agent N` counter placeholder assigned before a child's
    /// dispatch metadata arrives. Placeholders are the only label that may be
    /// upgraded once the child's identity is observed.
    fn is_agent_counter_placeholder(label: &str) -> bool {
        label
            .strip_prefix("Agent ")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    }

    /// Resolve the identity-backed label for an agent, or `None` when no
    /// identity field is populated (so the caller falls back to a counter
    /// placeholder). Named children keep their name and gain a role suffix
    /// when the role is not already part of the name; unnamed children are
    /// disambiguated with a per-role sequence counter.
    fn resolved_identity_label(&mut self, agent_id: &str) -> Option<String> {
        let name = self.agent_session_name(agent_id);
        let role = self.agent_role_label(agent_id);
        match (name, role) {
            (Some(name), Some(role)) if !name.contains(&role) => Some(format!("{name} · {role}")),
            (Some(name), _) => Some(name),
            (None, Some(role)) => {
                let next = self.agent_role_counters.entry(role.clone()).or_insert(0);
                *next += 1;
                Some(format!("{role} · {next}"))
            }
            (None, None) => None,
        }
    }

    fn next_agent_placeholder(&mut self) -> String {
        self.agent_counter = self.agent_counter.saturating_add(1);
        format!("Agent {}", self.agent_counter)
    }

    /// #3030: return the stable user-facing label for an agent id. Labels are
    /// resolved from the child's own identity and never downgraded once set;
    /// only the generic `Agent N` placeholder upgrades when dispatch metadata
    /// arrives. A raw agent id is never used as a label.
    pub(crate) fn ensure_agent_label(&mut self, agent_id: &str) -> String {
        let existing = self.agent_label_map.get(agent_id).cloned();
        if let Some(existing) = existing {
            if !Self::is_agent_counter_placeholder(&existing) {
                return existing;
            }
            // Upgrade a placeholder only once identity metadata is available.
            if let Some(label) = self.resolved_identity_label(agent_id) {
                self.agent_label_map
                    .insert(agent_id.to_string(), label.clone());
                return label;
            }
            return existing;
        }
        let label = self
            .resolved_identity_label(agent_id)
            .unwrap_or_else(|| self.next_agent_placeholder());
        self.agent_label_map
            .insert(agent_id.to_string(), label.clone());
        label
    }

    /// #3030: read-only label lookup with raw-id fallback for agents the
    /// label map has never seen.
    pub(crate) fn agent_display_label(&self, agent_id: &str) -> String {
        self.agent_label_map
            .get(agent_id)
            .cloned()
            .unwrap_or_else(|| agent_id.to_string())
    }

    pub fn mark_history_updated(&mut self) {
        self.history_version = self.history_version.wrapping_add(1);
        // Resync per-cell revisions to history.len(). This is the
        // "I-don't-know-which-cell-changed" path: if cells were appended in
        // bulk (e.g. session resume, compaction), every new cell gets a
        // fresh revision; if cells were removed, drop trailing revs. We
        // intentionally do NOT bump revisions for indices that already had
        // one — the cache will reuse those. Callers that mutate a specific
        // cell's content must call `bump_history_cell(idx)` instead.
        self.resync_history_revisions();
        self.needs_redraw = true;
    }

    /// Invalidate only transcript rows whose visible liveness marker is
    /// time-based. Animation redraws must not churn settled history, but they
    /// do need fresh cache keys for running history and active-cell entries.
    pub(crate) fn mark_live_motion_updated(&mut self) {
        self.mark_live_motion_updated_inner(true);
    }

    /// Invalidate only committed live rows. The translation placeholder path
    /// already bumps the whole active-cell cache when it changes, so the UI
    /// uses this narrower path to avoid bumping that revision twice.
    pub(crate) fn mark_live_history_motion_updated(&mut self) {
        self.mark_live_motion_updated_inner(false);
    }

    fn mark_live_motion_updated_inner(&mut self, invalidate_active_cell: bool) {
        self.resync_history_revisions();
        let live_history_indices: Vec<usize> = self
            .history
            .iter()
            .enumerate()
            .filter_map(|(index, cell)| cell.has_live_motion().then_some(index))
            .collect();
        for index in live_history_indices {
            let previous_revision = self.history_revisions.get(index).copied();
            let streaming_content_len = (self.streaming_message_index == Some(index))
                .then(|| match self.history.get(index) {
                    Some(HistoryCell::Assistant {
                        content,
                        streaming: true,
                    }) => Some(content.len()),
                    _ => None,
                })
                .flatten();
            let revision = self.fresh_history_revision();
            if let Some(slot) = self.history_revisions.get_mut(index) {
                *slot = revision;
            }
            if let (Some(previous_revision), Some(content_len)) =
                (previous_revision, streaming_content_len)
            {
                let from_revision = self
                    .streaming_source_receipt
                    .filter(|receipt| {
                        receipt.cell_index == index && receipt.to_revision == previous_revision
                    })
                    .map_or(previous_revision, |receipt| receipt.from_revision);
                self.streaming_source_receipt =
                    Some(crate::tui::transcript::StreamingSourceReceipt {
                        cell_index: index,
                        from_revision,
                        to_revision: revision,
                        content_len,
                    });
            }
        }

        let active_has_live_motion = self
            .active_cell
            .as_ref()
            .is_some_and(|active| active.entries().iter().any(HistoryCell::has_live_motion));
        if invalidate_active_cell && active_has_live_motion {
            self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
            if let Some(active) = self.active_cell.as_mut() {
                active.bump_revision();
            }
        }

        self.history_version = self.history_version.wrapping_add(1);
        self.needs_redraw = true;
    }

    /// Issue a fresh, monotonically increasing revision counter for a new
    /// history cell. Wrapping is acceptable — collisions are astronomically
    /// rare and at worst trigger one extra re-render.
    fn fresh_history_revision(&mut self) -> u64 {
        let rev = self.next_history_revision;
        self.next_history_revision = self.next_history_revision.wrapping_add(1);
        rev
    }

    /// Bring `history_revisions` back into shape (`history_revisions.len() ==
    /// history.len()`). Pushes fresh revs for newly appended cells, truncates
    /// for cells that were removed. **Does not** invalidate existing entries.
    pub fn resync_history_revisions(&mut self) {
        if self.history_revisions.len() < self.history.len() {
            let needed = self.history.len() - self.history_revisions.len();
            for _ in 0..needed {
                let rev = self.fresh_history_revision();
                self.history_revisions.push(rev);
            }
        } else if self.history_revisions.len() > self.history.len() {
            self.history_revisions.truncate(self.history.len());
        }
    }

    /// Bump the revision counter of a single history cell so the transcript
    /// cache re-renders it on the next frame. Use this whenever a cell's
    /// content (e.g. a streaming Assistant body) is mutated in place.
    pub fn bump_history_cell(&mut self, idx: usize) {
        // Resync first in case callers mutated `history` directly without
        // pushing through `add_message`. After resync, the index is valid
        // (or out of bounds — in which case there's nothing to bump).
        self.resync_history_revisions();
        if self
            .streaming_source_receipt
            .is_some_and(|receipt| receipt.cell_index == idx)
        {
            self.streaming_source_receipt = None;
        }
        if let Some(rev) = self.history_revisions.get_mut(idx) {
            let new_rev = self.next_history_revision;
            self.next_history_revision = self.next_history_revision.wrapping_add(1);
            *rev = new_rev;
        }
        self.history_version = self.history_version.wrapping_add(1);
        self.needs_redraw = true;
    }

    /// Append a single history cell, allocating a fresh per-cell revision.
    /// Equivalent to `add_message` but exposed as a generic alias so call
    /// sites currently doing `app.history.push(...)` followed by
    /// `app.mark_history_updated()` can collapse to one helper.
    pub fn push_history_cell(&mut self, cell: HistoryCell) {
        let rev = self.fresh_history_revision();
        self.history.push(cell);
        self.history_revisions.push(rev);
        self.history_version = self.history_version.wrapping_add(1);
        self.maybe_fold_history();
        self.needs_redraw = true;
    }

    /// Append a batch of history cells, allocating fresh revisions.
    pub fn extend_history<I>(&mut self, cells: I)
    where
        I: IntoIterator<Item = HistoryCell>,
    {
        for cell in cells {
            let rev = self.fresh_history_revision();
            self.history.push(cell);
            self.history_revisions.push(rev);
        }
        self.maybe_fold_history();
        self.history_version = self.history_version.wrapping_add(1);
        self.needs_redraw = true;
    }

    /// Clear the history and its session-scoped side indexes. Used by /clear,
    /// session reset, and other "wipe and reload" flows.
    pub fn clear_history(&mut self) {
        self.history.clear();
        self.history_revisions.clear();
        self.completed_assistant_outputs.clear();
        self.context_references_by_cell.clear();
        self.session_context_references.clear();
        self.session_artifacts.clear();
        self.prune_transcript_index_state(0);
        self.history_version = self.history_version.wrapping_add(1);
        self.needs_redraw = true;
    }

    /// Record one user-visible assistant message after its typed completion
    /// boundary. Interrupted salvage never calls this path.
    pub(crate) fn record_completed_assistant_output(&mut self, history_index: usize, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        if let Some(receipt) = self
            .completed_assistant_outputs
            .iter_mut()
            .find(|receipt| receipt.history_index == history_index)
        {
            receipt.text = text.to_string();
            return;
        }
        self.completed_assistant_outputs
            .push(CompletedAssistantOutputReceipt {
                history_index,
                text: text.to_string(),
            });
    }

    /// Rebuild receipts only from the restored typed transcript projection.
    /// `history_cells_from_message` has already routed repair receipts to
    /// System cells and omitted interrupted assistant salvage.
    pub(crate) fn rebuild_completed_assistant_outputs_from_restored_history(&mut self) {
        self.completed_assistant_outputs = self
            .history
            .iter()
            .enumerate()
            .filter_map(|(history_index, cell)| match cell {
                HistoryCell::Assistant {
                    content,
                    streaming: false,
                } if !content.trim().is_empty() => Some(CompletedAssistantOutputReceipt {
                    history_index,
                    text: content.clone(),
                }),
                _ => None,
            })
            .collect();
    }

    pub(crate) fn completed_assistant_output_receipt(&self) -> Option<&str> {
        self.completed_assistant_outputs
            .iter()
            .rev()
            .find(|receipt| !receipt.text.trim().is_empty())
            .map(|receipt| receipt.text.as_str())
    }

    /// Pop the trailing history cell, keeping revisions in sync.
    pub fn pop_history(&mut self) -> Option<HistoryCell> {
        let cell = self.history.pop();
        if cell.is_some() {
            self.history_revisions.pop();
            self.completed_assistant_outputs
                .retain(|receipt| receipt.history_index < self.history.len());
            self.context_references_by_cell.remove(&self.history.len());
            self.rebuild_session_context_references();
            self.prune_transcript_index_state(self.history.len());
            self.history_version = self.history_version.wrapping_add(1);
            self.needs_redraw = true;
        }
        cell
    }

    /// Truncate `history` (and the parallel `history_revisions` + auxiliary
    /// per-cell maps) so that only cells with index `< new_len` remain.
    /// Used by Esc-Esc backtrack (#133) to roll the visible transcript
    /// back to a chosen user message. Cells dropped here are gone — the
    /// caller is expected to also trim the matching `api_messages` so the
    /// next turn matches what the user sees.
    pub fn truncate_history_to(&mut self, new_len: usize) {
        if new_len >= self.history.len() {
            return;
        }
        self.history.truncate(new_len);
        if self.history_revisions.len() > new_len {
            self.history_revisions.truncate(new_len);
        }
        self.completed_assistant_outputs
            .retain(|receipt| receipt.history_index < new_len);
        // Drop any auxiliary maps keyed on history indices that now point
        // past the new tail. We keep the rest intact so unaffected tool
        // cells continue to render correctly.
        self.tool_cells.retain(|_, idx| *idx < new_len);
        self.tool_details_by_cell.retain(|idx, _| *idx < new_len);
        self.context_references_by_cell
            .retain(|idx, _| *idx < new_len);
        self.rebuild_session_context_references();
        self.subagent_card_index.retain(|_, idx| *idx < new_len);
        if self
            .last_fanout_card_index
            .is_some_and(|idx| idx >= new_len)
        {
            self.last_fanout_card_index = None;
        }
        self.prune_transcript_index_state(new_len);
        self.history_version = self.history_version.wrapping_add(1);
        self.needs_redraw = true;
    }

    pub(crate) fn prune_transcript_index_state(&mut self, len: usize) {
        self.transcript_identity_epoch = self.transcript_identity_epoch.wrapping_add(1);
        self.collapsed_cells.retain(|idx| *idx < len);
        self.folded_thinking.retain(|idx| *idx < len);
        self.expanded_tool_runs.retain(|idx| *idx < len);
        self.collapsed_cell_map.clear();
    }

    #[must_use]
    pub fn tool_collapse_active(&self) -> bool {
        self.tool_collapse_threshold > 0 && self.tool_collapse_mode.is_active(self.calm_mode)
    }

    #[must_use]
    pub fn tool_run_start_for_history_index(&self, index: usize) -> Option<usize> {
        if !self.tool_collapse_active() {
            return None;
        }
        let active_entries = self
            .active_cell
            .as_ref()
            .map_or(&[][..], crate::tui::active_cell::ActiveCell::entries);
        if index >= self.history.len().saturating_add(active_entries.len()) {
            return None;
        }
        crate::tui::history::detect_tool_runs_from_slices(
            &self.history,
            active_entries,
            self.tool_collapse_threshold,
        )
        .into_iter()
        .find(|run| index >= run.start && index < run.start.saturating_add(run.count))
        .map(|run| run.start)
    }

    pub fn toggle_tool_run_expansion_at(&mut self, index: usize) -> bool {
        let Some(start) = self.tool_run_start_for_history_index(index) else {
            return false;
        };
        if self.expanded_tool_runs.remove(&start) {
            self.status_message = Some("Tool group collapsed".to_string());
        } else {
            self.expanded_tool_runs.insert(start);
            self.status_message = Some("Tool group expanded".to_string());
        }
        self.mark_history_updated();
        true
    }

    /// Bump the active-cell revision counter and request a redraw.
    ///
    /// Use this whenever an entry inside `active_cell` is mutated. The
    /// transcript cache combines this counter with `history_version` to
    /// produce a per-cell revision so the synthetic active-cell row can be
    /// re-rendered without invalidating committed history cells.
    pub fn bump_active_cell_revision(&mut self) {
        self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
        if let Some(active) = self.active_cell.as_mut() {
            active.bump_revision();
        }
        self.history_version = self.history_version.wrapping_add(1);
        self.needs_redraw = true;
    }

    /// Total number of cells in the *virtual* transcript: `history.len()`
    /// plus active cell entries (if any).
    #[must_use]
    #[allow(dead_code)] // Reserved for renderers that need a unified cell count.
    pub fn virtual_cell_count(&self) -> usize {
        self.history.len() + self.active_cell.as_ref().map_or(0, ActiveCell::entry_count)
    }

    #[must_use]
    pub fn original_cell_index_for_rendered(&self, rendered_index: usize) -> usize {
        self.collapsed_cell_map
            .get(rendered_index)
            .copied()
            .unwrap_or(rendered_index)
    }

    /// Resolve a virtual cell index to either a committed history cell or an
    /// active-cell entry. Used by the pager / details lookup code so it can
    /// transparently address still-in-flight cells.
    #[must_use]
    #[allow(dead_code)] // Used by the upcoming pager rewrite (read-only resolver).
    pub fn cell_at_virtual_index(&self, index: usize) -> Option<&HistoryCell> {
        if index < self.history.len() {
            self.history.get(index)
        } else {
            let entry_idx = index - self.history.len();
            self.active_cell
                .as_ref()
                .and_then(|active| active.entries().get(entry_idx))
        }
    }

    /// Resolve the tool-detail record for a committed or still-active virtual
    /// transcript cell.
    #[must_use]
    pub fn tool_detail_record_for_cell(&self, index: usize) -> Option<&ToolDetailRecord> {
        if let Some(detail) = self.tool_details_by_cell.get(&index) {
            return Some(detail);
        }
        self.active_tool_details
            .values()
            .find(|detail| self.tool_cells.get(&detail.tool_id).copied() == Some(index))
    }

    /// Whether a virtual transcript cell can open a meaningful `v` detail
    /// view. Thinking cells render their own raw text inline so there is no
    /// separate "raw" target. Error cells always get a full-message target so
    /// recovery instructions cannot be stranded below a short terminal view.
    #[must_use]
    pub fn cell_has_detail_target(&self, index: usize) -> bool {
        self.tool_detail_record_for_cell(index).is_some()
            || matches!(
                self.cell_at_virtual_index(index),
                Some(HistoryCell::Error { .. } | HistoryCell::Tool(_) | HistoryCell::SubAgent(_))
            )
    }

    /// Space owner: selection, newest visible cell, then latest virtual cell.
    #[must_use]
    pub(crate) fn transcript_action_owner(&self) -> Option<TranscriptActionOwner> {
        let meta = self.viewport.transcript_cache.line_meta();
        let selected = self
            .viewport
            .transcript_selection
            .ordered_endpoints()
            .and_then(|(start, _)| meta.get(start.line_index));
        let start = self.viewport.last_transcript_top.min(meta.len());
        let end = start
            .saturating_add(self.viewport.last_transcript_visible)
            .min(meta.len());
        let cell_index = selected
            .into_iter()
            .chain(meta[start..end].iter().rev())
            .find_map(|meta| {
                meta.cell_line()
                    .map(|(idx, _)| self.original_cell_index_for_rendered(idx))
                    .filter(|&idx| self.cell_at_virtual_index(idx).is_some())
            })
            .or_else(|| self.virtual_cell_count().checked_sub(1))?;
        Some(TranscriptActionOwner {
            cell_index,
            identity_epoch: self.transcript_identity_epoch,
        })
    }

    /// Pick the detail target for the current viewport. This is used by the
    /// transcript highlight and footer hint so they agree with `v`.
    #[must_use]
    pub fn detail_cell_index_for_viewport(
        &self,
        top: usize,
        visible: usize,
        line_meta: &[TranscriptLineMeta],
    ) -> Option<usize> {
        let original = |meta: &TranscriptLineMeta| {
            meta.cell_line()
                .map(|(idx, _)| self.original_cell_index_for_rendered(idx))
        };
        let selected = self
            .viewport
            .transcript_selection
            .ordered_endpoints()
            .and_then(|(start, _)| line_meta.get(start.line_index))
            .and_then(original)
            .filter(|&idx| self.cell_has_detail_target(idx));
        let start = top.min(line_meta.len().saturating_sub(1));
        let end = start.saturating_add(visible).min(line_meta.len());
        let mut visible_cells = line_meta[start..end].iter().filter_map(original);
        // A visible error is the most urgent detail target. Prefer the newest
        // visible error over an earlier tool card so Alt+V opens the failure
        // the user is looking at, even when both occupy the viewport.
        selected
            .or_else(|| {
                visible_cells.clone().rev().find(|&idx| {
                    matches!(
                        self.cell_at_virtual_index(idx),
                        Some(HistoryCell::Error { .. })
                    )
                })
            })
            .or_else(|| visible_cells.find(|&idx| self.cell_has_detail_target(idx)))
            .or_else(|| {
                (0..self.virtual_cell_count())
                    .rev()
                    .find(|&idx| self.cell_has_detail_target(idx))
            })
    }

    pub fn record_context_references(
        &mut self,
        history_cell: usize,
        message_index: usize,
        references: Vec<ContextReference>,
    ) {
        if references.is_empty() {
            return;
        }
        let records: Vec<SessionContextReference> = references
            .into_iter()
            .map(|reference| SessionContextReference {
                message_index,
                reference,
            })
            .collect();
        self.context_references_by_cell
            .insert(history_cell, records.clone());
        self.rebuild_session_context_references();
        self.needs_redraw = true;
    }

    pub fn sync_context_references_from_session(
        &mut self,
        references: &[SessionContextReference],
        message_to_cell: &HashMap<usize, usize>,
    ) {
        self.context_references_by_cell.clear();
        for record in references {
            let Some(&cell_index) = message_to_cell.get(&record.message_index) else {
                continue;
            };
            self.context_references_by_cell
                .entry(cell_index)
                .or_default()
                .push(record.clone());
        }
        self.rebuild_session_context_references();
    }

    fn rebuild_session_context_references(&mut self) {
        let mut records: Vec<SessionContextReference> = self
            .context_references_by_cell
            .values()
            .flat_map(|records| records.iter().cloned())
            .collect();
        records.sort_by_key(|record| record.message_index);
        self.session_context_references = records;
    }

    /// Mutable variant of [`Self::cell_at_virtual_index`]. Bumps the
    /// appropriate revision counter (active-cell revision when targeting an
    /// in-flight entry, history version otherwise).
    /// Shift every binding that points *into the active cell* up by `added`,
    /// to keep it pointing at the same entry after `added` cells are appended
    /// to history.
    ///
    /// Bindings below `history.len()` address finalized cells and must not
    /// move. Only called when an active cell exists: with no active cell there
    /// are no virtual indices to re-base, and shifting would corrupt real ones.
    fn rebase_active_cell_bindings(&mut self, added: usize) {
        if added == 0 || self.active_cell.is_none() {
            return;
        }
        let boundary = self.history.len();
        for index in self.tool_cells.values_mut() {
            if *index >= boundary {
                *index = index.saturating_add(added);
            }
        }
        for (cell_index, _) in self.exploring_entries.values_mut() {
            if *cell_index >= boundary {
                *cell_index = cell_index.saturating_add(added);
            }
        }
        self.active_tool_entry_completed_at =
            std::mem::take(&mut self.active_tool_entry_completed_at)
                .into_iter()
                .map(|(index, at)| {
                    if index >= boundary {
                        (index.saturating_add(added), at)
                    } else {
                        (index, at)
                    }
                })
                .collect();
    }

    pub fn cell_at_virtual_index_mut(&mut self, index: usize) -> Option<&mut HistoryCell> {
        if index < self.history.len() {
            // Bump only the targeted cell's revision; leave every other
            // cell's cached render intact.
            self.resync_history_revisions();
            if let Some(rev) = self.history_revisions.get_mut(index) {
                let new_rev = self.next_history_revision;
                self.next_history_revision = self.next_history_revision.wrapping_add(1);
                *rev = new_rev;
            }
            self.history_version = self.history_version.wrapping_add(1);
            self.history.get_mut(index)
        } else {
            let entry_idx = index - self.history.len();
            self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
            self.history_version = self.history_version.wrapping_add(1);
            self.active_cell
                .as_mut()
                .and_then(|active| active.entry_mut(entry_idx))
        }
    }

    /// Drain the active cell into history. Companion maps that reference
    /// active-cell entries by virtual index (`tool_cells`,
    /// `tool_details_by_cell`) are rewritten to point at the new history
    /// indices. Idempotent — calling this when there is no active cell is a
    /// no-op.
    ///
    /// Caller is responsible for first marking in-progress entries with the
    /// terminal status they want (e.g. via
    /// [`ActiveCell::mark_in_progress_as_interrupted`]).
    pub fn flush_active_cell(&mut self) {
        let Some(mut active) = self.active_cell.take() else {
            self.streaming_thinking_active_entry = None;
            return;
        };
        if active.is_empty() {
            self.exploring_cell = None;
            self.exploring_entries.clear();
            self.active_tool_details.clear();
            self.active_tool_entry_completed_at.clear();
            self.streaming_thinking_active_entry = None;
            self.bump_active_cell_revision();
            return;
        }

        if let Some(entry_idx) = self.streaming_thinking_active_entry.take()
            && let Some(HistoryCell::Thinking { streaming, .. }) = active.entry_mut(entry_idx)
        {
            *streaming = false;
        }

        let base_index = self.history.len();
        // Completed tools are removed from `tool_cells` before the active
        // group flushes, but `ActiveCell` deliberately keeps the stable
        // tool-to-entry binding until drain. Capture that binding first so
        // sequential or parallel tools in one model turn retain distinct raw
        // detail records instead of all falling back to the first cell.
        let detail_cell_indices: HashMap<String, usize> = self
            .active_tool_details
            .keys()
            .filter_map(|tool_id| {
                active
                    .entry_index_for_tool(tool_id)
                    .map(|entry_idx| (tool_id.clone(), base_index + entry_idx))
            })
            .collect();
        let drained = active.drain();

        let mut details = std::mem::take(&mut self.active_tool_details);
        self.active_tool_entry_completed_at.clear();
        for (tool_id, detail) in details.drain() {
            let cell_index = detail_cell_indices
                .get(&tool_id)
                .copied()
                .or_else(|| self.tool_cells.get(&tool_id).copied())
                .unwrap_or(base_index);
            self.tool_details_by_cell
                .entry(cell_index)
                .or_insert(detail);
        }

        self.exploring_cell = None;
        self.exploring_entries.clear();

        for cell in drained {
            let rev = self.fresh_history_revision();
            self.history.push(cell);
            self.history_revisions.push(rev);
        }
        self.history_version = self.history_version.wrapping_add(1);
        self.needs_redraw = true;
        let selection_has_range = self
            .viewport
            .transcript_selection
            .ordered_endpoints()
            .is_some_and(|(start, end)| start != end);
        if self.viewport.transcript_scroll.is_at_tail()
            && !self.viewport.transcript_selection.dragging
            && !selection_has_range
            && !self.user_scrolled_during_stream
            // While a worker's transcript owns the conversation area, its
            // pin governs the visible viewport: main-conversation activity
            // must not yank the user's read position in the focused
            // transcript (same stick-to-bottom rule as the main pane).
            && self
                .agent_focus
                .as_ref()
                .is_none_or(|focus| focus.scroll_top.is_none())
        {
            self.scroll_to_bottom();
        }
    }

    /// Mark every still-running entry in the active cell as interrupted, then
    /// flush. Convenience helper for cancellation paths.
    pub fn finalize_active_cell_as_interrupted(&mut self) {
        if let Some(active) = self.active_cell.as_mut() {
            active.mark_in_progress_as_interrupted();
        }
        self.flush_active_cell();
        // #4121: interrupt finalizes running workflow children as cancelled
        // and preserves the completed panel until the next run starts.
        if let Some(panel) = self.workflow_panel.as_mut() {
            panel.finalize_interrupt();
            self.needs_redraw = true;
        }
    }

    /// Apply a workflow panel event for one immutable workflow run, creating
    /// the panel on first `RunStarted`.
    ///
    /// Returns whether the event belonged to the displayed run and was
    /// applied. Budget-only updates still return `true`, but leave repaint to
    /// the caller so high-frequency fan-out budget ticks can be paced (#4095).
    /// A `RunStarted` event may select a different run only when its start is
    /// strictly newer; every other cross-run event fails closed.
    pub fn apply_workflow_panel_event(
        &mut self,
        event_run_id: &str,
        event: crate::tui::widgets::workflow_panel::WorkflowPanelEvent,
    ) -> bool {
        use crate::tui::widgets::workflow_panel::{WorkflowPanel, WorkflowPanelEvent};
        if event_run_id.trim().is_empty() {
            return false;
        }
        if let WorkflowPanelEvent::RunStarted { run_id, .. } = &event
            && run_id != event_run_id
        {
            return false;
        }
        if let Some(panel) = self.workflow_panel.as_ref()
            && panel.run_id != event_run_id
        {
            match &event {
                WorkflowPanelEvent::RunStarted { at_ms, .. } if *at_ms > panel.started_at_ms => {}
                _ => return false,
            }
        }

        let budget_only = matches!(&event, WorkflowPanelEvent::BudgetUpdated { .. });
        match (&mut self.workflow_panel, &event) {
            (
                None,
                WorkflowPanelEvent::RunStarted {
                    run_id,
                    workflow_goal,
                    workflow_id,
                    token_budget,
                    at_ms,
                    ..
                },
            ) => {
                let label = workflow_goal
                    .clone()
                    .or_else(|| workflow_id.clone())
                    .unwrap_or_else(|| "workflow".to_string());
                let mut panel = WorkflowPanel::new(run_id.clone(), label, *at_ms);
                panel.locale = self.ui_locale;
                panel.budget_total = *token_budget;
                panel.budget_remaining = *token_budget;
                self.workflow_panel = Some(panel);
            }
            (None, _) => {
                // No panel yet and event is not a start — seed a shell panel
                // so late events still surface rather than being dropped.
                let mut panel = WorkflowPanel::new(event_run_id, event_run_id, 0);
                panel.locale = self.ui_locale;
                panel.apply_event(event);
                self.workflow_panel = Some(panel);
            }
            (Some(panel), _) => {
                panel.apply_event(event);
            }
        }
        if !budget_only {
            self.needs_redraw = true;
        }
        true
    }

    /// Toggle the workflow panel expand/collapse state. Returns true when a
    /// panel was present and toggled.
    pub fn toggle_workflow_panel(&mut self) -> bool {
        let Some(panel) = self.workflow_panel.as_mut() else {
            return false;
        };
        let _ = panel.toggle_expanded();
        self.needs_redraw = true;
        true
    }

    /// How long the "press Ctrl+C again to quit" prompt stays armed before it
    /// silently expires.
    pub const QUIT_CONFIRMATION_WINDOW: Duration = Duration::from_secs(2);

    /// Arm the quit confirmation timer. The next Ctrl+C within
    /// [`Self::QUIT_CONFIRMATION_WINDOW`] should exit the app cleanly. Call this only
    /// from idle state — while a turn is in flight or a modal is open Ctrl+C
    /// retains its existing "interrupt this turn" / "close modal" semantics.
    ///
    /// A live cloud job survives the quit by design (its runner is
    /// detached), but its sandbox keeps billing until the next startup
    /// sweep reconciles it — so arming the quit prompt also surfaces that
    /// cost in the status line. Ctrl+D exits without arming and therefore
    /// without this warning.
    pub fn arm_quit(&mut self) {
        self.quit_armed_until = Some(Instant::now() + Self::QUIT_CONFIRMATION_WINDOW);
        // The armed state must be spoken, not silent: surface the localized
        // press-again hint as a typed toast with the same lifetime as the
        // confirmation window, so the user learns a second Ctrl+C exits.
        self.push_status_toast(
            self.tr(MessageId::FooterPressCtrlCAgain),
            StatusToastLevel::Info,
            Some(Self::QUIT_CONFIRMATION_WINDOW.as_millis() as u64),
        );
        if let Some(warning) = crate::cloud_dispatch::CloudJobStore::from_env()
            .ok()
            .and_then(|store| crate::cloud_dispatch::live_job_quit_warning(&store))
        {
            self.push_status_toast(warning, StatusToastLevel::Warning, Some(8_000));
        }
    }

    /// Whether the quit timer is currently armed (i.e. a prior Ctrl+C set it
    /// and it hasn't expired yet).
    pub fn quit_is_armed(&self) -> bool {
        self.quit_armed_until
            .map(|deadline| Instant::now() < deadline)
            .unwrap_or(false)
    }

    /// Clear the quit-armed timer. Call when expiry is detected on a tick or
    /// when the user takes any other action that should disarm the prompt
    /// (typing, sending a message, etc.).
    pub fn disarm_quit(&mut self) {
        if self.quit_armed_until.is_some() {
            self.quit_armed_until = None;
            self.needs_redraw = true;
        }
    }

    /// Tick called from the redraw loop. Lets time-based UI state (the
    /// quit-armed prompt) expire even when no input event is delivered.
    pub fn tick_quit_armed(&mut self) {
        if let Some(deadline) = self.quit_armed_until
            && Instant::now() >= deadline
        {
            self.quit_armed_until = None;
            self.needs_redraw = true;
        }
    }

    pub const RECEIPT_VISIBLE_DURATION: Duration = Duration::from_secs(8);

    pub fn set_receipt_text(&mut self, text: impl Into<String>) {
        self.receipt_text = Some(text.into());
        self.receipt_started_at = Some(Instant::now());
        self.needs_redraw = true;
    }

    pub fn clear_receipt(&mut self) {
        if self.receipt_text.is_some() || self.receipt_started_at.is_some() {
            self.receipt_text = None;
            self.receipt_started_at = None;
            self.needs_redraw = true;
        }
    }

    /// Tick called from the redraw loop so transient receipts leave the UI
    /// without waiting for the next keypress.
    pub fn tick_receipt(&mut self) {
        if self
            .receipt_started_at
            .is_some_and(|started| started.elapsed() > Self::RECEIPT_VISIBLE_DURATION)
        {
            self.clear_receipt();
        }
    }

    pub fn close_slash_menu(&mut self) {
        self.slash_menu_hidden = true;
        self.needs_redraw = true;
    }

    /// Ceiling on how far the ambient clock advances per sampled frame.
    /// Bursty draw schedules (fast token streams) slow the aquarium down
    /// instead of teleporting creatures across the gap.
    pub const AMBIENT_MAX_STEP_MS: u128 = 160;
    /// Gentle-motion grace before a fully idle aquarium settles still.
    pub const AMBIENT_IDLE_SETTLE_MS: u64 = 6_000;

    /// Advance and read the ambient animation clock. Every decorative
    /// position derives from this value; it moves by real elapsed time
    /// clamped to [`Self::AMBIENT_MAX_STEP_MS`] per sample, so motion stays
    /// continuous no matter how irregular the draw schedule is.
    pub fn sample_ambient_clock_ms(&mut self) -> u128 {
        let now = Instant::now();
        let step = self
            .ambient_clock_sampled_at
            .map(|last| {
                now.duration_since(last)
                    .as_millis()
                    .min(Self::AMBIENT_MAX_STEP_MS)
            })
            .unwrap_or(0);
        self.ambient_clock_sampled_at = Some(now);
        self.ambient_clock_ms = self.ambient_clock_ms.saturating_add(step);
        self.ambient_clock_ms
    }

    /// Track idleness and report whether the ambient scene has settled.
    /// `busy` is the caller's aggregation of live activity signals (running
    /// turn, live sub-agents, active durable tasks, completion exhale, user
    /// browsing). While busy the idle anchor clears; once quiet, motion gets
    /// [`Self::AMBIENT_IDLE_SETTLE_MS`] of grace and then stills.
    pub fn ambient_idle_settled(&mut self, busy: bool, now: Instant) -> bool {
        if busy {
            self.ambient_idle_since = None;
            return false;
        }
        let since = *self.ambient_idle_since.get_or_insert(now);
        now.duration_since(since) >= Duration::from_millis(Self::AMBIENT_IDLE_SETTLE_MS)
    }

    /// Resolve one motion policy for every surface that can request or paint
    /// animation. `fancy_animations = false` is a true still mode even when
    /// the separate accessibility preference is left at its default.
    #[must_use]
    pub(crate) fn motion_policy(&self) -> MotionPolicy {
        MotionPolicy::from_settings(
            self.low_motion,
            self.fancy_animations,
            self.constrained_frame_rate,
        )
    }

    /// Resolve the `[title] …` window-title prefix for the terminal title.
    ///
    /// Precedence: the session-level `/title` override wins over the `title`
    /// config default; neither configured means no prefix (the historical
    /// window titles stay byte-for-byte unchanged). This is deliberately
    /// independent of [`session_title`](Self::session_title) — the session
    /// *name* keeps identifying the composer border and picker, while this
    /// prefix only decorates the terminal window/tab title.
    #[must_use]
    pub(crate) fn window_title_prefix(&self) -> Option<&str> {
        self.window_title
            .as_deref()
            .or(self.title_default.as_deref())
            .filter(|prefix| !prefix.trim().is_empty())
    }

    /// Bridge the centralized policy into transcript renderers that still
    /// accept the legacy boolean motion contract.
    #[must_use]
    pub(crate) fn effective_low_motion_for_status(&self) -> bool {
        self.motion_policy().as_low_motion()
    }

    pub fn transcript_render_options(&self) -> TranscriptRenderOptions {
        TranscriptRenderOptions {
            locale: self.ui_locale,
            show_thinking: self.show_thinking,
            thinking_highlight: self.thinking_highlight,
            thinking_default_expanded: self.thinking_default_expanded,
            thinking_preview_lines: self.thinking_preview_lines,
            verbose: self.verbose_transcript,
            show_tool_details: self.show_tool_details,
            inline_diff_mode: self.inline_diff_mode,
            calm_mode: self.calm_mode,
            low_motion: self.effective_low_motion_for_status(),
            motion_mode: self.motion_policy().mode(),
            spacing: self.transcript_spacing,
            palette_mode: self.ui_theme.mode,
            prose_measure: self.prose_measure,
            reasoning_preview_extra_lines: 0,
            reasoning_preview_viewport_lines: None,
        }
    }

    /// Handle terminal resize event.
    pub fn handle_resize(&mut self, _width: u16, _height: u16) {
        let preserved_scroll = (!self.viewport.transcript_scroll.is_at_tail())
            .then_some(self.viewport.last_transcript_top);
        self.viewport.transcript_cache = TranscriptViewCache::new();

        if let Some(top) = preserved_scroll {
            self.viewport.transcript_scroll = TranscriptScroll::at_line(top);
        }

        self.viewport.pending_scroll_delta = 0;
        self.viewport.transcript_selection.clear();

        self.viewport.last_transcript_area = None;
        self.viewport.last_approval_area = None;
        self.viewport.last_transcript_top = 0;
        // Seed visible height from the resize event so paging keys use a
        // useful page size immediately, before the next render updates it.
        self.viewport.last_transcript_visible = (_height as usize).saturating_sub(2).max(1);
        self.viewport.last_transcript_total = 0;
        self.viewport.last_transcript_padding_top = 0;
        self.viewport.jump_to_latest_button_area = None;

        self.mark_history_updated();
    }

    pub fn scroll_up(&mut self, amount: usize) {
        let delta = i32::try_from(amount).unwrap_or(i32::MAX);
        self.viewport.pending_scroll_delta =
            self.viewport.pending_scroll_delta.saturating_sub(delta);
        self.user_scrolled_during_stream = true;
        self.needs_redraw = true;
    }

    pub fn scroll_down(&mut self, amount: usize) {
        let delta = i32::try_from(amount).unwrap_or(i32::MAX);
        self.viewport.pending_scroll_delta =
            self.viewport.pending_scroll_delta.saturating_add(delta);
        self.user_scrolled_during_stream = true;
        self.needs_redraw = true;
    }

    pub fn scroll_to_bottom(&mut self) {
        self.viewport.transcript_scroll = TranscriptScroll::to_bottom();
        self.viewport.pending_scroll_delta = 0;
        self.viewport.jump_to_latest_button_area = None;
        self.user_scrolled_during_stream = false;
        // While a worker's transcript owns the conversation area, the
        // jump-to-bottom affordances (Ctrl+End, Alt+G, the jump-to-latest
        // button, sending a follow-up) must release its pin too: the two
        // surfaces share one command set, so returning to the live tail has
        // to mean the tail the user is actually looking at.
        if let Some(focus) = self.agent_focus.as_mut() {
            focus.scroll_top = None;
        }
        self.needs_redraw = true;
    }

    pub fn queue_message(&mut self, message: QueuedMessage) {
        self.queued_messages.push_back(message);
    }

    pub fn pop_queued_message(&mut self) -> Option<QueuedMessage> {
        self.queued_messages.pop_front()
    }

    pub fn remove_queued_message(&mut self, index: usize) -> Option<QueuedMessage> {
        self.queued_messages.remove(index)
    }

    pub fn queued_message_count(&self) -> usize {
        self.queued_messages.len()
    }

    /// Pop the most-recently queued message back into the composer for editing
    /// (issue #85 — ↑ affordance). The popped message is parked in
    /// [`Self::queued_draft`] so the next Enter re-queues it carrying its
    /// original skill instruction. No-op if the composer already has typed
    /// content or a draft is already being edited — surfacing the affordance
    /// would be ambiguous in either case.
    ///
    /// Returns `true` when the composer state was mutated.
    pub fn pop_last_queued_into_draft(&mut self) -> bool {
        if !self.input.is_empty() || self.queued_draft.is_some() {
            return false;
        }
        let Some(msg) = self.queued_messages.pop_back() else {
            return false;
        };
        self.input = msg.display.clone();
        self.cursor_position = char_count(&self.input);
        self.selected_attachment_index = None;
        self.queued_draft = Some(msg);
        self.needs_redraw = true;
        true
    }

    /// Stop editing a queued follow-up and put the original queued message back
    /// at the tail where [`Self::pop_last_queued_into_draft`] took it from.
    pub fn cancel_queued_draft_edit(&mut self) -> bool {
        let Some(draft) = self.queued_draft.take() else {
            return false;
        };
        self.queued_messages.push_back(draft);
        self.clear_input_recoverable();
        self.needs_redraw = true;
        true
    }

    /// Park a legacy pending steer. New keyboard handling routes running-turn
    /// drafts through Ctrl+Enter (same-turn steer) or Enter (next-turn
    /// follow-up).
    #[allow(dead_code)]
    pub fn push_pending_steer(&mut self, message: QueuedMessage) {
        self.pending_steers.push_back(message);
        self.submit_pending_steers_after_interrupt = true;
        self.needs_redraw = true;
    }

    /// Drain the pending-steer queue and clear the resend flag. Returns the
    /// messages in submit order (oldest first).
    pub fn drain_pending_steers(&mut self) -> Vec<QueuedMessage> {
        self.submit_pending_steers_after_interrupt = false;
        if self.pending_steers.is_empty() {
            return Vec::new();
        }
        self.needs_redraw = true;
        self.pending_steers.drain(..).collect()
    }

    /// Decide how to route a fresh non-empty composer submit.
    ///
    /// Running turns always queue bare-Enter submissions. Ctrl+Enter is the
    /// single explicit gesture for amending the active turn, regardless of
    /// whether the provider has emitted its first token yet.
    ///
    /// Truth table:
    ///   offline=F, busy=F → Immediate
    ///   offline=F, busy=T, streaming=* → Queue (Ctrl+Enter steers)
    ///   offline=T, busy=* → Queue
    #[must_use]
    pub fn decide_submit_disposition(&self) -> SubmitDisposition {
        if self.offline_mode {
            return SubmitDisposition::Queue;
        }
        // A spawned dispatch is still resolving route/sending the op (#4605);
        // queue rather than spawn a second dispatch that could reorder ops.
        if self.dispatch_in_flight {
            return SubmitDisposition::Queue;
        }
        if !self.is_loading {
            return SubmitDisposition::Immediate;
        }
        // Busy: queue the message. Steer is an explicit Ctrl+Enter gesture,
        // not a timing-sensitive change in bare Enter behavior.
        SubmitDisposition::Queue
    }

    /// Resolve Enter-shaped input from the same state used by composer hints.
    ///
    /// Bare Enter is portable across supported terminals: it sends while idle,
    /// queues while busy, and an empty Enter promotes the oldest queued message
    /// into the active turn. Ctrl+Enter remains accepted when a terminal can
    /// report it distinctly, but is intentionally not advertised because many
    /// terminals encode it exactly like Enter.
    #[must_use]
    pub fn decide_composer_submit(&self, chord: ComposerSubmitChord) -> ComposerSubmitAction {
        if self.input.is_empty() {
            if self.is_loading && self.queued_draft.is_none() && !self.queued_messages.is_empty() {
                return ComposerSubmitAction::SendQueuedNow;
            }
            return ComposerSubmitAction::Noop;
        }

        let disposition = match chord {
            ComposerSubmitChord::Enter => self.decide_submit_disposition(),
            ComposerSubmitChord::CtrlEnter
                if self.is_loading && !self.offline_mode && !self.dispatch_in_flight =>
            {
                SubmitDisposition::Steer
            }
            ComposerSubmitChord::CtrlEnter => self.decide_submit_disposition(),
        };
        ComposerSubmitAction::Submit(disposition)
    }

    /// How long after a queued Enter a second, empty Enter still means
    /// "send that now" (the grokbuild double-tap, restored 2026-09-02).
    pub const DOUBLE_TAP_WINDOW: Duration = Duration::from_millis(500);

    /// Resolve what bare Enter should do right now, with double-tap
    /// detection.
    ///
    /// While the engine is busy the first Enter queues and opens the window;
    /// a second Enter inside it resolves to `Steer` — the same disposition
    /// Ctrl+Enter takes, so there is one steering path. The event loop pairs
    /// this with [`Self::take_queued_for_double_tap_steer`] on an empty
    /// composer, because the first tap already emptied it. Idle Enter
    /// submits immediately and closes any window.
    #[must_use]
    pub fn enter_with_double_tap(&mut self) -> Option<SubmitDisposition> {
        let disposition = self.decide_submit_disposition();
        match disposition {
            SubmitDisposition::Queue if !self.offline_mode && !self.dispatch_in_flight => {
                if self.double_tap_window_open() {
                    self.last_enter_instant = None;
                    return Some(SubmitDisposition::Steer);
                }
                self.last_enter_instant = Some(Instant::now());
                Some(SubmitDisposition::Queue)
            }
            other => {
                self.last_enter_instant = None;
                Some(other)
            }
        }
    }

    /// Open the double-tap window: a queued Enter happened while the engine
    /// was busy. The typed-submit path calls this when it queues.
    pub fn arm_double_tap_window(&mut self) {
        self.last_enter_instant = Some(Instant::now());
    }

    /// True while a second, empty Enter would send the just-queued message
    /// now — the posture bar advertises the gesture exactly this long.
    #[must_use]
    pub fn double_tap_window_open(&self) -> bool {
        self.is_loading
            && !self.offline_mode
            && !self.dispatch_in_flight
            && self
                .last_enter_instant
                .is_some_and(|instant| instant.elapsed() < Self::DOUBLE_TAP_WINDOW)
    }

    /// Pop the most recently queued message when the double-tap window is
    /// still open. Clears the window so a third Enter does not re-steer.
    pub fn take_queued_for_double_tap_steer(&mut self) -> Option<QueuedMessage> {
        if !self.double_tap_window_open() || self.queued_messages.is_empty() {
            return None;
        }
        match self.enter_with_double_tap() {
            Some(SubmitDisposition::Steer) => self.queued_messages.pop_back(),
            _ => None,
        }
    }

    /// Mark the in-flight streaming Assistant cell as interrupted: prepend
    /// `[interrupted]` to whatever streamed so far (so the user can see what
    /// was salvaged) and flip `streaming` off so the spinner halts. No-op if
    /// no Assistant cell is currently streaming.
    ///
    /// Deliberate divergence from openai/codex which discards partial output
    /// on abort — V4 thinking is expensive and the user usually wants to see
    /// what the model produced before steering.
    pub fn finalize_streaming_assistant_as_interrupted(&mut self) {
        let Some(index) = self.streaming_message_index.take() else {
            return;
        };
        if let Some(HistoryCell::Assistant { content, streaming }) = self.history.get_mut(index) {
            *streaming = false;
            if content.is_empty() {
                *content = "[interrupted]".to_string();
            } else if !content.starts_with("[interrupted]") {
                content.insert_str(0, "[interrupted] ");
            }
        }
        self.bump_history_cell(index);
    }

    /// Retry a `try_lock` up to `retries` times with a 1ms pause between
    /// attempts. Returns `Some(guard)` on success, `None` if the lock
    /// remains contended after all retries.
    fn retry_lock<T>(
        mutex: &tokio::sync::Mutex<T>,
        retries: u32,
    ) -> Option<tokio::sync::MutexGuard<'_, T>> {
        for _ in 0..retries {
            if let Ok(guard) = mutex.try_lock() {
                return Some(guard);
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        None
    }

    /// Capture the durable Work state without ever converting lock contention
    /// into an empty snapshot.
    pub fn work_state_snapshot(&self) -> Result<Option<SessionWorkState>, String> {
        if let Some(work) = self.runtime_services.work.as_ref() {
            return work
                .capture(self.current_session_id.as_deref())
                .map(|state| {
                    state.map(|state| SessionWorkState {
                        graph: Some(state.graph),
                        todos: state.todos,
                        plan: state.plan,
                    })
                });
        }
        let todos = Self::retry_lock(&self.todos, 100)
            .ok_or_else(|| "To-do state is busy; try saving again".to_string())?;
        let plan = Self::retry_lock(&self.plan_state, 100)
            .ok_or_else(|| "Plan state is busy; try saving again".to_string())?;
        let state = SessionWorkState {
            graph: None,
            todos: todos.snapshot(),
            plan: plan.snapshot(),
        };
        Ok((!state.is_empty()).then_some(state))
    }

    /// Non-blocking snapshot for the render/event loop. Automatic persistence
    /// must skip a contended first save instead of pausing the UI or writing a
    /// false empty state.
    pub fn try_work_state_snapshot(&mut self) -> Result<Option<SessionWorkState>, String> {
        if let Some(work) = self.runtime_services.work.as_ref() {
            let state = work
                .try_capture(self.current_session_id.as_deref())
                .map(|state| {
                    state.map(|state| SessionWorkState {
                        graph: Some(state.graph),
                        todos: state.todos,
                        plan: state.plan,
                    })
                })?;
            self.last_known_work_state = Some(state.clone());
            return Ok(state);
        }
        let todos = self
            .todos
            .try_lock()
            .map_err(|_| "To-do state is busy".to_string())?;
        let plan = self
            .plan_state
            .try_lock()
            .map_err(|_| "Plan state is busy".to_string())?;
        let state = SessionWorkState {
            graph: None,
            todos: todos.snapshot(),
            plan: plan.snapshot(),
        };
        let state = (!state.is_empty()).then_some(state);
        drop(plan);
        drop(todos);
        self.last_known_work_state = Some(state.clone());
        Ok(state)
    }

    /// Atomically replace the live Work state from a saved session.
    pub fn restore_work_state(
        &mut self,
        session_id: &str,
        workspace: &Path,
        state: Option<&SessionWorkState>,
    ) -> Result<(), String> {
        if let Some(work) = self.runtime_services.work.as_ref() {
            let empty = SessionWorkState::default();
            let state = state.unwrap_or(&empty);
            work.restore_with_workspace_owner_bindings(
                session_id,
                workspace,
                state.graph.as_ref(),
                &state.todos,
                &state.plan,
            )?;
            let restored = work.capture(Some(session_id))?;
            let normalized_state = restored.map(|state| SessionWorkState {
                graph: Some(state.graph),
                todos: state.todos,
                plan: state.plan,
            });
            self.work_surface.record_restored_session(
                session_id,
                normalized_state
                    .as_ref()
                    .and_then(|state| state.graph.as_ref()),
            );
            self.last_known_work_state = Some(normalized_state);
            return Ok(());
        }
        let (restored_todos, restored_plan) = match state {
            Some(state) => (
                TodoList::from_snapshot(&state.todos)?,
                PlanState::from_snapshot(&state.plan),
            ),
            None => (TodoList::new(), PlanState::default()),
        };
        let normalized_state = SessionWorkState {
            graph: None,
            todos: restored_todos.snapshot(),
            plan: restored_plan.snapshot(),
        };

        let mut todos = Self::retry_lock(&self.todos, 100)
            .ok_or_else(|| "To-do state is busy; session was not restored".to_string())?;
        let mut plan = Self::retry_lock(&self.plan_state, 100)
            .ok_or_else(|| "Plan state is busy; session was not restored".to_string())?;
        *todos = restored_todos;
        *plan = restored_plan;
        drop(plan);
        drop(todos);
        self.work_surface.record_restored_session(session_id, None);
        self.last_known_work_state =
            Some((!normalized_state.is_empty()).then_some(normalized_state));
        Ok(())
    }

    pub fn clear_todos(&mut self) -> bool {
        if let Some(work) = self.runtime_services.work.as_ref() {
            if !work.clear(self.current_session_id.as_deref()) {
                return false;
            }
            self.last_known_work_state = Some(None);
            return true;
        }
        // Acquire both stores before mutating either one. `/clear` must never
        // report success after clearing only half of the Work surface.
        let Some(mut todos) = Self::retry_lock(&self.todos, 100) else {
            return false;
        };
        let Some(mut plan) = Self::retry_lock(&self.plan_state, 100) else {
            return false;
        };
        todos.clear();
        *plan = PlanState::default();
        drop(plan);
        drop(todos);
        self.last_known_work_state = Some(None);
        true
    }

    /// Publish a validated Work Graph transaction after a synchronous caller
    /// has completed its atomic session write.
    pub fn publish_pending_work_state(&mut self) -> Result<bool, String> {
        let published = self
            .runtime_services
            .work
            .as_ref()
            .map_or(Ok(false), |work| work.publish_pending_sync())?;
        Ok(published)
    }

    pub fn update_model_compaction_budget(&mut self) {
        let model = self.effective_model_for_budget().to_string();
        self.compact_threshold = crate::route_budget::compaction_threshold_for_route_at_percent(
            self.api_provider,
            &model,
            self.active_route_limits,
            self.auto_compact_threshold_percent,
        );
        if !self.auto_compact_user_configured {
            self.auto_compact = crate::route_budget::auto_compact_default_for_route(
                self.api_provider,
                &model,
                self.active_route_limits,
            );
        }
    }

    pub fn set_active_route_limits(&mut self, limits: RouteLimits) {
        self.active_route_limits = crate::route_budget::known_route_limits(limits);
    }

    /// Install an already-resolved runtime route receipt in one operation so
    /// endpoint-sensitive reasoning and context reporting cannot drift apart.
    pub fn set_active_route_resolution(
        &mut self,
        base_url: impl Into<String>,
        limits: RouteLimits,
        context_window_source: crate::route_runtime::ContextWindowSource,
    ) {
        self.active_route_base_url = base_url.into();
        self.set_active_route_limits(limits);
        self.active_context_window_source = context_window_source;
    }

    pub fn set_active_context_window_override(&mut self, context_window: Option<u32>) {
        self.active_context_window_override = context_window;
        if context_window.is_some() {
            self.active_context_window_source =
                crate::route_runtime::ContextWindowSource::Configured;
        }
        if self.active_route_limits.is_none() {
            self.active_route_limits = self.context_window_override_limits();
        }
    }

    pub fn context_window_override_limits(&self) -> Option<RouteLimits> {
        self.active_context_window_override
            .map(|window| RouteLimits {
                context_tokens: Some(u64::from(window)),
                ..RouteLimits::default()
            })
    }

    pub fn set_model_selection(&mut self, model: String) {
        let auto_model = model.trim().eq_ignore_ascii_case("auto");
        self.model = if auto_model {
            "auto".to_string()
        } else {
            model
        };
        self.auto_model = auto_model;
        self.last_effective_model = None;
        self.last_effective_provider = None;
        self.last_effective_provider_identity = None;
        self.last_auto_route_receipt = None;
        self.pending_auto_route_receipt = None;
        self.last_effective_reasoning_effort = None;
        // Auto model routing is independent from an explicitly requested raw
        // reasoning tier. Never reuse the route-normalized live value here:
        // fixed DeepSeek can collapse low→high and Codex off→low.
        if auto_model {
            self.reasoning_effort = self
                .reasoning_effort_preference
                .unwrap_or(ReasoningEffort::Auto);
        } else {
            let requested = self
                .reasoning_effort_preference
                .unwrap_or(self.reasoning_effort);
            self.reasoning_effort = requested.normalize_for_provider(self.api_provider);
        }
    }

    pub fn model_selection_for_persistence(&self) -> String {
        if self.auto_model || self.model.trim().eq_ignore_ascii_case("auto") {
            "auto".to_string()
        } else {
            self.model.clone()
        }
    }

    /// Atomic latest Auto route metadata for session snapshots. The provider,
    /// exact identity, model, and receipt are either persisted together or
    /// omitted together so a resumed session cannot display a mixed route.
    #[must_use]
    pub(crate) fn auto_route_for_persistence(
        &self,
    ) -> Option<crate::session_manager::SavedAutoRouteReceipt> {
        if !self.auto_model {
            return None;
        }
        let (provider, model, receipt) = (
            self.last_effective_provider?,
            self.last_effective_model.as_ref()?,
            self.last_auto_route_receipt.as_ref()?,
        );
        if model.trim().is_empty() {
            return None;
        }
        let provider_identity = self
            .last_effective_provider_identity
            .clone()
            .unwrap_or_else(|| {
                if provider == ApiProvider::Custom {
                    self.provider_identity_for_persistence().to_string()
                } else {
                    provider.as_str().to_string()
                }
            });
        Some(crate::session_manager::SavedAutoRouteReceipt {
            provider,
            provider_identity,
            model: model.clone(),
            receipt: receipt.clone(),
            effective_reasoning_effort: self.last_effective_reasoning_effort.map(Into::into),
        })
    }

    #[must_use]
    pub(crate) fn provider_identity_for_persistence(&self) -> &str {
        if self.api_provider == ApiProvider::Custom {
            &self.provider_identity
        } else {
            self.api_provider.as_str()
        }
    }

    #[must_use]
    pub(crate) fn provider_id_for_persistence(&self) -> Option<&str> {
        self.provider_exact_id.as_deref()
    }

    pub(crate) fn set_provider_identity(
        &mut self,
        provider: ApiProvider,
        identity: impl Into<String>,
    ) {
        let identity = identity.into();
        self.api_provider = provider;
        self.provider_exact_id = (!(provider == ApiProvider::Custom
            && identity.eq_ignore_ascii_case(ApiProvider::Custom.as_str())))
        .then(|| identity.clone());
        self.provider_identity = identity;
    }

    pub(crate) fn set_provider_identity_record(
        &mut self,
        identity: crate::config::ProviderIdentity,
    ) {
        self.api_provider = identity.provider;
        self.provider_identity = identity.key;
        self.provider_exact_id = identity.exact_id;
    }

    pub fn accepts_custom_model_ids(&self) -> bool {
        self.model_ids_passthrough
            || crate::config::provider_passes_model_through(self.api_provider)
    }

    pub(crate) fn apply_provider_switch_reasoning_effort(
        &mut self,
        provider: ApiProvider,
        base_url: &str,
        model_override: Option<&str>,
    ) {
        let wire_model = model_override.unwrap_or(&self.model);
        let inferred = model_override.and_then(|model| {
            crate::config::legacy_deepseek_alias_effort_for_route(provider, base_url, model)
        });
        self.reasoning_effort = if let Some(requested) = self.reasoning_effort_preference {
            requested.normalize_for_route(provider, base_url, wire_model)
        } else if let Some(effort) = inferred {
            ReasoningEffort::from_setting(effort)
                .normalize_for_route(provider, base_url, wire_model)
        } else if let Some(default) = ReasoningEffort::catalog_default(provider, wire_model) {
            default
        } else {
            self.reasoning_effort
                .normalize_for_route(provider, base_url, wire_model)
        };
        self.invalidate_route_receipts_for_reasoning_change();
    }

    pub fn effective_model_for_budget(&self) -> &str {
        if self.auto_model {
            return self
                .last_effective_model
                .as_deref()
                .filter(|model| *model != "auto")
                .unwrap_or(DEFAULT_TEXT_MODEL);
        }
        &self.model
    }

    pub fn model_display_label(&self) -> String {
        if self.auto_model {
            if let Some(effective) = self.last_effective_model.as_deref()
                && effective != "auto"
            {
                return format!("auto: {effective}");
            }
            return "auto".to_string();
        }
        self.model.clone()
    }

    /// Provider/model identity used by the in-flight or most recent request.
    /// This is the display contract for auto routing and must match billing.
    #[must_use]
    pub fn effective_route_display(&self) -> (ApiProvider, String) {
        if let Some((provider, model, _)) = self.pending_turn_route.as_ref() {
            return (*provider, model.clone());
        }
        if self.auto_model
            && let (Some(provider), Some(model)) = (
                self.last_effective_provider,
                self.last_effective_model.as_ref(),
            )
        {
            return (provider, model.clone());
        }
        (self.api_provider, self.model_display_label())
    }

    /// Exact non-secret route label for user-visible status surfaces.
    #[must_use]
    pub fn effective_route_identity_display(&self) -> (String, String) {
        let (provider, model) = self.effective_route_display();
        let identity = if provider == ApiProvider::Custom {
            if self.pending_turn_route.is_none() && self.auto_model {
                self.last_effective_provider_identity
                    .as_deref()
                    .unwrap_or_else(|| self.provider_identity_for_persistence())
            } else {
                self.provider_identity_for_persistence()
            }
        } else {
            provider.display_name()
        };
        (identity.to_string(), model)
    }

    fn effective_reasoning_effort_for_active_route(
        &self,
        requested: ReasoningEffort,
    ) -> EffectiveReasoningEffort {
        let route_truth = self.active_reasoning_route_truth();
        let auto_route_has_receipt = self
            .active_turn
            .as_ref()
            .and_then(|turn| turn.route.as_ref())
            .is_some_and(|route| route.receipt.is_some());
        if self.auto_model
            && !auto_route_has_receipt
            && self.last_auto_route_receipt.is_some()
            && requested == self.reasoning_effort
            && let Some(effective) = self.last_effective_reasoning_effort
        {
            // Once a concrete Auto route has been accepted, its normalized
            // tier remains the display authority until the model or requested
            // effort changes. The configured classifier route is not evidence
            // of what the completed turn received.
            return effective;
        }
        if requested == self.reasoning_effort
            && requested == ReasoningEffort::Auto
            && let Some(effective) = self.last_effective_reasoning_effort
        {
            // The accepted route receipt is already the strongest available
            // truth. Preserve enabled-but-untiered and unavailable states
            // instead of forcing them through the tier-only projection.
            return effective;
        }
        let effective = if requested == ReasoningEffort::Auto {
            ReasoningEffort::Auto
        } else if self.auto_model && !auto_route_has_receipt {
            // The configured provider is only the classifier's starting
            // point, not the route that will receive the request.
            requested
        } else if let Some((provider, _, base_url, model)) = route_truth {
            requested.normalize_for_route(provider, base_url, model)
        } else {
            requested.normalize_for_route(
                self.api_provider,
                &self.active_route_base_url,
                &self.model,
            )
        };

        // Prefer the immutable installed-client receipt while a turn is live.
        // If it is unavailable, only use the configured route when no pending
        // or active foreign route could make that identity stale.
        if let Some((provider, _, base_url, model)) = route_truth {
            if let Some(constrained) = crate::work_graph::constrained_effective_reasoning_for_route(
                requested.into(),
                provider,
                base_url,
                model,
            ) {
                return constrained.into();
            }
        } else if self.active_turn.as_ref().is_some_and(|turn| {
            turn.route.as_ref().is_some_and(|route| {
                matches!(
                    route.provider,
                    ApiProvider::Zai
                        | ApiProvider::Minimax
                        | ApiProvider::MinimaxAnthropic
                        | ApiProvider::Custom
                ) && route.receipt.is_none()
            })
        }) || self
            .pending_turn_route
            .as_ref()
            .is_some_and(|(provider, _, _)| {
                matches!(
                    provider,
                    ApiProvider::Zai
                        | ApiProvider::Minimax
                        | ApiProvider::MinimaxAnthropic
                        | ApiProvider::Custom
                )
            })
        {
            // A route without its immutable endpoint receipt cannot prove
            // first-party semantics from provider/model identity alone.
            return EffectiveReasoningEffort::Unavailable;
        }
        EffectiveReasoningEffort::Tier(effective)
    }

    fn active_reasoning_route_truth(&self) -> Option<(ApiProvider, &str, &str, &str)> {
        if let Some(route) = self
            .active_turn
            .as_ref()
            .and_then(|turn| turn.route.as_ref())
        {
            route.receipt.as_ref().map(|receipt| {
                (
                    receipt.provider(),
                    receipt.provider_identity(),
                    receipt.endpoint_identity(),
                    receipt.wire_model(),
                )
            })
        } else if self.pending_turn_route.is_none() {
            Some((
                self.api_provider,
                self.provider_identity_for_persistence(),
                self.active_route_base_url.as_str(),
                self.model.as_str(),
            ))
        } else {
            None
        }
    }

    fn reasoning_effort_resolution_label(
        requested: ReasoningEffort,
        effective: EffectiveReasoningEffort,
        provider: ApiProvider,
    ) -> String {
        match effective {
            EffectiveReasoningEffort::Tier(effective) => {
                if requested == effective {
                    return effective.display_label_for_provider(provider).to_string();
                }
                let effective = effective.display_label_for_provider(provider);
                if requested == ReasoningEffort::Auto {
                    format!("auto: {effective}")
                } else {
                    format!("{}→{effective}", requested.short_label())
                }
            }
            EffectiveReasoningEffort::ThinkingEnabledGranularityUnavailable => format!(
                "{}→thinking enabled; granularity unavailable",
                requested.short_label()
            ),
            EffectiveReasoningEffort::Unavailable => {
                format!("{}→effective unavailable", requested.short_label())
            }
        }
    }

    pub fn reasoning_effort_display_label(&self) -> String {
        let requested = self.reasoning_effort;
        let effective = self.effective_reasoning_effort_for_active_route(requested);
        Self::reasoning_effort_resolution_label(requested, effective, self.api_provider)
    }

    /// Return the concrete provider/model route whose current prompt may be
    /// inspected or replayed.
    ///
    /// For a fixed selection, the active route is authoritative. For Auto,
    /// `self.model` is only the selector sentinel, so the latest completed
    /// turn supplies provider/model/endpoint truth. A restored Auto session
    /// retains provider/model but not a raw endpoint; warmup may re-resolve
    /// that route from live config, while inspect fails honestly until a new
    /// turn captures the endpoint.
    #[must_use]
    pub(crate) fn cache_replay_target(&self) -> Option<CacheReplayTarget> {
        if !self.auto_model {
            let model = self.model.trim();
            if model.is_empty() || model.eq_ignore_ascii_case("auto") {
                return None;
            }
            let base_url = (!self.active_route_base_url.trim().is_empty())
                .then(|| self.active_route_base_url.clone());
            return Some(CacheReplayTarget {
                provider: self.api_provider,
                provider_identity: self.provider_identity_for_persistence().to_string(),
                provider_id: self.provider_id_for_persistence().map(str::to_string),
                model: model.to_string(),
                base_url,
            });
        }

        let provider = self.last_effective_provider?;
        let model = self.last_effective_model.as_deref()?.trim();
        if model.is_empty() || model.eq_ignore_ascii_case("auto") {
            return None;
        }
        let provider_identity = self
            .last_effective_provider_identity
            .as_deref()
            .map(str::trim)
            .filter(|identity| !identity.is_empty())
            .map(str::to_string)
            .or_else(|| (provider != ApiProvider::Custom).then(|| provider.as_str().to_string()))?;
        let provider_id = if provider != ApiProvider::Custom {
            Some(provider.as_str().to_string())
        } else if !provider_identity.eq_ignore_ascii_case(ApiProvider::Custom.as_str()) {
            Some(provider_identity.clone())
        } else if self.api_provider == ApiProvider::Custom
            && self
                .provider_identity_for_persistence()
                .eq_ignore_ascii_case(&provider_identity)
        {
            self.provider_id_for_persistence().map(str::to_string)
        } else {
            None
        };

        let latest_matches_route = self
            .session
            .turn_cache_history
            .back()
            .is_some_and(|record| {
                record.auto_model
                    && record.provider == Some(provider)
                    && record
                        .model
                        .as_deref()
                        .is_some_and(|record_model| record_model.eq_ignore_ascii_case(model))
                    && record
                        .provider_identity
                        .as_deref()
                        .map(str::trim)
                        .filter(|identity| !identity.is_empty())
                        .map_or(provider != ApiProvider::Custom, |identity| {
                            identity == provider_identity
                        })
            });
        let warmup_base_url = self
            .session
            .last_warmup_key
            .as_ref()
            .filter(|key| {
                key.provider == provider_identity
                    && key.model.eq_ignore_ascii_case(model)
                    && !key.base_url.trim().is_empty()
            })
            .map(|key| key.base_url.clone());
        let base_url = latest_matches_route
            .then(|| self.session.last_base_url.clone())
            .flatten()
            .or(warmup_base_url)
            .filter(|base_url| !base_url.trim().is_empty());

        Some(CacheReplayTarget {
            provider,
            provider_identity,
            provider_id,
            model: model.to_string(),
            base_url,
        })
    }

    /// Provider-facing effort used when replaying the current prompt for cache
    /// inspection or warmup on one exact route.
    #[must_use]
    pub(crate) fn reasoning_effort_api_value_for_replay(
        &self,
        provider: ApiProvider,
        base_url: &str,
        model: &str,
    ) -> Option<&'static str> {
        let requested = if self.reasoning_effort == ReasoningEffort::Auto {
            self.last_effective_reasoning_effort?
                .request_tier_for_replay()?
        } else {
            self.reasoning_effort
        };
        requested.api_value_for_route(provider, base_url, model)
    }

    pub fn compaction_config(&self) -> CompactionConfig {
        let mut config = self.compaction_config_for_route(
            self.api_provider,
            self.effective_model_for_budget(),
            self.active_route_limits,
        );
        // These cached fields are the active-route compatibility authority and
        // are updated together by `update_model_compaction_budget`. Commands
        // and embedders may also adjust them directly between route updates.
        config.enabled = self.auto_compact;
        config.token_threshold = self.compact_threshold;
        config
    }

    /// Build compaction policy from one already-resolved provider route.
    ///
    /// Auto routing can select a provider/model whose context limits differ
    /// from the route currently displayed by the app. Callers dispatching that
    /// turn must derive every compaction input from the selected descriptor,
    /// not from the previous route cached in `App`.
    pub(crate) fn compaction_config_for_route(
        &self,
        provider: ApiProvider,
        model: &str,
        route_limits: Option<RouteLimits>,
    ) -> CompactionConfig {
        CompactionConfig {
            enabled: if self.auto_compact_user_configured {
                self.auto_compact
            } else {
                crate::route_budget::auto_compact_default_for_route(provider, model, route_limits)
            },
            token_threshold: crate::route_budget::compaction_threshold_for_route_at_percent(
                provider,
                model,
                route_limits,
                self.auto_compact_threshold_percent,
            ),
            model: model.to_string(),
            effective_context_window: Some(crate::route_budget::route_context_window_tokens(
                provider,
                model,
                route_limits,
            )),
            ..Default::default()
        }
    }

    pub fn fallback_chain_entries(&self) -> Vec<(usize, ApiProvider, bool)> {
        let Some(chain) = &self.provider_chain else {
            return Vec::new();
        };
        let position = chain.position();
        chain
            .providers()
            .iter()
            .enumerate()
            .map(|(index, provider)| (index, ApiProvider::from_kind(*provider), index == position))
            .collect()
    }

    pub fn fallback_chain_position(&self) -> Option<usize> {
        self.provider_chain.as_ref().map(ProviderChain::position)
    }

    pub fn fallback_chain_len(&self) -> usize {
        self.provider_chain
            .as_ref()
            .map_or(0, |chain| chain.providers().len())
    }

    /// Whether a fallback chain entry can serve a turn right now (#2574).
    ///
    /// Mirrors the provider picker's eligibility: hosted providers need a key
    /// (`has_api_key_for`, captured into `provider_readiness` at startup) while
    /// self-hosted providers (Ollama/vLLM/SGLang) are always ready. Providers
    /// absent from the snapshot default to ready so an unknown entry is tried
    /// rather than silently skipped.
    fn fallback_provider_is_ready(&self, provider: ApiProvider) -> bool {
        self.provider_readiness
            .iter()
            .find_map(|(candidate, ready)| (*candidate == provider).then_some(*ready))
            .unwrap_or(true)
    }

    /// Advance to the next *eligible* provider in the fallback chain (#2574).
    ///
    /// Walks the chain from the current position, skipping entries that are not
    /// ready (hosted providers missing auth) and recording a clear note for each
    /// skip. Local providers are always eligible. Returns the first ready
    /// provider, or `None` (with an exhaustion reason) when every remaining entry
    /// is unready or the end of the chain is reached. `ProviderChain::advance`
    /// stays pure — the readiness filtering lives here at the App level.
    ///
    /// Note: auth-rejection (401) failures never reach this path; the caller
    /// excludes them from fallback so a bad key does not silently rotate
    /// providers (see `apply_engine_error_to_app`).
    ///
    /// Local/private policy (#2574): when the chain's primary provider is a
    /// self-hosted / local runtime, cloud candidates are skipped with a clear
    /// note so a local/private route never silently falls back out to a hosted
    /// provider. Self-hosted siblings remain eligible. The policy is anchored
    /// to the original primary; a cloud primary may still hop through a local
    /// runtime and then back to another cloud fallback.
    pub fn advance_fallback(&mut self, reason: impl Into<String>) -> Option<ApiProvider> {
        let reason = reason.into();
        self.provider_chain.as_ref()?;

        let origin_is_local = self
            .provider_chain
            .as_ref()
            .and_then(|chain| chain.providers().first().copied())
            .map(ApiProvider::from_kind)
            .is_some_and(ApiProvider::is_self_hosted);

        let mut skip_notes: Vec<String> = Vec::new();
        let mut chosen: Option<ApiProvider> = None;
        while let Some(next_kind) = self
            .provider_chain
            .as_mut()
            .and_then(ProviderChain::advance)
        {
            let candidate = ApiProvider::from_kind(next_kind);
            if origin_is_local && !candidate.is_self_hosted() {
                skip_notes.push(format!(
                    "skipped {}: local/private policy (no local->cloud fallback)",
                    candidate.as_str()
                ));
                continue;
            }
            if self.fallback_provider_is_ready(candidate) {
                chosen = Some(candidate);
                break;
            }
            skip_notes.push(format!("skipped {}: needs auth", candidate.as_str()));
        }

        let skipped = if skip_notes.is_empty() {
            String::new()
        } else {
            format!(" ({})", skip_notes.join("; "))
        };

        let Some(next_provider) = chosen else {
            let total = self
                .provider_chain
                .as_ref()
                .map_or(0, |chain| chain.providers().len());
            self.last_fallback_reason = Some(format!(
                "Fallback chain exhausted after {total} provider(s): {reason}{skipped}"
            ));
            return None;
        };

        self.set_provider_identity(next_provider, next_provider.as_str());
        self.last_fallback_reason = Some(format!(
            "Fell back to {} after recoverable provider error: {reason}{skipped}",
            next_provider.as_str()
        ));
        Some(next_provider)
    }

    pub fn is_fallback_active(&self) -> bool {
        self.provider_chain
            .as_ref()
            .is_some_and(ProviderChain::is_fallback_active)
    }
}

pub fn media_attachment_reference(kind: &str, path: &Path, description: Option<&str>) -> String {
    match description {
        Some(description) if !description.trim().is_empty() => {
            format!(
                "[Attached {kind}: {} at {}]",
                description.trim(),
                path.display()
            )
        }
        _ => format!("[Attached {kind}: {}]", path.display()),
    }
}

#[cfg(test)]
mod tests;
