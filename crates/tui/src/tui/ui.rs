//! TUI event loop and rendering logic for `DeepSeek` CLI.

use std::collections::{HashSet, VecDeque};
use std::fmt::Write as _;
use std::future::Future;
use std::io::{self, IsTerminal, Stdout, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{
    Arc, LazyLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use crate::error_taxonomy::{ErrorCategory, ErrorEnvelope, ErrorSeverity};
use crate::resource_telemetry::{TokenThroughput, estimate_output_tokens_from_text};
use anyhow::{Context, Result};
use codewhale_release::InstallMethod;
// On Windows the push/pop helpers write the escapes directly; crossterm's
// PushKeyboardEnhancementFlags / PopKeyboardEnhancementFlags commands are
// never referenced, so the imports are gated to avoid -D warnings failures.
#[cfg(not(windows))]
use crossterm::event::{
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    layout::{Constraint, Direction, Layout, Rect, Size},
    prelude::Widget,
    style::Style,
    widgets::Block,
};
use tracing;
#[cfg(target_os = "windows")]
use windows::Win32::System::Console::{GetConsoleMode, GetStdHandle, SetConsoleMode};

use crate::audit::log_sensitive_event;
use crate::automation_manager::{AutomationManager, AutomationSchedulerConfig, spawn_scheduler};
use crate::client::{
    CACHE_WARMUP_MAX_TOKENS, CacheWarmupKey, DeepSeekClient, PromptInspection,
    build_cache_warmup_request, inspect_prompt_for_request,
};
use crate::commands;
use crate::compaction::CompactionConfig;
use crate::compaction::{estimate_input_tokens_conservative, estimate_tokens};
use crate::config::{
    ApiProvider, Config, ProviderConfig, ProviderIdentity, ProvidersConfig, StatusItem,
    UpdateConfig, persist_external_credential_consent_for_at,
    revoke_external_credential_consent_for_at,
};
use crate::config_ui::{self, ConfigUiMode, WebConfigSession, WebConfigSessionEvent};
use crate::core::engine::{EngineConfig, EngineHandle, spawn_engine};
use crate::core::events::Event as EngineEvent;
use crate::core::ops::{Op, ProviderRuntimeStatus, USER_SHELL_TOOL_ID_PREFIX, UserInputProvenance};
use crate::hooks::{HookEvent, HookExecutor, TurnEndPayloadInput, TurnEndTotals};
use crate::llm_client::LlmClient;
use crate::localization::{MessageId, tr};
use crate::models::{ContentBlock, Message, MessageRequest, SystemPrompt, Usage};
use crate::palette;
use crate::prompts;
use crate::route_runtime::{resolve_runtime_route, resolve_runtime_route_for_identity};
use crate::session_manager::{
    OfflineQueueState, QueuedSessionMessage, SavedSession, SessionManager,
    create_saved_session_with_id_and_mode, create_saved_session_with_mode,
};
use crate::settings::Settings;
use crate::task_manager::{
    NewTaskRequest, SharedTaskManager, TaskManager, TaskManagerConfig, TaskStatus, TaskSummary,
};
use crate::tools::goal::{GoalSnapshot, GoalStatus};
use crate::tools::shell::{ShellJobSnapshot, ShellStatus};
use crate::tools::spec::{RuntimeToolServices, ToolResult};
use crate::tools::subagent::{MailboxMessage, SubAgentStatus, subagent_progress_tool_display_name};
use crate::tui::auto_router;
use crate::tui::clipboard::ClipboardContent;
use crate::tui::color_compat::ColorCompatBackend;
use crate::tui::command_palette::{
    CommandPaletteView, build_entries_with_plugins as build_command_palette_entries,
};
use crate::tui::composer_ui::*;
use crate::tui::context_inspector::ContextInspectorView;
use crate::tui::event_broker::EventBroker;
use crate::tui::file_mention::ContextReference;
use crate::tui::file_picker_relevance;
use crate::tui::footer_ui::{friendly_subagent_progress, is_noisy_subagent_progress};
use crate::tui::format_helpers;
use crate::tui::hotbar::actions::HotbarDispatch;
use crate::tui::key_shortcuts;
use crate::tui::live_transcript::LiveTranscriptOverlay;
use crate::tui::mcp_routing::{add_mcp_message, open_mcp_manager_pager};
use crate::tui::mouse_ui::*;
use crate::tui::notifications;
use crate::tui::onboarding;
use crate::tui::pager::PagerView;
use crate::tui::persistence_actor::{self, PersistRequest};
use crate::tui::scrolling::TranscriptScroll;
use crate::turn_route_plan::{PlannedTurnRoute, TurnRoutePlanRequest, plan_turn_route};
use crate::work_graph::task_owner_snapshot;
// SelectionAutoscroll unused
use crate::tui::motion::{FrameRequester, MotionMode};
use crate::tui::session_picker::SessionPickerView;
use crate::tui::shell_job_routing::{
    add_shell_job_message, format_shell_job_list, format_shell_poll, open_shell_job_pager,
};
use crate::tui::streaming::StreamDisplayClock;
use crate::tui::streaming_thinking;
use crate::tui::subagent_routing::{
    apply_subagent_terminal_projection, format_task_list, handle_subagent_mailbox_for_turn,
    open_task_pager, parent_stop_status, reconcile_subagent_activity_state, running_agent_count,
    sort_subagents_in_place, subagent_message_refreshes_workspace_context, task_mode_label,
    task_summary_to_panel_entry,
};
#[cfg(test)]
use crate::tui::subagent_routing::{handle_subagent_mailbox, reconcile_subagent_activity_state_at};
#[cfg(test)]
use crate::tui::tool_routing::exploring_label;
use crate::tui::tool_routing::{
    apply_owned_workflow_ui_event, handle_tool_call_complete, handle_tool_call_started,
};
use crate::tui::ui_text::history_cell_to_text;
use crate::tui::user_input::UserInputView;
use crate::tui::views::subagent_view_agents;
use crate::tui::vim_mode;
use crate::tui::workspace_context;

use super::key_actions;

use super::app::{
    ActiveCompaction, ActiveTurnMetadata, AgentCurrentActivity, AgentCurrentActivityStatus, App,
    AppAction, AppMode, ComposerSubmitAction, ComposerSubmitChord, EffectiveReasoningEffort,
    GoalControlIntent, OnboardingState, PendingGoalControl, PendingProviderSwitch, QueuedMessage,
    ReasoningEffort, ScreenMode, StatusToast, StatusToastLevel, SubmitDisposition, TaskPanelEntry,
    TaskPanelEntryKind, ToolEvidence, TuiOptions, bound_agent_activity_text, is_stop_word,
    looks_like_slash_command_input, shell_command_from_bang_input,
};
use super::approval::{
    ApprovalMode, ApprovalRequest, ApprovalView, ElevationRequest, ElevationView, ReviewDecision,
};
use super::history::{
    ExecCell, HistoryCell, ReasoningAction, ToolCell, ToolStatus, history_cells_from_message,
    summarize_tool_output,
};
use super::slash_menu::{
    apply_slash_menu_selection, partial_inline_skill_mention_at_cursor,
    try_autocomplete_slash_command, visible_slash_menu_entries,
};
use super::views::{ConfigView, ContextMenuAction, HelpView, ModalKind, ViewEvent};
use super::widgets::pending_input_preview::{ContextPreviewItem, PendingInputPreview};
use super::widgets::{ChatWidget, ComposerWidget, Renderable};

// Activity Detail / raw-detail / pager-text helpers extracted into `activity_detail`
// (issue #4103). Re-export the cross-module entry points so existing
// `crate::tui::ui::{...}` importers (mouse_ui, footer_ui) keep resolving, and
// import the ui-internal entry points used from this file's own body.
pub(crate) use self::activity_detail::{
    completed_assistant_answer_text, copy_cell_to_clipboard, detail_target_label,
    open_details_pager_for_cell, open_focused_cell_pager, turn_handoff_markdown,
};
use self::activity_detail::{
    copy_focused_cell, copy_focused_cell_metadata, detail_target_cell_index,
    extract_reasoning_header, open_reasoning_detail_pager, open_tool_details_pager,
    open_turn_inspector_pager,
};
// Ctrl+O now opens the full recorded Reasoning Detail for the selected or
// current reasoning block. The whole-turn Turn Inspector moved to Ctrl+Alt+O
// and `/turn inspect`. (`v` raw leaf detail keeps using `open_tool_details_pager`.)

// === Constants ===

/// Upper bound on slash-menu entries returned to the renderer. The composer's
/// render path already paginates with center-tracking (see
/// `widgets::ComposerWidget::render`), so this only needs to be high enough to
/// encompass the full filtered command list — never the visible-row budget.
/// Bumped from 6 to 128 to fix #64 (selection couldn't reach commands beyond
/// the visible window because the source list itself was capped).
const SLASH_MENU_LIMIT: usize = 128;
const MIN_CHAT_HEIGHT: u16 = 3;
const MIN_COMPOSER_HEIGHT: u16 = 2;
const CONTEXT_WARNING_THRESHOLD_PERCENT: f64 = 85.0;
const CONTEXT_CRITICAL_THRESHOLD_PERCENT: f64 = 95.0;
const CONTEXT_SUGGEST_COMPACT_THRESHOLD_PERCENT: f64 = 60.0;
const UI_IDLE_POLL_MS: u64 = 48;
const UI_ACTIVE_POLL_MS: u64 = 24;
const SUBAGENT_HOOK_PREVIEW_LIMIT: usize = 2_048;
const WEB_CONFIG_POLL_MS: u64 = 16;
const DISPATCH_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(30);
/// Minimum wall-clock time a turn may stay in `"in_progress"` before the UI
/// assumes the engine stalled (e.g. sub-agent hang, lost completion event,
/// engine panic).  The effective watchdog also respects the configured stream
/// idle timeout so legitimate long model-reasoning pauses are not interrupted
/// prematurely.
const TURN_STALL_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(300);
const TURN_STALL_WATCHDOG_GRACE: Duration = Duration::from_secs(30);
/// Running tools can legitimately exceed the silent-turn timeout, but a tool
/// with no progress heartbeat or output beyond this ceiling is treated as hung.
// Must stay comfortably above `turn_stall_watchdog_timeout` so a running tool
// gets extra grace beyond the turn-stall threshold (#1862 trimmed 15m → 10m).
const TOOL_HANG_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(600);
// Forced repaint cadence while a turn is live (model loading, compacting,
// sub-agents running). Drives the footer water-spout animation as well as
// the per-tool spinner pulse — keep this fast enough that the whale-spout
// braille pattern reads as continuous motion instead of teleport-frames.
const UI_STATUS_ANIMATION_MS: u64 = crate::tui::spinner::BRAILLE_SPINNER_FRAME_MS;
/// Ambient fish, the idle-mark caustic, and the completion wake use a modest
/// ~12.5fps clock by default. On measured high-Hz displays the adaptive probe
/// may raise this (still bounded); low_motion always freezes the cadence.
/// Active markers run at 8fps; atmosphere stays subordinate.
pub(crate) const UI_UNDERWATER_ANIMATION_MS: u64 = 80;
/// Full-motion compatibility cadence for VTE, tmux, and other terminals that
/// explicitly request the 30 FPS safety cap.
pub(crate) const UI_CONSTRAINED_UNDERWATER_ANIMATION_MS: u64 = 34;
/// 30 FPS Ghostty atmosphere clock. Input, streaming, and other interactive
/// state still request immediate frames up to the separate 60 FPS draw cap;
/// idle water no longer forces a full-screen repaint at that rate.
pub(crate) const UI_GHOSTTY_UNDERWATER_ANIMATION_MS: u64 = 34;
// Minimum chat-host width at which the file-tree pane renders. At an
// 80-column terminal the file tree owns 20 columns, leaving a 60-column chat
// host; below this floor the tree is hidden rather than squeezing the
// transcript under 40 columns. (Named for the file tree — the legacy sidebar
// this constant once described no longer gates on it.)
pub(crate) const FILE_TREE_MIN_HOST_WIDTH: u16 = 60;
const DEFAULT_TERMINAL_PROBE_TIMEOUT_MS: u64 = 500;
const TURN_META_PREFIX: &str = "<turn_meta>";
const SESSION_TITLE_MAX_CHARS: usize = 32;
const VERSION_HINT_TOAST_TTL_MS: u64 = 12_000;

const REQUIRED_RELEASE_ASSETS: &[&str] = &[
    "codewhale-linux-x64",
    "codew-linux-x64",
    "codewhale-linux-arm64",
    "codew-linux-arm64",
    "codewhale-android-arm64",
    "codew-android-arm64",
    "codewhale-macos-x64",
    "codew-macos-x64",
    "codewhale-macos-arm64",
    "codew-macos-arm64",
    "codewhale-windows-x64.exe",
    "codew-windows-x64.exe",
    "codewhale.bat",
    "codewhale-windows-arm64.exe",
    "codew-windows-arm64.exe",
    "codewhale-linux-x64.tar.gz",
    "codewhale-linux-arm64.tar.gz",
    "codewhale-android-arm64.tar.gz",
    "codewhale-macos-x64.tar.gz",
    "codewhale-macos-arm64.tar.gz",
    "codewhale-windows-x64.zip",
    "codewhale-windows-x64-portable.zip",
    "codewhale-windows-arm64.zip",
    "codewhale-windows-arm64-portable.zip",
    "CodeWhaleSetup.exe",
    "codewhale-bundles-sha256.txt",
    "codewhale-artifacts-sha256.txt",
];

type AppTerminal = Terminal<ColorCompatBackend<Stdout>>;

type PendingToolUses = Vec<(String, String, serde_json::Value)>;

#[derive(Debug)]
enum TranslationEvent {
    AssistantMessage {
        history_index: Option<usize>,
        original_text: String,
        translated: anyhow::Result<String>,
        thinking: Option<String>,
        tool_uses: PendingToolUses,
    },
    Thinking {
        placeholder: String,
        translated: anyhow::Result<String>,
    },
}

// Reset scroll region (`\x1b[r`), origin mode (`\x1b[?6l`), and home the cursor
// (`\x1b[H`) before letting ratatui's diff renderer repaint. The destructive
// `\x1b[2J\x1b[3J` pair was previously appended here to also wipe the visible
// screen and saved scrollback, but combined with the immediately-following
// `terminal.clear()` it produced a double-clear that several terminals
// (Ghostty, VSCode terminal, Win10 conhost) render as visible flicker on every
// TurnComplete / focus-gain / resize. The alt-screen buffer's double-buffering
// plus ratatui's `terminal.clear()` are sufficient to repaint cleanly.
const TERMINAL_ORIGIN_RESET: &[u8] = b"\x1b[r\x1b[?6l\x1b[H";
// Xterm alternate-scroll mode (DECSET 1007) converts wheel input into arrow
// keys. It is only meaningful when mouse reporting is unavailable; while
// mouse capture is active the terminal must deliver wheel events as mouse
// events, so 1007 stays off (iTerm2 converts anyway, breaking transcript
// wheel-scroll — #5223). `--no-mouse-capture` also keeps it off so the host
// terminal owns raw mouse selection behavior end-to-end (#4026).
const ENABLE_ALT_SCROLL_MODE: &[u8] = b"\x1b[?1007h";
const DISABLE_ALT_SCROLL_MODE: &[u8] = b"\x1b[?1007l";
/// Begin synchronized update (DEC 2026): tell the terminal to defer
/// rendering until END_SYNC_UPDATE is received. Best-effort —
/// terminals that don't support this silently ignore the sequence.
/// Reduces flicker on GPU-accelerated terminals (Ghostty, VSCode
/// Terminal, Kitty, WezTerm) by batching ratatui's incremental
/// diff writes into a single frame.
const BEGIN_SYNC_UPDATE: &[u8] = b"\x1b[?2026h";
/// End synchronized update (DEC 2026): tell the terminal to render
/// the complete frame now.
const END_SYNC_UPDATE: &[u8] = b"\x1b[?2026l";
/// Throttled in-progress checkpoint while a turn is live (#1830 progress loss).
const RECOVERY_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(45);

/// Where a key goes while onboarding owns the screen (#4763).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OnboardingKeyRoute {
    /// Terminate the session. Ctrl+C is unconditional during onboarding.
    Quit,
    /// Hand the key to the provider picker on the view stack.
    ProviderPicker,
    /// Take the advertised offline exit (#3927). Reachable from Provider
    /// setup even while the provider picker owns the screen, so the choice is
    /// never hidden behind a modal the user cannot satisfy.
    ExploreOffline,
    /// Fall through to the legacy onboarding key switch.
    Legacy,
}

fn surface_prompt_override_notices(app: &mut App) {
    for notice in prompts::take_prompt_override_notices() {
        app.add_message(HistoryCell::System {
            content: format!("Warning: {notice}"),
        });
        app.push_status_toast(notice, StatusToastLevel::Warning, Some(12_000));
    }
}

#[cfg(test)]
#[test]
fn tui_launch_preflight_explains_non_tty_failure() {
    assert!(require_interactive_terminal(true, true).is_ok());
    for (stdin_is_tty, stdout_is_tty) in [(false, true), (true, false), (false, false)] {
        let err = require_interactive_terminal(stdin_is_tty, stdout_is_tty)
            .expect_err("a missing TTY must fail before raw mode");
        let message = err.to_string();
        assert!(message.contains("interactive terminal"), "{message}");
        assert!(message.contains("codewhale exec"), "{message}");
    }
}

#[cfg(unix)]
#[test]
fn tui_launch_preflight_rejects_background_process_group() {
    assert!(validate_foreground_process_group(41, 41).is_ok());
    let err = validate_foreground_process_group(41, 42)
        .expect_err("a background process group must fail before raw mode");
    let message = err.to_string();
    assert!(message.contains("background or suspended"), "{message}");
    assert!(message.contains("Run `fg`"), "{message}");
    assert!(message.contains("codewhale exec"), "{message}");
}

fn should_show_resume_hint(session_id: Option<&str>) -> bool {
    session_id.is_some_and(|id| !id.trim().is_empty())
}

fn resume_hint_text() -> &'static str {
    "To continue this session, execute codewhale run --continue"
}

struct TerminalCleanupGuard {
    use_bracketed_paste: bool,
    defused: bool,
}

impl Drop for TerminalCleanupGuard {
    fn drop(&mut self) {
        if self.defused {
            return;
        }

        let mut stdout = io::stdout();
        pop_keyboard_enhancement_flags(&mut stdout);
        disable_alternate_scroll_mode(&mut stdout);
        let _ = execute!(stdout, DisableFocusChange);
        let _ = disable_raw_mode();
        // The live screen, not the one startup chose: `/inline` moves it.
        if live_alt_screen() {
            let _ = leave_alt_screen(&mut stdout);
        }
        // Capture follows the screen at runtime too; disabling it when it was
        // never on is harmless (the emergency path already does).
        let _ = execute!(stdout, DisableMouseCapture);
        if self.use_bracketed_paste {
            disable_bracketed_paste_mode(&mut stdout);
        }
        let _ = execute!(stdout, crossterm::cursor::Show);
    }
}

/// Recognise composer input that is a `# foo` memory quick-add (#492).
///
/// Returns `true` for inputs that:
/// - start with `#`,
/// - have at least one non-whitespace character after the leading `#`,
/// - are a single line (no embedded `\n`), and
/// - are not a shebang (`#!`) or Markdown heading (`## …`, `### …`).
///
/// Multi-`#` prefixes are deliberately rejected so users can paste
/// Markdown headings into the composer without triggering the quick-add.
#[must_use]
fn is_memory_quick_add(input: &str) -> bool {
    let trimmed = input.trim_start();
    if !trimmed.starts_with('#') {
        return false;
    }
    if trimmed.starts_with("##") || trimmed.starts_with("#!") {
        return false;
    }
    if input.contains('\n') {
        return false;
    }
    // Require something after the `#`.
    !trimmed.trim_start_matches('#').trim().is_empty()
}

fn should_intercept_memory_quick_add(config: &Config, input: &str) -> bool {
    config.memory_enabled() && is_memory_quick_add(input)
}

#[cfg(test)]
mod memory_quick_add_tests {
    use super::should_intercept_memory_quick_add;
    use crate::config::Config;

    #[test]
    fn memory_quick_add_interception_requires_memory_opt_in() {
        let enabled: Config = toml::from_str(
            r#"
            [memory]
            enabled = true
            "#,
        )
        .expect("parse enabled memory config");
        assert!(should_intercept_memory_quick_add(
            &enabled,
            "# remember this"
        ));

        let disabled: Config = Config::default();
        assert!(!should_intercept_memory_quick_add(
            &disabled,
            "# remember this"
        ));
        assert!(!should_intercept_memory_quick_add(
            &enabled,
            "## Markdown heading"
        ));
    }
}

fn spawn_tui_engine(config: EngineConfig, api_config: &Config) -> EngineHandle {
    let handle = spawn_engine(config, api_config);
    // Prime durable agent + coordination state through the same engine event
    // used by later refreshes. All TUI engine replacements use this wrapper,
    // so workspace switches and provider recovery cannot retain stale Work.
    let _ = handle.try_send(Op::ListSubAgents);
    handle
}

fn configured_instruction_sources(config: &Config) -> Vec<prompts::InstructionSource> {
    config
        .instructions_paths()
        .into_iter()
        .map(Into::into)
        .collect()
}

/// Open the exact effective base-prompt preview (#3928).
///
/// Assembles the prompt through [`build_app_system_prompt_with_goal`] — the same
/// function the dispatch path calls — so the preview is the next turn's bytes,
/// not a reconstruction of them. Nothing is sent and no tool catalog is
/// expanded; the preview is a pure read.
fn preview_effective_base_prompt(app: &mut App, config: &Config) {
    use crate::prompts::base_preview;

    let prompt = build_app_system_prompt_with_goal(app, config, app.goal.objective.as_deref());
    let home = codewhale_config::codewhale_home().ok();
    let constitution_path = codewhale_config::UserConstitution::path().ok();
    let sources = base_preview::PreviewSources {
        base_prompt: Some(crate::prompts::effective_base_prompt_source(
            home.as_deref(),
        )),
        user_constitution_path: constitution_path.as_deref(),
        workspace: Some(app.workspace.as_path()),
        home: home.as_deref(),
    };
    let report = base_preview::render_report(&base_preview::preview(&prompt, &sources));
    let width = app
        .viewport
        .last_transcript_area
        .map(|area| area.width)
        .unwrap_or(80);
    app.view_stack.push(crate::tui::pager::PagerView::from_text(
        crate::prompts::base_preview::PREVIEW_TITLE,
        &report,
        width.saturating_sub(2),
    ));
}

/// Minimum interval between balance API fetches to avoid flooding.
const BALANCE_FETCH_COOLDOWN: Duration = Duration::from_secs(60);

/// Shared `reqwest::Client` for balance fetches so connection pools are
/// reused across successive background polls.
static BALANCE_CLIENT: LazyLock<::reqwest::Client> = LazyLock::new(|| {
    crate::tls::reqwest_client_builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default()
});

#[derive(Debug)]
pub(crate) struct CacheWarmupOutcome {
    usage: Usage,
    provider_identity: String,
    model: String,
    base_url: String,
    inspection: PromptInspection,
}

/// Install a completed constitution draft into the setup wizard (if still on
/// top) and open its ratification preview, or surface a failure. Called from
/// the event loop when the background draft lands, and directly on the
/// pre-spawn provider-construction failure.
fn deliver_constitution_draft_result(
    app: &mut App,
    model_label: String,
    locale: crate::localization::Locale,
    outcome: Result<Box<codewhale_config::UserConstitution>, String>,
) {
    match outcome {
        Ok(constitution) => {
            if app.view_stack.top_kind() == Some(ModalKind::SetupWizard)
                && let Some(mut boxed) = app.view_stack.pop()
            {
                let preview = boxed
                    .as_any_mut()
                    .downcast_mut::<crate::tui::setup::SetupWizardView>()
                    .map(|wizard| wizard.install_model_draft(constitution, model_label.clone()));
                app.view_stack.push_boxed(boxed);
                if let Some((title, content)) = preview {
                    open_text_pager(app, title, content);
                    app.status_message = Some(crate::tui::setup::model_draft_ready_message(
                        locale,
                        &model_label,
                    ));
                }
            }
        }
        Err(reason) => {
            app.status_message = Some(crate::tui::setup::model_draft_failed_message(
                locale,
                &model_label,
                &reason,
            ));
        }
    }
    app.needs_redraw = true;
}

/// Install a completed fleet-profile draft into the wizard (if it is still on
/// top), or surface a failure. Called from the event loop when the
/// background draft lands, and directly on the pre-spawn
/// provider-construction failure.
///
/// The preview renders inline on the wizard's own Review step — deliberately
/// NOT in a separate pager (#4093): a standalone pager view owns its own
/// `g`/`G` scroll bindings and would swallow the ratify keypress, forcing an
/// Esc-then-g round trip before the user could actually save.
fn deliver_fleet_draft_result(
    app: &mut App,
    model_label: String,
    picked_route: Option<(String, String)>,
    reasoning_effort: Option<String>,
    outcome: Result<Box<crate::fleet::profile::FleetProfileDraft>, String>,
    locale: crate::localization::Locale,
) {
    match outcome {
        Ok(draft) => {
            if app.view_stack.top_kind() == Some(ModalKind::FleetSetup)
                && let Some(mut boxed) = app.view_stack.pop()
            {
                let installed = boxed
                    .as_any_mut()
                    .downcast_mut::<crate::tui::views::fleet_setup::FleetSetupView>()
                    .map(|wizard| {
                        wizard.install_model_draft(
                            draft,
                            model_label.clone(),
                            picked_route.clone(),
                            reasoning_effort.clone(),
                        )
                    })
                    .is_some();
                app.view_stack.push_boxed(boxed);
                if installed {
                    app.status_message = Some(match locale {
                        crate::localization::Locale::ZhHans => {
                            format!("{model_label} 已起草配置。请查看下方 TOML，然后按 g 保存。")
                        }
                        _ => format!(
                            "{model_label} drafted the profile. Review the TOML below, then press g to save."
                        ),
                    });
                }
            }
        }
        Err(reason) => {
            app.status_message = Some(match locale {
                crate::localization::Locale::ZhHans => {
                    format!("{model_label} 未能起草配置（{reason}）。按 Enter 仍会插入编写提示。")
                }
                _ => format!(
                    "{model_label} could not draft the profile ({reason}). Enter still inserts the authoring prompt."
                ),
            });
        }
    }
    app.needs_redraw = true;
}

// `format_*` chip/message builders moved to `tui/format_helpers.rs`.

fn is_work_graph_mutation_tool(name: &str) -> bool {
    matches!(
        name,
        "update_plan"
            | "work_update"
            | "checklist_write"
            | "todo_write"
            | "checklist_add"
            | "todo_add"
            | "checklist_update"
            | "todo_update"
            | "task_create"
            | "task_cancel"
            // Unified durable-task tool (piagent phase B): covers the
            // create/cancel actions the legacy names above carried.
            | "tasks"
            | "exec_shell"
            | "exec_shell_wait"
            | "exec_shell_cancel"
            | "agent"
            | "workflow"
    )
}

fn turn_stall_watchdog_timeout(app: &App) -> Duration {
    let stream_budget = Duration::from_secs(app.stream_chunk_timeout_secs)
        .saturating_add(TURN_STALL_WATCHDOG_GRACE);
    TURN_STALL_WATCHDOG_TIMEOUT.max(stream_budget)
}

fn active_turn_has_running_tool(app: &App) -> bool {
    app.active_cell.as_ref().is_some_and(|active| {
        active.entries().iter().any(|cell| match cell {
            HistoryCell::Tool(tool) => tool.is_running(),
            _ => false,
        })
    })
}

// Per-turn notification composition (settings, message body, summary)
// moved to `tui/notifications.rs` alongside the dispatch primitives.

async fn tool_result_content_for_api_message(
    app: &App,
    id: &str,
    name: &str,
    output: &ToolResult,
) -> String {
    let raw = output.content.trim();
    if raw.is_empty() {
        return String::new();
    }

    if matches!(
        name,
        "run_tests" | "run_verifiers" | "task_gate_run" | "tasks"
    ) {
        return crate::core::engine::compact_tool_result_for_route(
            app.api_provider,
            &app.model,
            app.active_route_limits,
            name,
            output,
        );
    }

    if raw.chars().count() > crate::tool_output_receipts::RAW_TOOL_OUTPUT_RECEIPT_THRESHOLD_CHARS {
        let messages = live_tool_receipt_messages(app, id, raw, output.success);
        let artifacts = app.session_artifacts.clone();
        let raw = raw.to_string();
        match tokio::task::spawn_blocking(move || {
            compact_live_tool_receipt(messages, artifacts, raw)
        })
        .await
        {
            Ok(Some(receipt)) => return receipt,
            Ok(None) => {}
            Err(err) => {
                crate::logging::warn(format!("live tool-output receipt compaction failed: {err}"));
            }
        }
    }

    crate::core::engine::compact_tool_result_for_route(
        app.api_provider,
        &app.model,
        app.active_route_limits,
        name,
        output,
    )
}

// Streaming-thinking lifecycle helpers moved to `tui/streaming_thinking.rs`.

/// Data produced by the async dispatch phase that is needed to apply the
/// post-acceptance mutations to `App`.
#[derive(Debug, Clone)]
pub(crate) struct UserDispatchOutcome {
    turn_compaction: CompactionConfig,
    effective_provider: ApiProvider,
    effective_model: String,
    effective_provider_identity: String,
    effective_provider_label: String,
    effective_reasoning_effort: EffectiveReasoningEffort,
    auto_selection: Option<crate::model_routing::AutoRouteSelection>,
}

fn is_model_visible_tool_call(id: &str) -> bool {
    !id.starts_with(USER_SHELL_TOOL_ID_PREFIX)
}

/// Tell the operator that an explicit "make this my default" request did not
/// take effect, instead of leaving a normal apply summary that reads like
/// success. Silence here is what made the sticky-default bug so confusing.
fn note_startup_default_not_saved(app: &mut App, save_as_startup_default: bool) {
    if !save_as_startup_default {
        return;
    }
    let existing = app.status_message.take();
    let note = "Startup default unchanged — the route was not applied.";
    app.status_message = Some(match existing {
        Some(message) if !message.trim().is_empty() => format!("{message} · {note}"),
        _ => note.to_string(),
    });
}

/// Route every Fleet-setup entry point to the storage surface that actually
/// controls the effective roster. A selected v2 Fleet always opens its exact
/// named editor; the legacy profile wizard is reachable only with no selected
/// Fleet. Selection resolution deliberately does not consult project trust.
fn open_fleet_setup_target(app: &mut App, config: &Config, member_id: Option<&str>) {
    use crate::tui::views::fleet_setup::{FleetSetupEditTarget, resolve_fleet_setup_edit_target};

    match resolve_fleet_setup_edit_target(&app.workspace) {
        Ok(FleetSetupEditTarget::SelectedFleet { name, scope }) => {
            if app.view_stack.top_kind() == Some(ModalKind::FleetDetail) {
                return;
            }
            let Some(view) = crate::tui::views::fleet_detail::FleetDetailView::open_for_member(
                app, config, &name, scope, member_id,
            ) else {
                app.set_sticky_status(
                    "Selected Fleet is invalid or unreadable; open /fleet fleets to repair or clear the selection. Legacy profiles were not opened."
                        .to_string(),
                    StatusToastLevel::Error,
                    None,
                );
                return;
            };
            let fleet_name = crate::safe_label::SafeLabel::phrase(&name);
            app.view_stack.push(view);
            app.status_message = Some(format!(
                "Editing selected Fleet `{fleet_name}` ({}) — legacy profiles will not be changed.",
                scope.label()
            ));
        }
        Ok(FleetSetupEditTarget::LegacyProfiles) => {
            if app.view_stack.top_kind() == Some(ModalKind::FleetSetup) {
                return;
            }
            let _ = app.next_draft_gen();
            let view = match member_id {
                Some(member_id) => crate::tui::views::fleet_setup::FleetSetupView::new_for_role(
                    app, config, member_id,
                ),
                None => crate::tui::views::fleet_setup::FleetSetupView::new(app, config),
            };
            app.view_stack.push(view);
        }
        Err(message) => {
            app.set_sticky_status(message, StatusToastLevel::Error, None);
        }
    }
}

fn open_fleet_model_target(app: &mut App, config: &Config, member_id: &str) {
    use crate::tui::views::fleet_setup::{FleetSetupEditTarget, resolve_fleet_setup_edit_target};

    match resolve_fleet_setup_edit_target(&app.workspace) {
        Ok(FleetSetupEditTarget::SelectedFleet { name, scope }) => {
            if app.view_stack.top_kind() == Some(ModalKind::FleetDetail) {
                return;
            }
            let Some(mut view) = crate::tui::views::fleet_detail::FleetDetailView::open_for_member(
                app,
                config,
                &name,
                scope,
                Some(member_id),
            ) else {
                app.set_sticky_status(
                    "Selected Fleet is invalid or unreadable; open /fleet fleets to repair or clear the selection."
                        .to_string(),
                    StatusToastLevel::Error,
                    None,
                );
                return;
            };
            view.open_model_picker();
            app.view_stack.push(view);
            let fleet_name = crate::safe_label::SafeLabel::phrase(&name);
            app.status_message = Some(format!(
                "Editing member `{member_id}` in Fleet `{fleet_name}` — choose a model route.",
            ));
        }
        Ok(FleetSetupEditTarget::LegacyProfiles) => {
            open_fleet_setup_target(app, config, Some(member_id));
        }
        Err(message) => app.set_sticky_status(message, StatusToastLevel::Error, None),
    }
}

pub(crate) struct ProviderFallbackRollback {
    identity: ProviderIdentity,
    chain: Option<codewhale_config::ProviderChain>,
}

// File-picker relevance scoring moved to `tui/file_picker_relevance.rs`.

#[cfg(test)]
use std::process::{Command, Stdio};

// `ui.rs` had grown past 19k lines. These three modules hold the same code,
// moved verbatim, and are re-exported so every existing path still resolves.
mod apply;
mod approval_routing;
use approval_routing::*;
mod event_loop;
mod handlers;

pub(crate) use apply::*;
pub(crate) use event_loop::*;
pub(crate) use handlers::*;
// The crate-wide glob would otherwise narrow this to `pub(crate)`; `tui/mod.rs`
// re-exports it as the binary's entry point.
pub use event_loop::run_tui;

mod compaction_flow;
pub(crate) use compaction_flow::*;
pub(crate) use provider_setup::*;
mod dispatch;
mod dispatch_prepare;
pub(crate) use dispatch_prepare::*;
pub(crate) mod fatal_signal_guard;
mod motion;
mod observer_hooks;
mod provider_setup;
mod release_check;
mod remote_control_bridge;
mod task_projection;
mod terminal;
mod terminal_input;
use remote_control_bridge::*;
use terminal_input::*;

pub(crate) use dispatch::*;
pub(crate) use motion::*;
pub(crate) use release_check::*;
pub(crate) use terminal::*;

// `frame` is `pub(crate)` so sibling modules (e.g. the widgets ASCII-safety
// test) can reach the topbar builders that project `App` state.
pub(crate) mod frame;
mod overlays;
mod provider_routes;
mod session_state;

pub(crate) use frame::*;
pub(crate) use overlays::*;
pub(crate) use provider_routes::*;
pub(crate) use session_state::*;

#[cfg(test)]
fn spawn_external_url_command(mut command: Command) -> Result<()> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|err| anyhow::anyhow!("failed to launch browser command: {err}"))
}

async fn execute_command_input(
    terminal: &mut AppTerminal,
    app: &mut App,
    engine_handle: &mut EngineHandle,
    task_manager: &SharedTaskManager,
    config: &mut Config,
    web_config_session: &mut Option<WebConfigSession>,
    input: &str,
) -> Result<bool> {
    let _ = app.note_manual_command_for_tip(input);
    if let Some(parsed_index) = parse_queue_send_command(input) {
        match parsed_index {
            Ok(index) => {
                send_queued_message_at_index_now(app, config, engine_handle, index).await?;
            }
            Err(message) => {
                app.status_message = Some(message);
            }
        }
        return Ok(false);
    }

    let result = commands::execute(input, app);
    // After /logout: clear the in-memory api_key fields so the next
    // onboarding round entering a new key doesn't see the stale value
    // (#343). The on-disk side is handled by clear_api_key() inside
    // commands::config::logout.
    if input.trim().eq_ignore_ascii_case("/logout") {
        // Only clear the active provider's in-memory API key, not every
        // provider.  The on-disk clear_api_key() inside commands::config::logout
        // already removes all saved keys; clearing only the active slot here
        // prevents surprising side-effects when the user has multiple providers
        // configured.
        clear_active_provider_api_key_from_memory(app, config);
        app.api_key_env_only = crate::config::active_provider_uses_env_only_api_key(config);
    }
    apply_command_result(
        terminal,
        app,
        engine_handle,
        task_manager,
        config,
        web_config_session,
        result,
    )
    .await
}

#[derive(Debug, Clone)]
pub(crate) struct SteerPausedSnapshot {
    paused: bool,
    pausable: bool,
    paused_goal_objective: Option<String>,
    objective: Option<String>,
    tokens_used: u64,
    time_used_seconds: u64,
    continuation_count: u32,
}

fn use_bundled_constitution(app: &mut App, config: &Config) {
    let mut state = crate::tui::setup::load_setup_state_for_app(app, config);
    state.complete_constitution_checkpoint(
        crate::tui::setup::CONSTITUTION_CHECKPOINT_VERSION,
        codewhale_config::ConstitutionChoice::Bundled,
    );
    state.constitution_source = codewhale_config::ConstitutionSource::Bundled;
    state.constitution_validity = codewhale_config::ConstitutionValidity::Unknown;
    state.constitution_preview_hash = None;
    state.set_step(
        codewhale_config::SetupStep::Constitution,
        codewhale_config::StepEntry::new(
            codewhale_config::StepStatus::Verified,
            true,
            crate::tui::setup::CONSTITUTION_CHECKPOINT_VERSION,
        )
        .with_result("bundled/default constitution"),
    );

    match state.save() {
        Ok(()) => {
            app.status_message = Some(
                "Using the bundled/default constitution; custom user-global law is inactive."
                    .to_string(),
            );
        }
        Err(err) => {
            app.status_message = Some(format!("Failed to save constitution choice: {err}"));
            app.add_message(HistoryCell::System {
                content: format!("Failed to save constitution choice: {err}"),
            });
        }
    }
    app.needs_redraw = true;
}

fn prepare_config_update_result(
    mut result: commands::CommandResult,
    persist: bool,
) -> commands::CommandResult {
    // Live previews can fire on every navigation tick. Suppress routine
    // confirmations, but preserve errors and AppAction so one canonical path
    // remains responsible for both user-visible output and side effects.
    if !persist && !result.is_error {
        result.message = None;
    }
    result
}

pub(crate) struct ApprovalDecisionEvent {
    tool_id: String,
    tool_name: String,
    decision: ReviewDecision,
    timed_out: bool,
    approval_key: String,
    approval_grouping_key: String,
    persistent_rules: Vec<codewhale_config::ToolAskRule>,
}

fn mark_active_turn_cancelled_locally(app: &mut App) {
    // #2739: every local cancel surface (Esc, Ctrl+C, approval abort, paused
    // command abort) must snapshot before it clears turn state. Otherwise
    // --continue reloads the previous save and the interrupted turn vanishes.
    app.streaming_state.reset();
    app.finalize_active_cell_as_interrupted();
    app.finalize_streaming_assistant_as_interrupted();
    persist_recovery_snapshot(app);
    app.is_loading = false;
    app.dispatch_started_at = None;
    app.turn_started_at = None;
    app.turn_last_activity_at = None;
    app.runtime_turn_id = None;
    app.runtime_turn_status = None;
    app.suppress_stream_events_until_turn_complete = true;
    crate::retry_status::clear();
    crate::tui::notifications::clear_taskbar_progress();
    crate::tui::notifications::stop_title_animation_quietly();
}

/// The Esc-shaped "cancel the active turn" body, extracted verbatim from the
/// event loop's `EscapeAction::CancelRequest` arm so the session control
/// socket's `interrupt` verb and the Esc key cannot drift apart. Returns
/// `true` when the caller should stop handling the event (compaction cancel
/// or goal-continuation stop consumed it), `false` otherwise. The caller
/// keeps its own Esc-specific state (backtrack reset) outside this body.
pub(crate) fn escape_cancel_request(
    app: &mut App,
    engine_handle: &EngineHandle,
    current_streaming_text: &mut String,
    stream_display_clock: &mut StreamDisplayClock,
) -> bool {
    if try_cancel_compaction(app, engine_handle) {
        return true;
    }
    if app.paused || app.paused_goal_objective.is_some() {
        clear_paused_command_state(app, engine_handle);
        if app.is_loading || matches!(app.runtime_turn_status.as_deref(), Some("in_progress")) {
            engine_handle.cancel();
            mark_active_turn_cancelled_locally(app);
            current_streaming_text.clear();
            stream_display_clock.reset();
        }
        app.active_allowed_tools = None;
        app.goal.objective = None;
        app.goal.tokens_used = 0;
        app.goal.time_used_seconds = 0;
        app.goal.continuation_count = 0;
        app.status_message = Some(parent_stop_status(app, "Paused command cancelled"));
        false
    } else {
        let was_waiting = app.goal_continuation_waiting;
        engine_handle.cancel();
        if was_waiting {
            app.goal_continuation_waiting = false;
            app.status_message = Some(app.tr(MessageId::GoalContinuationStopped).to_string());
            return true;
        }
        mark_active_turn_cancelled_locally(app);
        current_streaming_text.clear();
        stream_display_clock.reset();
        app.status_message = Some(parent_stop_status(app, "Request cancelled"));
        false
    }
}

fn suppress_engine_event_after_local_cancel(event: &EngineEvent) -> bool {
    matches!(
        event,
        EngineEvent::MessageStarted { .. }
            | EngineEvent::MessageDelta { .. }
            | EngineEvent::MessageComplete { .. }
            | EngineEvent::ThinkingStarted { .. }
            | EngineEvent::ThinkingDelta { .. }
            | EngineEvent::ThinkingComplete { .. }
            | EngineEvent::ToolCallStarted { .. }
            | EngineEvent::ToolCallHeartbeat
            | EngineEvent::ToolCallComplete { .. }
            | EngineEvent::ApprovalRequired { .. }
            | EngineEvent::UserInputRequired { .. }
            | EngineEvent::ElevationRequired { .. }
            | EngineEvent::SessionUpdated { .. }
    )
}

fn ignore_stale_stream_event_while_idle(event: &EngineEvent) -> bool {
    matches!(
        event,
        EngineEvent::MessageStarted { .. }
            | EngineEvent::MessageDelta { .. }
            | EngineEvent::MessageComplete { .. }
            | EngineEvent::ThinkingStarted { .. }
            | EngineEvent::ThinkingDelta { .. }
            | EngineEvent::ThinkingComplete { .. }
            | EngineEvent::ToolCallStarted { .. }
            | EngineEvent::ToolCallHeartbeat
            | EngineEvent::ToolCallComplete { .. }
            | EngineEvent::ApprovalRequired { .. }
            | EngineEvent::UserInputRequired { .. }
            | EngineEvent::ElevationRequired { .. }
    )
}

type ProviderKeyVerification<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

pub(crate) fn request_foreground_shell_background(app: &mut App) {
    if !app.is_loading {
        app.status_message = Some("No foreground shell wait to move to /jobs".to_string());
        return;
    }
    if !active_foreground_shell_running(app) {
        // #3032 AC3: name the reason backgrounding is unavailable —
        // interactive execs and non-shell blocking tools are visibly running
        // but cannot be detached, and a generic shrug reads like a bug.
        let reason = if terminal_pause_has_live_owner(app) {
            "the running command is interactive"
        } else if app
            .active_cell
            .as_ref()
            .is_some_and(|active| !active.is_empty())
        {
            "the running tool is not a foreground shell command"
        } else {
            "no foreground shell command is running"
        };
        app.status_message = Some(format!(
            "Cannot move to /jobs: {reason}. Press Ctrl+C to cancel the turn, or wait for completion."
        ));
        return;
    }

    match request_active_foreground_shell_background(app) {
        Ok(()) => {
            app.status_message = Some("Moving current shell command to /jobs...".to_string());
        }
        Err(err) => {
            app.status_message = Some(err.to_string());
        }
    }
}

fn request_active_foreground_shell_background(app: &App) -> Result<()> {
    let shell_manager = app
        .runtime_services
        .shell_manager
        .clone()
        .context("No shell session is active.")?;
    let mut manager = shell_manager.lock().map_err(|_| {
        anyhow::anyhow!("Shell tracking hit an internal error — restart Codewhale to recover.")
    })?;
    manager.request_foreground_background();
    Ok(())
}

pub(crate) fn prefill_jobs_cancel_all_if_tasks_sidebar(app: &mut App) -> bool {
    if !app.view_stack.is_empty()
        || app.work_surface.panel != crate::tui::work_surface::RailPanel::Tasks
        || app.work_surface.last_area.is_none()
        || !app
            .task_panel
            .iter()
            .any(|task| task.id.starts_with("shell_") && task.status == "running")
    {
        return false;
    }

    app.input = "/jobs cancel-all".to_string();
    app.cursor_position = app.input.len();
    app.status_message = Some("Press Enter to cancel all running commands".to_string());
    true
}

pub(crate) fn active_foreground_shell_running(app: &App) -> bool {
    app.active_cell.as_ref().is_some_and(|active| {
        active.entries().iter().any(|cell| {
            matches!(
                cell,
                HistoryCell::Tool(ToolCell::Exec(exec))
                    if exec.status == ToolStatus::Running
                        && exec.interaction.is_none()
                        && exec.shell_task_id.is_none()
            )
        })
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchDirection {
    Forward,
    Backward,
}

pub(crate) fn clamp_event_poll_timeout(timeout: Duration) -> Duration {
    const MIN_EVENT_POLL_TIMEOUT: Duration = Duration::from_millis(1);
    timeout.max(MIN_EVENT_POLL_TIMEOUT)
}

/// Decide whether an `AgentComplete` event should fire a subagent-completion
/// desktop notification, per the `[notifications].subagent_completion` mode.
/// `settings()` still has the final say (method=off / condition=never).
fn should_notify_subagent_completion(
    mode: crate::config::SubagentCompletionNotification,
    has_other_running_subagents: bool,
    workflow_tool_running: bool,
) -> bool {
    use crate::config::SubagentCompletionNotification as Mode;
    match mode {
        Mode::Off => false,
        Mode::Always => true,
        Mode::FinalOnly => !has_other_running_subagents && !workflow_tool_running,
    }
}

// Keyboard-shortcut predicates moved to `tui/key_shortcuts.rs`.

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StartupVersionCheckSource {
    Disabled,
    ConfiguredUrl(String),
    ReleaseResolver,
}

/// A newer-stable-release notice, carrying enough context to render both the
/// short transient toast and the durable in-transcript update prompt (#3961).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpdateNotice {
    current: String,
    latest: String,
}

impl UpdateNotice {
    /// Short line for the transient status toast, naming the command that
    /// actually updates *this* install.
    fn toast_line(&self, install: InstallMethod) -> String {
        format!(
            "v{latest} available - run `{command}` and restart",
            latest = self.latest,
            command = install.update_command()
        )
    }

    /// Compact header chip label shown once the check has landed. Quiet by
    /// design: no action verb, no repetition — the toast and transcript
    /// notice carry the update instructions (#14).
    fn chip_label(&self) -> String {
        format!("↑ v{latest}", latest = self.latest)
    }

    /// Durable, actionable notice pushed into the transcript so it survives the
    /// toast TTL. Includes current/latest versions, release notes, the exact
    /// update command, and restart guidance.
    ///
    /// Package-managed installs get their manager's command instead of
    /// `codewhale update`, plus an explicit warning: self-updating a binary
    /// Homebrew or npm owns leaves the manager's metadata lying about what is
    /// on disk, and the next upgrade silently reverts the user.
    fn notice_block(&self, install: InstallMethod) -> String {
        let action = if install.supports_self_update() {
            "Run `/update install` here (preview it with a bare `/update`), or `codewhale update` in a shell, then restart Codewhale."
                .to_string()
        } else {
            format!(
                "Installed via {label}. Run `{command}`, then restart Codewhale.\n\
                 Do not use `codewhale update` here — it would replace a binary {label} manages.",
                label = install.label(),
                command = install.update_command()
            )
        };
        format!(
            "Update available: v{current} -> v{latest}\n\
             Release notes: https://github.com/Hmbown/CodeWhale/releases/tag/v{latest}\n\
             {action}",
            current = self.current,
            latest = self.latest
        )
    }
}

mod activity_detail;

#[cfg(test)]
mod provider_key_validation_tests {
    use super::*;
    use crate::core::engine::mock_engine_handle;
    use ratatui::{buffer::Buffer, layout::Rect};
    use tempfile::TempDir;

    struct ConfigPathEnvGuard {
        _tmp: TempDir,
        _codewhale_config_path: crate::test_support::EnvVarGuard,
        _deepseek_config_path: crate::test_support::EnvVarGuard,
        _lock: crate::test_support::TestEnvLock,
    }

    impl ConfigPathEnvGuard {
        fn new() -> Self {
            let lock = crate::test_support::lock_test_env();
            let tmp = TempDir::new().expect("config tempdir");
            let config_path = tmp.path().join(".codewhale").join("config.toml");
            std::fs::create_dir_all(config_path.parent().expect("config parent"))
                .expect("config dir");
            Self {
                _tmp: tmp,
                _codewhale_config_path: crate::test_support::EnvVarGuard::set(
                    "CODEWHALE_CONFIG_PATH",
                    &config_path,
                ),
                _deepseek_config_path: crate::test_support::EnvVarGuard::set(
                    "DEEPSEEK_CONFIG_PATH",
                    &config_path,
                ),
                _lock: lock,
            }
        }

        fn config_path(&self) -> PathBuf {
            std::env::var_os("CODEWHALE_CONFIG_PATH")
                .map(PathBuf::from)
                .expect("config path set")
        }
    }

    fn create_test_app() -> App {
        let options = TuiOptions {
            start_in_agent_mode: true,
            skip_onboarding: false,
            ..crate::test_support::test_tui_options(PathBuf::from("."))
        };
        let mut app = App::new(options, &Config::default());
        // These suites assert legacy strip geometry (work surface above the
        // transcript). The Bottom default (round 3, 2026-09-01) has its own
        // coverage in work_surface::rail_panels_render_in_all_placements.
        app.work_surface.placement = crate::tui::work_surface::WorkSurfacePlacement::Top;
        app.api_provider = ApiProvider::Deepseek;
        app.model = "deepseek-v4-pro".to_string();
        app.auto_model = false;
        app
    }

    #[test]
    fn api_key_live_mirror_revokes_stale_external_credential_consent() {
        let external_path = if cfg!(windows) {
            PathBuf::from(r"C:\Users\test\grok-auth.json")
        } else {
            PathBuf::from("/tmp/grok-auth.json")
        };
        let mut config = Config {
            providers: Some(ProvidersConfig {
                xai: ProviderConfig {
                    auth_mode: Some("oauth".to_string()),
                    external_credentials: Some(
                        codewhale_config::ExternalCredentialConsentToml::read_only(
                            codewhale_config::ProviderKind::Xai,
                            codewhale_config::ExternalCredentialSource::GrokCli,
                            external_path,
                        ),
                    ),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        mirror_saved_api_key_in_config(
            &mut config,
            ApiProvider::Xai,
            "codewhale-owned-api-key".to_string(),
        );

        let xai = config
            .provider_config_for(ApiProvider::Xai)
            .expect("xAI live config");
        assert_eq!(xai.auth_mode.as_deref(), Some("api_key"));
        assert_eq!(xai.api_key.as_deref(), Some("codewhale-owned-api-key"));
        assert!(xai.external_credentials.is_none());
    }

    struct MockProviderKeyVerifier {
        result: Result<(), String>,
        calls: std::sync::Mutex<Vec<(ApiProvider, String, String)>>,
    }

    impl MockProviderKeyVerifier {
        fn new(result: Result<(), String>) -> Self {
            Self {
                result,
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<(ApiProvider, String, String)> {
            self.calls.lock().expect("calls lock").clone()
        }
    }

    impl ProviderKeyVerifier for MockProviderKeyVerifier {
        fn verify<'a>(
            &'a self,
            provider: ApiProvider,
            api_key: &'a str,
            base_url: &'a str,
        ) -> ProviderKeyVerification<'a> {
            self.calls.lock().expect("calls lock").push((
                provider,
                api_key.to_string(),
                base_url.to_string(),
            ));
            Box::pin(std::future::ready(self.result.clone()))
        }
    }

    fn openrouter_config(base_url: &str) -> Config {
        Config {
            providers: Some(ProvidersConfig {
                openrouter: ProviderConfig {
                    base_url: Some(base_url.to_string()),
                    ..ProviderConfig::default()
                },
                ..ProvidersConfig::default()
            }),
            ..Config::default()
        }
    }

    fn two_named_custom_routes() -> Config {
        Config {
            provider: Some("custom-a".to_string()),
            providers: Some(ProvidersConfig {
                custom: std::collections::HashMap::from([
                    (
                        "custom-a".to_string(),
                        ProviderConfig {
                            kind: Some("openai-compatible".to_string()),
                            base_url: Some("http://127.0.0.1:18181/v1".to_string()),
                            model: Some("model-a".to_string()),
                            api_key: Some("key-a".to_string()),
                            ..Default::default()
                        },
                    ),
                    (
                        "custom-b".to_string(),
                        ProviderConfig {
                            kind: Some("openai-compatible".to_string()),
                            base_url: Some("http://127.0.0.1:18182/v1".to_string()),
                            model: Some("model-b".to_string()),
                            ..Default::default()
                        },
                    ),
                ]),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn provider_key_check_classifies_transport_failures_truthfully() {
        assert_eq!(
            provider_verification_error_category("connection refused"),
            crate::error_taxonomy::ErrorCategory::Network
        );
        assert_eq!(
            provider_verification_error_category("request timed out"),
            crate::error_taxonomy::ErrorCategory::Timeout
        );
        assert_eq!(
            provider_verification_error_category("HTTP 429 rate limit"),
            crate::error_taxonomy::ErrorCategory::RateLimit
        );
        assert_eq!(
            provider_verification_error_category("HTTP 401 unauthorized"),
            crate::error_taxonomy::ErrorCategory::Authentication
        );
        assert_eq!(
            provider_verification_error_category("HTTP 403 forbidden"),
            crate::error_taxonomy::ErrorCategory::Authorization
        );
        assert_eq!(
            provider_verification_error_category("HTTP 500 upstream failure"),
            crate::error_taxonomy::ErrorCategory::Network
        );
    }

    #[tokio::test]
    async fn provider_key_submit_opens_model_pick_without_persisting_on_validation_success() {
        let config_env = ConfigPathEnvGuard::new();
        let mut app = create_test_app();
        let mut engine = mock_engine_handle();
        let mut config = openrouter_config("https://mock.openrouter.test/v1");
        let verifier = MockProviderKeyVerifier::new(Ok(()));
        let identity = picker_provider_identity(&config, ApiProvider::Openrouter, None)
            .expect("OpenRouter identity");

        apply_provider_picker_api_key_with_verifier(
            &mut app,
            &mut engine.handle,
            &mut config,
            identity,
            "sk-verified".to_string(),
            None,
            &verifier,
        )
        .await;

        assert_eq!(
            verifier.calls(),
            vec![(
                ApiProvider::Openrouter,
                "sk-verified".to_string(),
                "https://mock.openrouter.test/v1".to_string()
            )]
        );
        // Validation success must not persist or switch yet (#3875 residual):
        // the guided flow continues at model pick first.
        assert_eq!(app.api_provider, ApiProvider::Deepseek);
        assert_eq!(config.provider.as_deref(), None);
        assert_eq!(
            config
                .providers
                .as_ref()
                .and_then(|providers| providers.openrouter.api_key.as_deref()),
            None
        );
        let saved = std::fs::read_to_string(config_env.config_path()).unwrap_or_default();
        assert!(!saved.contains("sk-verified"));
        assert_eq!(app.view_stack.top_kind(), Some(ModalKind::ProviderPicker));
        assert!(
            app.status_message.as_deref().is_some_and(|status| {
                status.contains("Connection checked (/models returned 2xx)")
            }),
            "status names connection-probe success: {:?}",
            app.status_message
        );
        let verified_route = crate::provider_readiness::route_identity_for_model(
            &config,
            ApiProvider::Openrouter,
            crate::config::DEFAULT_OPENROUTER_MODEL,
        );
        assert_eq!(
            crate::provider_readiness::resolve_with_identity(
                &verified_route,
                crate::provider_readiness::CredentialState::Saved,
                true,
                &app.provider_health,
            ),
            crate::provider_readiness::ResolvedProviderReadiness::ConnectionCheckedModelUnchecked,
            "the live connection probe must not be reported as model ready",
        );

        let picker = app.view_stack.pop().expect("provider picker reopened");
        let area = Rect::new(0, 0, 90, 16);
        let mut buf = Buffer::empty(area);
        picker.render(area, &mut buf);
        let rendered = (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            rendered.contains("Connection checked (/models returned 2xx)")
                && rendered.contains("Pick a default model"),
            "expected model-pick stage UI, got:\n{rendered}"
        );
    }

    #[tokio::test]
    async fn test_connection_records_models_probe_not_ready() {
        let config_env = ConfigPathEnvGuard::new();
        let mut app = create_test_app();
        let mut engine = mock_engine_handle();
        let mut config = openrouter_config("https://mock.openrouter.test/v1");
        config.provider = Some("openrouter".to_string());
        if let Some(providers) = config.providers.as_mut() {
            providers.openrouter.api_key = Some("sk-saved".to_string());
        }
        let verifier = MockProviderKeyVerifier::new(Ok(()));
        let identity = picker_provider_identity(&config, ApiProvider::Openrouter, None)
            .expect("OpenRouter identity");

        apply_provider_picker_test_connection_with_verifier(
            &mut app,
            &mut engine.handle,
            &mut config,
            identity,
            false,
            &verifier,
        )
        .await;

        assert_eq!(
            verifier.calls(),
            vec![(
                ApiProvider::Openrouter,
                "sk-saved".to_string(),
                "https://mock.openrouter.test/v1".to_string()
            )]
        );
        let _ = config_env;
        assert_eq!(config.provider.as_deref(), Some("openrouter"));
        assert!(
            app.status_toasts.iter().any(|toast| {
                toast
                    .text
                    .contains("Connection checked (/models returned 2xx)")
                    && !toast.text.contains("Pick a default model")
            }),
            "test connection names reachability only: {:?}",
            app.status_toasts
        );
        let verified_route = crate::provider_readiness::route_identity_for_model(
            &config,
            ApiProvider::Openrouter,
            crate::config::DEFAULT_OPENROUTER_MODEL,
        );
        assert_eq!(
            crate::provider_readiness::resolve_with_identity(
                &verified_route,
                crate::provider_readiness::CredentialState::Saved,
                true,
                &app.provider_health,
            ),
            crate::provider_readiness::ResolvedProviderReadiness::ConnectionCheckedModelUnchecked,
        );
        assert_ne!(
            crate::provider_readiness::resolve_with_identity(
                &verified_route,
                crate::provider_readiness::CredentialState::Saved,
                true,
                &app.provider_health,
            ),
            crate::provider_readiness::ResolvedProviderReadiness::Ready,
        );
        assert_eq!(app.view_stack.top_kind(), Some(ModalKind::ProviderPicker));
    }

    #[tokio::test]
    async fn test_connection_without_key_does_not_mark_ready() {
        let config_env = ConfigPathEnvGuard::new();
        let mut app = create_test_app();
        let mut engine = mock_engine_handle();
        let mut config = openrouter_config("https://mock.openrouter.test/v1");
        let verifier = MockProviderKeyVerifier::new(Ok(()));
        let identity = picker_provider_identity(&config, ApiProvider::Openrouter, None)
            .expect("OpenRouter identity");

        apply_provider_picker_test_connection_with_verifier(
            &mut app,
            &mut engine.handle,
            &mut config,
            identity,
            false,
            &verifier,
        )
        .await;

        assert!(verifier.calls().is_empty());
        let _ = config_env;
        assert!(
            app.status_toasts
                .iter()
                .any(|toast| toast.text.contains("No API key saved")),
            "{:?}",
            app.status_toasts
        );
        let verified_route = crate::provider_readiness::route_identity_for_model(
            &config,
            ApiProvider::Openrouter,
            crate::config::DEFAULT_OPENROUTER_MODEL,
        );
        assert_eq!(
            crate::provider_readiness::resolve_with_identity(
                &verified_route,
                crate::provider_readiness::CredentialState::MissingKey,
                true,
                &app.provider_health,
            ),
            crate::provider_readiness::ResolvedProviderReadiness::MissingKey,
        );
    }

    #[tokio::test]
    async fn test_connection_failure_redacts_the_api_key() {
        let config_env = ConfigPathEnvGuard::new();
        let mut app = create_test_app();
        let mut engine = mock_engine_handle();
        let mut config = openrouter_config("https://mock.openrouter.test/v1");
        config.provider = Some("openrouter".to_string());
        if let Some(providers) = config.providers.as_mut() {
            providers.openrouter.api_key = Some("sk-saved".to_string());
        }
        let verifier = MockProviderKeyVerifier::new(Err(
            "HTTP 401: upstream echoed sk-saved in a long diagnostic body that must not stay visible"
                .repeat(4),
        ));
        let identity = picker_provider_identity(&config, ApiProvider::Openrouter, None)
            .expect("OpenRouter identity");

        apply_provider_picker_test_connection_with_verifier(
            &mut app,
            &mut engine.handle,
            &mut config,
            identity,
            true,
            &verifier,
        )
        .await;

        let _ = config_env;
        let status = app
            .status_toasts
            .iter()
            .map(|toast| toast.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !status.contains("sk-saved"),
            "probe toast leaked the API key: {status}"
        );
        assert!(status.contains("***"), "{status}");
        assert!(
            app.provider_picker_memory
                .as_ref()
                .is_some_and(|memory| memory.catalog_view),
            "catalog browsing context must survive the probe"
        );
        let verified_route = crate::provider_readiness::route_identity_for_model(
            &config,
            ApiProvider::Openrouter,
            crate::config::DEFAULT_OPENROUTER_MODEL,
        );
        assert!(matches!(
            crate::provider_readiness::resolve_with_identity(
                &verified_route,
                crate::provider_readiness::CredentialState::Saved,
                true,
                &app.provider_health,
            ),
            crate::provider_readiness::ResolvedProviderReadiness::SavedLastCheckFailed { .. }
        ));
    }

    /// #4526: the wizard's StepFun billing-route choice must be the endpoint
    /// the key is probed against, and it must reach disk only once the user
    /// confirms — never as a side effect of validation.
    #[tokio::test]
    async fn stepfun_plan_route_is_validated_before_the_key_is_persisted() {
        let config_env = ConfigPathEnvGuard::new();
        let mut app = create_test_app();
        let mut engine = mock_engine_handle();
        let mut config = Config::default();
        let verifier = MockProviderKeyVerifier::new(Ok(()));
        let identity = picker_provider_identity(&config, ApiProvider::Stepfun, None)
            .expect("StepFun identity");

        apply_provider_picker_api_key_with_verifier(
            &mut app,
            &mut engine.handle,
            &mut config,
            identity,
            "step-plan-key".to_string(),
            Some(crate::config::DEFAULT_STEPFUN_PLAN_BASE_URL.to_string()),
            &verifier,
        )
        .await;

        assert_eq!(
            verifier.calls(),
            vec![(
                ApiProvider::Stepfun,
                "step-plan-key".to_string(),
                crate::config::DEFAULT_STEPFUN_PLAN_BASE_URL.to_string()
            )],
            "the chosen Step Plan endpoint must be the one live-validated"
        );
        assert_eq!(
            config
                .providers
                .as_ref()
                .and_then(|providers| providers.stepfun.base_url.clone()),
            None,
            "validation must not mutate the live config"
        );
        let saved = std::fs::read_to_string(config_env.config_path()).unwrap_or_default();
        assert!(
            !saved.contains("step_plan"),
            "nothing persisted yet: {saved}"
        );
        assert!(!saved.contains("step-plan-key"), "no secret yet: {saved}");
    }

    /// The confirm stage writes the endpoint into `[providers.stepfun]` and
    /// leaves every other provider table alone.
    #[tokio::test]
    async fn stepfun_setup_confirm_writes_only_the_stepfun_base_url() {
        let config_env = ConfigPathEnvGuard::new();
        let mut app = create_test_app();
        let mut engine = mock_engine_handle();
        let mut config = Config::default();
        let identity = picker_provider_identity(&config, ApiProvider::Stepfun, None)
            .expect("StepFun identity");

        apply_provider_picker_setup_confirmed(
            &mut app,
            &mut engine.handle,
            &mut config,
            identity,
            "step-plan-key".to_string(),
            crate::config::DEFAULT_STEPFUN_MODEL.to_string(),
            None,
            Some(crate::config::DEFAULT_STEPFUN_PLAN_BASE_URL.to_string()),
        )
        .await;

        let saved = std::fs::read_to_string(config_env.config_path()).expect("config written");
        let document: toml::Table = toml::from_str(&saved).expect("valid TOML");
        let providers = document
            .get("providers")
            .and_then(toml::Value::as_table)
            .expect("providers table");
        assert_eq!(
            providers
                .get("stepfun")
                .and_then(|entry| entry.get("base_url"))
                .and_then(toml::Value::as_str),
            Some(crate::config::DEFAULT_STEPFUN_PLAN_BASE_URL)
        );
        assert_eq!(
            providers.keys().collect::<Vec<_>>(),
            vec!["stepfun"],
            "the route choice must not touch other provider tables"
        );
        assert!(
            document.get("base_url").is_none(),
            "the root base_url must stay untouched: {saved}"
        );
        assert_eq!(
            config
                .providers
                .as_ref()
                .and_then(|providers| providers.stepfun.base_url.as_deref()),
            Some(crate::config::DEFAULT_STEPFUN_PLAN_BASE_URL),
            "the live config mirrors the persisted endpoint"
        );
    }

    #[tokio::test]
    async fn replacing_legacy_kimi_import_verifies_and_persists_the_kimi_code_api_key_route() {
        let config_env = ConfigPathEnvGuard::new();
        std::fs::write(
            config_env.config_path(),
            r#"# preserve-kimi-comment
[providers.moonshot]
auth_mode = "kimi_oauth"
"#,
        )
        .expect("seed legacy Kimi import config");
        let mut app = create_test_app();
        let mut engine = mock_engine_handle();
        let mut config = Config {
            providers: Some(ProvidersConfig {
                moonshot: ProviderConfig {
                    auth_mode: Some("kimi_oauth".to_string()),
                    ..ProviderConfig::default()
                },
                ..ProvidersConfig::default()
            }),
            ..Config::default()
        };
        let identity = picker_provider_identity(&config, ApiProvider::Moonshot, None)
            .expect("Moonshot identity");
        let verifier = MockProviderKeyVerifier::new(Ok(()));

        apply_provider_picker_api_key_with_verifier(
            &mut app,
            &mut engine.handle,
            &mut config,
            identity.clone(),
            "sk-kimi-supported".to_string(),
            None,
            &verifier,
        )
        .await;

        assert_eq!(
            verifier.calls(),
            vec![(
                ApiProvider::Moonshot,
                "sk-kimi-supported".to_string(),
                crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string(),
            )],
            "replacement keys must be verified against Kimi Code, not the ordinary Moonshot API"
        );

        apply_provider_picker_setup_confirmed(
            &mut app,
            &mut engine.handle,
            &mut config,
            identity,
            "sk-kimi-supported".to_string(),
            crate::config::DEFAULT_KIMI_CODE_MODEL.to_string(),
            None,
            None,
        )
        .await;

        let moonshot = config
            .providers
            .as_ref()
            .map(|providers| &providers.moonshot)
            .expect("in-memory Moonshot config");
        assert_eq!(moonshot.auth_mode.as_deref(), Some("api_key"));
        assert_eq!(
            moonshot.base_url.as_deref(),
            Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL)
        );
        assert_eq!(moonshot.api_key.as_deref(), Some("sk-kimi-supported"));

        let saved = std::fs::read_to_string(config_env.config_path()).expect("saved config");
        assert!(saved.contains("# preserve-kimi-comment"));
        assert!(saved.contains("auth_mode = \"api_key\""));
        assert!(saved.contains(&format!(
            "base_url = \"{}\"",
            crate::config::DEFAULT_KIMI_CODE_BASE_URL
        )));
    }

    #[tokio::test]
    async fn provider_setup_confirm_persists_provider_model_and_preserves_comments() {
        let config_env = ConfigPathEnvGuard::new();
        // Seed a commented config so the confirm path must preserve it.
        std::fs::write(
            config_env.config_path(),
            r#"# keep-me-comment
[providers.openrouter]
# openrouter-table-comment
base_url = "https://mock.openrouter.test/v1"

[providers.anthropic]
api_key = "fixture-other-provider-key"
"#,
        )
        .expect("seed config");

        let mut app = create_test_app();
        let mut engine = mock_engine_handle();
        let mut config = openrouter_config("https://mock.openrouter.test/v1");
        config
            .providers
            .get_or_insert_with(ProvidersConfig::default)
            .anthropic
            .api_key = Some("fixture-other-provider-key".to_string());
        let model = "deepseek/deepseek-v4-pro".to_string();
        let identity = picker_provider_identity(&config, ApiProvider::Openrouter, None)
            .expect("OpenRouter identity");

        apply_provider_picker_setup_confirmed(
            &mut app,
            &mut engine.handle,
            &mut config,
            identity,
            "sk-confirmed".to_string(),
            model.clone(),
            None,
            None,
        )
        .await;

        assert_eq!(app.api_provider, ApiProvider::Openrouter);
        assert_eq!(config.provider.as_deref(), Some("openrouter"));
        assert_eq!(
            config
                .providers
                .as_ref()
                .and_then(|providers| providers.openrouter.api_key.as_deref()),
            Some("sk-confirmed")
        );
        assert_eq!(
            config
                .providers
                .as_ref()
                .and_then(|providers| providers.openrouter.model.as_deref()),
            Some(model.as_str())
        );
        let saved = std::fs::read_to_string(config_env.config_path()).expect("saved config");
        assert!(
            saved.contains("# keep-me-comment"),
            "root comment lost:\n{saved}"
        );
        assert!(
            saved.contains("# openrouter-table-comment"),
            "table comment lost:\n{saved}"
        );
        assert!(saved.contains("[providers.openrouter]"));
        assert!(saved.contains("api_key = \"sk-confirmed\""));
        assert!(saved.contains(&format!("model = \"{model}\"")));
        assert!(saved.contains("[providers.anthropic]"));
        assert!(saved.contains("api_key = \"fixture-other-provider-key\""));
        assert_eq!(
            config
                .providers
                .as_ref()
                .and_then(|providers| providers.anthropic.api_key.as_deref()),
            Some("fixture-other-provider-key"),
            "saving OpenRouter must not overwrite a different provider slot"
        );
    }

    #[tokio::test]
    async fn provider_key_submit_reopens_picker_without_persisting_on_validation_failure() {
        let config_env = ConfigPathEnvGuard::new();
        let mut app = create_test_app();
        let mut engine = mock_engine_handle();
        let mut config = openrouter_config("https://mock.openrouter.test/v1");
        let verifier = MockProviderKeyVerifier::new(Err("HTTP 401: unauthorized".to_string()));
        let identity = picker_provider_identity(&config, ApiProvider::Openrouter, None)
            .expect("OpenRouter identity");

        apply_provider_picker_api_key_with_verifier(
            &mut app,
            &mut engine.handle,
            &mut config,
            identity,
            "sk-rejected".to_string(),
            None,
            &verifier,
        )
        .await;

        assert_eq!(app.api_provider, ApiProvider::Deepseek);
        assert_eq!(config.provider.as_deref(), None);
        assert_eq!(
            config
                .providers
                .as_ref()
                .and_then(|providers| providers.openrouter.api_key.as_deref()),
            None
        );
        let saved = std::fs::read_to_string(config_env.config_path()).unwrap_or_default();
        assert!(!saved.contains("sk-rejected"));
        assert_eq!(app.view_stack.top_kind(), Some(ModalKind::ProviderPicker));
        assert!(
            app.status_message
                .as_deref()
                .is_some_and(|status| status.contains("API key verification failed")),
            "status names validation failure: {:?}",
            app.status_message
        );

        let picker = app.view_stack.pop().expect("provider picker reopened");
        let area = Rect::new(0, 0, 90, 14);
        let mut buf = Buffer::empty(area);
        picker.render(area, &mut buf);
        let rendered = (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("Verification failed: HTTP 401: unauthorized"));
    }

    #[tokio::test]
    async fn named_custom_verification_failure_and_dismiss_keep_committed_a_route() {
        let _config_env = ConfigPathEnvGuard::new();
        let mut app = create_test_app();
        app.set_provider_identity(ApiProvider::Custom, "custom-a");
        app.set_model_selection("model-a".to_string());
        let mut engine = mock_engine_handle();
        let mut config = two_named_custom_routes();
        let identity = picker_provider_identity(&config, ApiProvider::Custom, Some("custom-b"))
            .expect("custom B identity");
        let verifier = MockProviderKeyVerifier::new(Err("HTTP 401: unauthorized".to_string()));

        apply_provider_picker_api_key_with_verifier(
            &mut app,
            &mut engine.handle,
            &mut config,
            identity,
            "rejected-b-key".to_string(),
            None,
            &verifier,
        )
        .await;

        assert_eq!(config.provider.as_deref(), Some("custom-a"));
        assert_eq!(app.provider_identity_for_persistence(), "custom-a");
        app.view_stack.pop().expect("failed verifier picker");
        sync_config_provider_from_app(&mut config, &app);
        let route = validated_app_runtime_route(&app, &config).expect("committed A route");
        assert_eq!(route.identity.key, "custom-a");
        assert_eq!(route.client.base_url(), "http://127.0.0.1:18181/v1");
    }

    #[tokio::test]
    async fn named_custom_setup_persists_exact_provider_table_and_model() {
        let config_env = ConfigPathEnvGuard::new();
        std::fs::write(
            config_env.config_path(),
            r#"provider = "custom-a"

[providers.custom-a]
kind = "openai-compatible"
base_url = "http://127.0.0.1:18181/v1"
model = "model-a"

[providers.custom-b]
kind = "openai-compatible"
base_url = "http://127.0.0.1:18182/v1"
model = "model-b"
"#,
        )
        .expect("seed named custom config");
        let mut app = create_test_app();
        app.set_provider_identity(ApiProvider::Custom, "custom-a");
        app.set_model_selection("model-a".to_string());
        let mut engine = mock_engine_handle();
        let mut config = two_named_custom_routes();
        let identity = picker_provider_identity(&config, ApiProvider::Custom, Some("custom-b"))
            .expect("custom B identity");

        apply_provider_picker_setup_confirmed(
            &mut app,
            &mut engine.handle,
            &mut config,
            identity,
            "saved-b-key".to_string(),
            "model-b-confirmed".to_string(),
            None,
            None,
        )
        .await;

        assert_eq!(app.provider_identity_for_persistence(), "custom-b");
        assert_eq!(config.provider.as_deref(), Some("custom-b"));
        let saved = std::fs::read_to_string(config_env.config_path()).expect("saved config");
        assert!(saved.contains("[providers.custom-b]"));
        assert!(saved.contains("api_key = \"saved-b-key\""));
        assert!(saved.contains("model = \"model-b-confirmed\""));
        assert!(!saved.contains("[providers.custom]\n"));
    }

    #[test]
    fn legacy_literal_custom_identity_persistence_stays_root_shaped() {
        let config_env = ConfigPathEnvGuard::new();
        std::fs::write(
            config_env.config_path(),
            r#"provider = "custom"
base_url = "http://127.0.0.1:18180/v1"
default_text_model = "legacy-model"
"#,
        )
        .expect("seed legacy root route");
        let config = Config {
            provider: Some("custom".to_string()),
            base_url: Some("http://127.0.0.1:18180/v1".to_string()),
            default_text_model: Some("legacy-model".to_string()),
            ..Default::default()
        };
        let identity = config
            .resolve_provider_identity("custom")
            .expect("legacy identity");

        crate::config::save_api_key_for_identity(&identity, &config, "legacy-saved-key")
            .expect("save legacy key");
        crate::config::save_provider_model_for_identity(&identity, &config, "legacy-model-updated")
            .expect("save legacy model");

        let saved = std::fs::read_to_string(config_env.config_path()).expect("saved config");
        assert!(saved.contains("api_key = \"legacy-saved-key\""));
        assert!(saved.contains("default_text_model = \"legacy-model-updated\""));
        assert!(!saved.contains("[providers.custom]"));
        let reloaded = Config::load(Some(config_env.config_path()), None).expect("reload legacy");
        assert!(reloaded.uses_legacy_literal_custom_route());
        assert_eq!(
            reloaded
                .resolve_provider_identity("custom")
                .expect("repeat legacy identity"),
            identity
        );
        let route =
            resolve_runtime_route(&reloaded, ApiProvider::Custom, Some("legacy-model-updated"))
                .expect("resolve reloaded legacy")
                .validate()
                .expect("preflight reloaded legacy");
        assert_eq!(route.client.base_url(), "http://127.0.0.1:18180/v1");
    }

    #[test]
    fn legacy_active_route_does_not_redirect_named_custom_persistence_to_root() {
        let config_env = ConfigPathEnvGuard::new();
        std::fs::write(
            config_env.config_path(),
            r#"provider = "custom"
api_key = "legacy-root-key"
base_url = "http://127.0.0.1:18180/v1"
default_text_model = "legacy-model"

[providers.custom-b]
kind = "openai-compatible"
base_url = "http://127.0.0.1:18182/v1"
model = "model-b"
"#,
        )
        .expect("seed coexistence config");
        let config = Config::load(Some(config_env.config_path()), None).expect("load config");
        assert!(config.uses_legacy_literal_custom_route());
        let identity = config
            .resolve_provider_identity("custom-b")
            .expect("named custom identity");

        crate::config::save_api_key_for_identity(&identity, &config, "saved-b-key")
            .expect("save named custom key");
        crate::config::save_provider_model_for_identity(&identity, &config, "model-b-updated")
            .expect("save named custom model");

        let saved = std::fs::read_to_string(config_env.config_path()).expect("saved config");
        assert!(saved.contains("api_key = \"legacy-root-key\""));
        assert!(saved.contains("default_text_model = \"legacy-model\""));
        assert!(saved.contains("[providers.custom-b]"));
        assert!(saved.contains("api_key = \"saved-b-key\""));
        assert!(saved.contains("model = \"model-b-updated\""));
    }
}

/// Build the foreground receipt only from the immutable route captured when
/// this turn started. The app's selected route may already have changed by the
/// time `TurnComplete` is handled, so it is not accepted as an input here.
fn completed_turn_cost_route_receipt(
    completed_turn: Option<&crate::tui::app::ActiveTurnMetadata>,
    audit: &crate::pricing::TurnCostAudit,
) -> Option<String> {
    let route = completed_turn?.route.as_ref()?;
    Some(route.cost_envelope()?.receipt(audit))
}

#[cfg(test)]
mod tests;
