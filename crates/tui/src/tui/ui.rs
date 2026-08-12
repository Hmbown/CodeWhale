//! TUI event loop and rendering logic for `DeepSeek` CLI.

use std::cell::Cell;
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
use std::thread::{self, JoinHandle};
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
    apply_workflow_ui_event, handle_tool_call_complete, handle_tool_call_started,
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
    HuntVerdict, OnboardingState, PendingProviderSwitch, QueuedMessage, ReasoningEffort,
    StatusToast, StatusToastLevel, SubmitDisposition, TaskPanelEntry, TaskPanelEntryKind,
    ToolEvidence, TuiOptions, bound_agent_activity_text, is_stop_word,
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
    copy_cell_to_clipboard, detail_target_label, open_details_pager_for_cell, turn_handoff_markdown,
};
use self::activity_detail::{
    copy_focused_cell, detail_target_cell_index, extract_reasoning_header,
    open_reasoning_detail_pager, open_tool_details_pager, open_turn_inspector_pager,
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

fn is_session_approved_for_tool(app: &App, tool_name: &str, grouping_key: &str) -> bool {
    app.approval_session_approved.contains(grouping_key)
        || app.approval_session_approved.contains(tool_name)
}

fn is_session_denied_for_key(app: &App, approval_key: &str) -> bool {
    app.approval_session_denied.contains(approval_key)
}

fn session_denied_notice(app: &App, tool_name: &str) -> String {
    app.tr(MessageId::ApprovalAutoDeniedSession)
        .replace("{tool}", tool_name)
}

fn surface_session_denied_notice(app: &mut App, tool_name: &str) {
    let notice = session_denied_notice(app, tool_name);
    app.status_message = Some(notice.clone());
    app.push_status_toast(notice.clone(), StatusToastLevel::Warning, Some(12_000));

    // Tool completion and turn completion can replace the one-line status
    // before the next frame is painted. Keep the recovery path in the
    // transcript as a settled receipt as well, where it survives that event
    // ordering and remains available to screen readers and scrollback.
    let latest_transcript_cell = app
        .active_cell
        .as_ref()
        .and_then(|cell| cell.entries().last())
        .or_else(|| app.history.last());
    let already_latest_receipt = matches!(
        latest_transcript_cell,
        Some(HistoryCell::System { content }) if content == &notice
    );
    if !already_latest_receipt {
        let receipt = HistoryCell::System { content: notice };
        if let Some(active_cell) = app.active_cell.as_mut() {
            // Never grow committed history underneath an active cell: tool
            // lookup indices address `history ++ active_cell`, so changing
            // history.len() mid-turn would retarget the pending completion.
            active_cell.push_untracked(receipt);
            app.bump_active_cell_revision();
        } else {
            app.add_message(receipt);
        }
    }
}

async fn auto_deny_session_approval(
    app: &mut App,
    engine_handle: &EngineHandle,
    id: &str,
    tool_name: &str,
    approval_key: &str,
) {
    log_sensitive_event(
        "tool.approval.auto_deny_session",
        serde_json::json!({
            "tool_name": tool_name,
            "approval_key": approval_key,
            "session_id": app.current_session_id,
        }),
    );
    let _ = engine_handle.deny_tool_call(id.to_string()).await;
    surface_session_denied_notice(app, tool_name);
}

fn app_auto_approve_enabled(app: &App) -> bool {
    app.mode == AppMode::Yolo || app.approval_mode == ApprovalMode::Bypass
}

/// Build the UI-side TurnAuthority for approval disposition (#4412).
///
/// Shell/trust bits do not affect disposition; mode + approval_mode + the
/// full-access shape (Yolo/Bypass) are what the shared resolver consults.
fn app_turn_authority_for_approvals(app: &App) -> crate::core::authority::TurnAuthority {
    crate::core::authority::TurnAuthority::from_effective_fields(
        app.mode,
        true,
        false,
        app_auto_approve_enabled(app),
        app.approval_mode,
    )
}

fn resolve_ui_approval_disposition(
    app: &App,
    tool_name: &str,
    grouping_key: &str,
    approval_key: &str,
    approval_force_prompt: bool,
) -> crate::core::authority::ApprovalRequestDisposition {
    crate::core::authority::resolve_approval_request_disposition(
        &app_turn_authority_for_approvals(app),
        is_session_approved_for_tool(app, tool_name, grouping_key),
        is_session_denied_for_key(app, approval_key),
        approval_force_prompt,
    )
}

fn should_suppress_user_input_prompt(app: &App) -> bool {
    // Legacy hosts may still report Yolo/auto-approve with a stale `Auto`
    // enum. Canonicalize that shape to Full Access before applying the one
    // posture that suppresses questions: genuine Auto-Review.
    let effective_posture = if app_auto_approve_enabled(app) {
        ApprovalMode::Bypass
    } else {
        app.approval_mode
    };
    !crate::core::authority::permission_posture_allows_questions(effective_posture)
}

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
const TERMINAL_INPUT_POLL_INTERVAL: Duration = Duration::from_millis(50);
const TERMINAL_INPUT_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(500);
const TERMINAL_INPUT_STALL_TIMEOUT: Duration = Duration::from_secs(5);
const TERMINAL_INPUT_RECOVERY_COOLDOWN: Duration = Duration::from_secs(10);
const TERMINAL_INPUT_CHILD_PAUSE_TIMEOUT: Duration = Duration::from_millis(500);
const TERMINAL_INPUT_CHILD_PAUSE_POLL_INTERVAL: Duration = Duration::from_millis(5);
/// Upper bound on engine events processed before yielding to terminal input.
const MAX_ENGINE_EVENTS_PER_DRAIN: usize = 16;
/// Wall-clock budget for one engine drain batch (#1830 / #2317 input fairness).
const ENGINE_DRAIN_TIME_BUDGET: Duration = Duration::from_millis(8);
/// Throttled in-progress checkpoint while a turn is live (#1830 progress loss).
const RECOVERY_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(45);

enum TerminalInputMessage {
    Event(Event),
    Heartbeat,
    Error(io::Error),
}

pub(crate) struct TerminalInputPump {
    rx: std::sync::mpsc::Receiver<TerminalInputMessage>,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    paused_ack: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    last_alive_at: Cell<Instant>,
}

struct TerminalInputPumpParts {
    rx: std::sync::mpsc::Receiver<TerminalInputMessage>,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    paused_ack: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

impl TerminalInputPump {
    fn spawn() -> io::Result<Self> {
        let parts = Self::spawn_parts()?;
        Ok(Self {
            rx: parts.rx,
            stop: parts.stop,
            paused: parts.paused,
            paused_ack: parts.paused_ack,
            handle: Some(parts.handle),
            last_alive_at: Cell::new(Instant::now()),
        })
    }

    fn spawn_parts() -> io::Result<TerminalInputPumpParts> {
        let (tx, rx) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let paused_ack = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread_paused = Arc::clone(&paused);
        let thread_paused_ack = Arc::clone(&paused_ack);
        let handle = thread::Builder::new()
            .name("codewhale-terminal-input".to_string())
            .spawn(move || {
                let mut last_heartbeat = Instant::now();
                while !thread_stop.load(Ordering::Acquire) {
                    if thread_paused.load(Ordering::Acquire) {
                        thread_paused_ack.store(true, Ordering::Release);
                        thread::sleep(TERMINAL_INPUT_CHILD_PAUSE_POLL_INTERVAL);
                        continue;
                    }
                    thread_paused_ack.store(false, Ordering::Release);
                    match event::poll(TERMINAL_INPUT_POLL_INTERVAL) {
                        Ok(true) => match event::read() {
                            Ok(event) => {
                                last_heartbeat = Instant::now();
                                if tx.send(TerminalInputMessage::Event(event)).is_err() {
                                    break;
                                }
                            }
                            Err(err) => {
                                let _ = tx.send(TerminalInputMessage::Error(err));
                                break;
                            }
                        },
                        Ok(false) => {
                            let now = Instant::now();
                            if now.duration_since(last_heartbeat)
                                >= TERMINAL_INPUT_HEARTBEAT_INTERVAL
                            {
                                last_heartbeat = now;
                                if tx.send(TerminalInputMessage::Heartbeat).is_err() {
                                    break;
                                }
                            }
                        }
                        Err(err) => {
                            let _ = tx.send(TerminalInputMessage::Error(err));
                            break;
                        }
                    }
                }
            })?;
        Ok(TerminalInputPumpParts {
            rx,
            stop,
            paused,
            paused_ack,
            handle,
        })
    }

    fn recv_timeout(&self, timeout: Duration) -> io::Result<Option<Event>> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(remaining) {
                Ok(TerminalInputMessage::Event(event)) => {
                    self.mark_alive();
                    return Ok(Some(event));
                }
                Ok(TerminalInputMessage::Heartbeat) => {
                    self.mark_alive();
                    if remaining.is_zero() {
                        return Ok(None);
                    }
                }
                Ok(TerminalInputMessage::Error(err)) => {
                    self.mark_alive();
                    return Err(err);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return Ok(None),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "terminal input pump disconnected",
                    ));
                }
            }
        }
    }

    fn try_recv(&self) -> io::Result<Option<Event>> {
        loop {
            match self.rx.try_recv() {
                Ok(TerminalInputMessage::Event(event)) => {
                    self.mark_alive();
                    return Ok(Some(event));
                }
                Ok(TerminalInputMessage::Heartbeat) => {
                    self.mark_alive();
                }
                Ok(TerminalInputMessage::Error(err)) => {
                    self.mark_alive();
                    return Err(err);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return Ok(None),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return Ok(None),
            }
        }
    }

    fn mark_alive(&self) {
        self.last_alive_at.set(Instant::now());
    }

    fn stalled_for(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.last_alive_at.get())
    }

    fn pause_for_child_terminal(&self) -> io::Result<()> {
        self.paused.store(true, Ordering::Release);
        if self.handle.is_none() {
            self.paused_ack.store(true, Ordering::Release);
            self.mark_alive();
            return Ok(());
        }

        let deadline = Instant::now() + TERMINAL_INPUT_CHILD_PAUSE_TIMEOUT;
        while !self.paused_ack.load(Ordering::Acquire) {
            if Instant::now() >= deadline {
                self.paused_ack.store(false, Ordering::Release);
                self.paused.store(false, Ordering::Release);
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "terminal input pump did not pause before launching editor",
                ));
            }
            thread::sleep(TERMINAL_INPUT_CHILD_PAUSE_POLL_INTERVAL);
        }
        self.mark_alive();
        Ok(())
    }

    fn resume_after_child_terminal(&self) {
        self.paused_ack.store(false, Ordering::Release);
        self.paused.store(false, Ordering::Release);
        self.mark_alive();
    }

    /// Replace a wedged pump thread with a freshly spawned one.
    ///
    /// The old thread may be blocked forever inside crossterm's blocking
    /// `event::read` (a stalled Windows console poll, or a Unix tty that
    /// stopped delivering bytes), so it can never be joined. Instead it is
    /// detached: `stop` is flagged and the `JoinHandle` dropped, so if the
    /// thread ever wakes it exits on its own (its send fails once `rx` is
    /// replaced, and the stop flag covers the poll loop).
    fn restart_detached(&mut self) -> io::Result<()> {
        self.detach_current_thread();
        let parts = Self::spawn_parts()?;
        self.install_parts(parts);
        Ok(())
    }

    /// Flag the current pump thread to stop and drop its handle without
    /// joining (the thread may be wedged in a blocking terminal read).
    fn detach_current_thread(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.handle.take();
    }

    /// Adopt freshly spawned pump parts and reset the liveness clock.
    fn install_parts(&mut self, parts: TerminalInputPumpParts) {
        self.rx = parts.rx;
        self.stop = parts.stop;
        self.paused = parts.paused;
        self.paused_ack = parts.paused_ack;
        self.handle = Some(parts.handle);
        self.last_alive_at.set(Instant::now());
    }
}

impl Drop for TerminalInputPump {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            #[cfg(target_os = "windows")]
            {
                drop(handle);
            }
            #[cfg(not(target_os = "windows"))]
            let _ = handle.join();
        }
    }
}

fn engine_drain_budget_exhausted(events_drained: usize, started: Instant, now: Instant) -> bool {
    events_drained >= MAX_ENGINE_EVENTS_PER_DRAIN
        || now.saturating_duration_since(started) >= ENGINE_DRAIN_TIME_BUDGET
}

/// Where a key goes while onboarding owns the screen (#4763).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OnboardingKeyRoute {
    /// Terminate the session. Ctrl+C is unconditional during onboarding.
    Quit,
    /// Hand the key to the provider picker on the view stack.
    ProviderPicker,
    /// Hand the key to the theme picker owning the appearance step (#3937).
    /// Escape belongs to the picker so its revert path runs; the shell must
    /// not pop the modal and strand a previewed-but-unsaved theme.
    ThemePicker,
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

async fn drain_remote_control_events(
    app: &mut App,
    config: &Config,
    engine_handle: &EngineHandle,
) -> Result<bool> {
    let mut changed = false;
    while let Some(event) = app.remote_control.try_next_event() {
        changed = true;
        match event {
            crate::remote_control::RemoteEvent::Notice(message) => {
                app.add_message(HistoryCell::System {
                    content: message.clone(),
                });
                app.status_message = Some(message.clone());
                app.sticky_status =
                    Some(StatusToast::new(message, StatusToastLevel::Warning, None));
            }
            crate::remote_control::RemoteEvent::Connected {
                account_ref,
                runner_id,
                attachment,
                ..
            } => {
                app.remote_control
                    .upload_snapshot(&attachment.run_id, &app.api_messages);
                let status = format!(
                    "REMOTE CONTROL · account {account_ref} · runner {runner_id} · /rc stop returns input here"
                );
                app.add_message(HistoryCell::System {
                    content: format!(
                        "{status}\n\nThe web now owns new prompts and approvals. This terminal remains readable."
                    ),
                });
                app.status_message = Some(status.clone());
                app.sticky_status = Some(StatusToast::new(status, StatusToastLevel::Warning, None));
            }
            crate::remote_control::RemoteEvent::Attachment { attachment } => {
                // Reconnect responses carry the server's current cursor and
                // snapshot receipt. `try_next_event` applies that truth before
                // this handler, so this is either a no-op or one bounded retry.
                app.remote_control
                    .upload_snapshot(&attachment.run_id, &app.api_messages);
            }
            crate::remote_control::RemoteEvent::RuntimeCursor { .. } => {
                // The controller has already retired the acknowledged prefix.
            }
            crate::remote_control::RemoteEvent::Failed(error) => {
                let status = format!(
                    "REMOTE CONTROL LOST · {error} · input stays locked until the server lease expires"
                );
                app.status_message = Some(status.clone());
                app.sticky_status = Some(StatusToast::new(status, StatusToastLevel::Error, None));
            }
            crate::remote_control::RemoteEvent::Stopped => {
                app.sticky_status = None;
                app.status_message =
                    Some("Remote control stopped; this terminal owns input again.".to_string());
            }
            crate::remote_control::RemoteEvent::OwnershipRestored { approvals } => {
                app.sticky_status = None;
                app.status_message = Some(
                    "The remote lease expired safely; this terminal owns input again.".to_string(),
                );
                for approval in approvals {
                    push_approval_request_view(
                        app,
                        &approval.tool_id,
                        &approval.tool_name,
                        &approval.description,
                        &approval.input,
                        &approval.approval_key,
                        approval.intent_summary.as_deref(),
                    );
                }
            }
            crate::remote_control::RemoteEvent::Command {
                run_id,
                seq,
                command,
            } => {
                match app.remote_control.claim_command(&run_id, seq, &command) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(error) => {
                        app.remote_control.acknowledge(
                            &run_id,
                            seq,
                            &command,
                            "failed",
                            Some(error.clone()),
                        );
                        app.remote_control.stop();
                        app.sticky_status = None;
                        app.status_message = Some(error);
                        continue;
                    }
                }
                match command.clone() {
                    crate::remote_control::RemoteCommand::Prompt { turn_id, prompt } => {
                        if app.is_loading || app.dispatch_in_flight {
                            app.remote_control.acknowledge(
                                &run_id,
                                seq,
                                &command,
                                "failed",
                                Some(
                                    "The exact session is already running a turn; no second owner was started."
                                        .to_string(),
                                ),
                            );
                            continue;
                        }
                        app.remote_control
                            .upload_snapshot(&run_id, &app.api_messages);
                        app.remote_control.activate_prompt(&run_id, &turn_id);
                        let message = QueuedMessage::new(prompt, None);
                        app.remote_control.set_applying_remote_command(true);
                        let result = dispatch_user_message_with_recovery(
                            app,
                            config,
                            engine_handle,
                            message,
                            DispatchRecovery::Immediate,
                        )
                        .await;
                        app.remote_control.set_applying_remote_command(false);
                        match result {
                            Ok(()) if app.is_loading || app.dispatch_in_flight => {
                                app.remote_control
                                    .acknowledge(&run_id, seq, &command, "applied", None);
                            }
                            Ok(()) => {
                                app.remote_control.fail_active_dispatch(
                                    "The remote prompt was blocked before dispatch.",
                                );
                                app.remote_control.acknowledge(
                                    &run_id,
                                    seq,
                                    &command,
                                    "failed",
                                    Some(
                                        "The remote prompt was blocked before dispatch."
                                            .to_string(),
                                    ),
                                );
                            }
                            Err(error) => {
                                app.remote_control.fail_active_dispatch(&error.to_string());
                                app.remote_control.acknowledge(
                                    &run_id,
                                    seq,
                                    &command,
                                    "failed",
                                    Some(error.to_string()),
                                );
                            }
                        }
                    }
                    crate::remote_control::RemoteCommand::Approval { gate, approved } => {
                        let Some(tool_id) = app.remote_control.take_pending_approval(&gate) else {
                            app.remote_control.acknowledge(
                                &run_id,
                                seq,
                                &command,
                                "failed",
                                Some("This approval is no longer pending.".to_string()),
                            );
                            continue;
                        };
                        let result = if approved {
                            engine_handle.approve_tool_call(tool_id).await
                        } else {
                            engine_handle.deny_tool_call(tool_id).await
                        };
                        match result {
                            Ok(()) => app
                                .remote_control
                                .acknowledge(&run_id, seq, &command, "applied", None),
                            Err(error) => app.remote_control.acknowledge(
                                &run_id,
                                seq,
                                &command,
                                "failed",
                                Some(error.to_string()),
                            ),
                        }
                    }
                    crate::remote_control::RemoteCommand::Control { .. } => {
                        if !app.remote_control.active_run_matches(&run_id) {
                            app.remote_control.acknowledge(
                                &run_id,
                                seq,
                                &command,
                                "failed",
                                Some("This run no longer owns an active turn.".to_string()),
                            );
                            continue;
                        }
                        engine_handle.cancel();
                        mark_active_turn_cancelled_locally(app);
                        app.remote_control
                            .acknowledge(&run_id, seq, &command, "applied", None);
                    }
                }
            }
        }
    }
    Ok(changed)
}

fn start_remote_control_session(app: &mut App) {
    if app.is_loading {
        app.status_message = Some(
            "Finish or interrupt the current turn before handing this session to the web."
                .to_string(),
        );
        return;
    }
    let session_id = app
        .current_session_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    app.current_session_id = Some(session_id.clone());
    let target_ref = crate::remote_control::target_ref(&app.workspace, &session_id);
    let workspace_label = app
        .workspace
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("Codewhale session")
        .to_string();
    let runtime_commit = option_env!("CODEWHALE_BUILD_COMMIT")
        .unwrap_or("")
        .to_string();
    // The crash-recoverable delivery journal is mandatory outside tests: it is
    // what lets an interrupted session prove which terminal/approval events
    // never reached the account before handing the session back.
    let journal_dir = match codewhale_config::codewhale_home() {
        Ok(home) => home.join("remote-control"),
        Err(_) => {
            let error =
                "Remote control needs a writable Codewhale home directory for its delivery journal."
                    .to_string();
            app.status_message = Some(error.clone());
            app.push_status_toast(error, StatusToastLevel::Error, Some(12_000));
            return;
        }
    };
    match app
        .remote_control
        .start(crate::remote_control::RemoteStart {
            workspace_label,
            target_ref,
            session_id,
            runtime_version: env!("CARGO_PKG_VERSION").to_string(),
            runtime_commit,
            journal_dir: Some(journal_dir),
        }) {
        Ok(()) => {
            let status = app.remote_control.status_line();
            app.status_message = Some(status.clone());
            app.sticky_status = Some(StatusToast::new(status, StatusToastLevel::Warning, None));
        }
        Err(error) => {
            app.status_message = Some(error.clone());
            app.push_status_toast(error, StatusToastLevel::Error, Some(12_000));
        }
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

fn should_show_resume_hint(session_id: Option<&str>) -> bool {
    session_id.is_some_and(|id| !id.trim().is_empty())
}

fn resume_hint_text() -> &'static str {
    "To continue this session, execute codewhale run --continue"
}

fn execute_subagent_observer_hook(
    app: &App,
    event: HookEvent,
    agent_id: &str,
    text_field: &str,
    text: &str,
) -> Result<(), String> {
    if !app.hooks.has_hooks_for_event(event) {
        return Ok(());
    }

    let (preview, truncated) = bounded_subagent_hook_preview(text);
    let context = app.base_hook_context().with_message(&preview);
    let mut payload = serde_json::json!({
        "event": event.as_str(),
        "agent_id": agent_id,
        "session_id": context.session_id.as_deref(),
        "workspace": context.workspace.as_ref().map(|path| path.display().to_string()),
        "mode": context.mode.as_deref(),
        "model": context.model.as_deref(),
        "total_tokens": context.total_tokens,
    });
    if let Some(object) = payload.as_object_mut() {
        object.insert(
            format!("{text_field}_preview"),
            serde_json::Value::String(preview),
        );
        object.insert(
            format!("{text_field}_truncated"),
            serde_json::Value::Bool(truncated),
        );
    }

    if event == HookEvent::SubagentComplete {
        payload["status"] = serde_json::Value::String(
            subagent_completion_status(text).unwrap_or_else(|| "unknown".to_string()),
        );
    }

    app.hooks.submit_json_observer(event, context, payload)
}

fn execute_turn_end_observer_hook(
    app: &App,
    turn: Option<&ActiveTurnMetadata>,
    usage: &Usage,
    billing_surface: Option<&str>,
    duration: Duration,
    error: Option<&str>,
) -> Result<(), String> {
    if !app.hooks.has_hooks_for_event(HookEvent::TurnEnd) {
        return Ok(());
    }

    let metadata = turn_end_observer_metadata(turn);
    let context = app.base_hook_context();
    let payload = crate::hooks::turn_end_payload(TurnEndPayloadInput {
        context: &context,
        created_at: metadata.created_at,
        model_backed: metadata.route.is_some(),
        provider: metadata.route.map(|route| route.provider_identity.as_str()),
        billing_surface: metadata.route.and(billing_surface),
        model: metadata.route.map(|route| route.model.as_str()),
        turn_id: metadata.turn_id.as_ref(),
        status: app.runtime_turn_status.as_deref().unwrap_or("unknown"),
        error,
        duration,
        usage,
        totals: TurnEndTotals {
            session_tokens: app.session.total_tokens,
            conversation_tokens: app.session.total_conversation_tokens,
            input_tokens: app.session.total_input_tokens,
            output_tokens: app.session.total_output_tokens,
        },
        tool_count: app.tool_evidence.len(),
        queued_message_count: app.queued_message_count(),
    });
    app.hooks
        .submit_json_observer(HookEvent::TurnEnd, context, payload)
}

fn surface_observer_hook_submission_failure(app: &mut App, error: String) {
    app.surface_observer_hook_submission_failure(error);
}

struct TurnEndObserverMetadata<'a> {
    turn_id: std::borrow::Cow<'a, str>,
    created_at: chrono::DateTime<chrono::Utc>,
    route: Option<&'a crate::core::events::TurnRoute>,
}

fn turn_end_observer_metadata(turn: Option<&ActiveTurnMetadata>) -> TurnEndObserverMetadata<'_> {
    turn.map_or_else(
        || TurnEndObserverMetadata {
            // Manual compaction, purge, and shell-only completions predate the
            // TurnStarted lifecycle event. Preserve their observer contract
            // with a distinct non-model identity instead of borrowing a stale
            // model turn id.
            turn_id: std::borrow::Cow::Owned(format!("lifecycle_{}", uuid::Uuid::new_v4())),
            created_at: chrono::Utc::now(),
            route: None,
        },
        |turn| TurnEndObserverMetadata {
            turn_id: std::borrow::Cow::Borrowed(&turn.turn_id),
            created_at: turn.created_at,
            route: turn.route.as_ref(),
        },
    )
}

fn bounded_subagent_hook_preview(text: &str) -> (String, bool) {
    if text.len() <= SUBAGENT_HOOK_PREVIEW_LIMIT {
        return (text.to_string(), false);
    }
    let safe_end = text
        .char_indices()
        .take_while(|(idx, ch)| idx + ch.len_utf8() <= SUBAGENT_HOOK_PREVIEW_LIMIT)
        .last()
        .map(|(idx, ch)| idx + ch.len_utf8())
        .unwrap_or(0);
    (format!("{}...[truncated]", &text[..safe_end]), true)
}

fn subagent_completion_status(result: &str) -> Option<String> {
    const START: &str = "<codewhale:subagent.done>";
    const END: &str = "</codewhale:subagent.done>";

    if let Some(start) = result.find(START).map(|idx| idx + START.len())
        && let Some(end) = result[start..].find(END).map(|idx| idx + start)
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&result[start..end])
        && let Some(status) = value.get("status").and_then(serde_json::Value::as_str)
    {
        return Some(status.to_string());
    }

    let summary = result.lines().find_map(|line| {
        let trimmed = line.trim();
        (!trimmed.is_empty()).then_some(trimmed)
    })?;
    let summary = summary.to_ascii_lowercase();
    if matches!(summary.as_str(), "cancelled" | "canceled")
        || summary.starts_with("cancelled:")
        || summary.starts_with("canceled:")
    {
        Some("cancelled".to_string())
    } else if summary == "failed" || summary.starts_with("failed:") {
        Some("failed".to_string())
    } else if summary == "interrupted" || summary.starts_with("interrupted:") {
        Some("interrupted".to_string())
    } else {
        None
    }
}

fn subagent_failure_notice(result: &str) -> Option<String> {
    const START: &str = "<codewhale:subagent.done>";
    const END: &str = "</codewhale:subagent.done>";
    let start = result.find(START)? + START.len();
    let end = result[start..].find(END)? + start;
    let value = serde_json::from_str::<serde_json::Value>(&result[start..end]).ok()?;
    (value.get("event").and_then(serde_json::Value::as_str) == Some("subagent.failed"))
        .then(|| {
            let name = value
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            let agent_id = value
                .get("agent_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            let class = value
                .get("failure_class")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unavailable");
            let steps = value
                .get("steps")
                .and_then(serde_json::Value::as_u64)
                .map_or_else(|| "?".to_string(), |steps| steps.to_string());
            let elapsed_ms = value
                .get("elapsed_ms")
                .and_then(serde_json::Value::as_u64)
                .map_or_else(|| "?".to_string(), |elapsed| elapsed.to_string());
            let transcript_handle = value
                .get("transcript_handle")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unavailable");
            format!(
                "{name} ({agent_id}) · {class} · {steps} steps · {elapsed_ms} ms · inspect {transcript_handle}"
            )
        })
}

fn subagent_status_from_completion_result(result: &str) -> SubAgentStatus {
    let reason = result
        .lines()
        .find_map(|line| {
            let trimmed = line.trim();
            (!trimmed.is_empty() && !trimmed.starts_with("<codewhale:subagent.done>"))
                .then_some(trimmed.to_string())
        })
        .unwrap_or_else(|| "sub-agent finished".to_string());
    match subagent_completion_status(result).as_deref() {
        Some("completed") => SubAgentStatus::Completed,
        Some("cancelled" | "canceled") => SubAgentStatus::Cancelled,
        Some("failed") => SubAgentStatus::Failed(reason),
        Some("interrupted") => SubAgentStatus::Interrupted(reason),
        Some("budget_exhausted") => SubAgentStatus::BudgetExhausted,
        _ => SubAgentStatus::Completed,
    }
}

struct TerminalCleanupGuard {
    use_alt_screen: bool,
    use_mouse_capture: bool,
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
        if self.use_alt_screen {
            let _ = execute!(stdout, LeaveAlternateScreen);
        }
        if self.use_mouse_capture {
            let _ = execute!(stdout, DisableMouseCapture);
        }
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

    let prompt = build_app_system_prompt_with_goal(app, config, app.hunt.quarry.as_deref());
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

async fn refresh_active_task_panel(app: &mut App, task_manager: &SharedTaskManager) -> bool {
    let tasks = task_manager.list_tasks(None).await;
    let previously_active_durable_ids = app
        .task_panel
        .iter()
        .filter(|entry| matches!(entry.status.as_str(), "queued" | "running"))
        .map(|entry| entry.id.as_str())
        .collect::<HashSet<_>>();
    let durable_background_completed = newly_completed_id(
        previously_active_durable_ids,
        tasks
            .iter()
            .filter(|task| task.status == TaskStatus::Completed)
            .map(|task| task.id.as_str()),
    );
    let mut lifecycle_changed = false;
    if let (Some(work), Some(session_id)) = (
        app.runtime_services.work.as_ref(),
        app.current_session_id.as_deref(),
    ) {
        for task in &tasks {
            let external = format!("task:{}", task.id);
            if !work.has_operation_binding(Some(session_id), &external) {
                continue;
            }
            match work.reconcile_operation(
                session_id,
                task_owner_snapshot(
                    &task.id,
                    task.status,
                    task.lifecycle_seq,
                    task.created_at,
                    task.started_at,
                    task.ended_at,
                ),
            ) {
                Ok(changed) => lifecycle_changed |= changed,
                Err(err) => {
                    tracing::warn!(task_id = %task.id, error = %err, "failed to reconcile durable task lifecycle");
                }
            }
        }
    }
    if lifecycle_changed && let Err(err) = persist_pending_work_checkpoint(app).await {
        tracing::warn!(error = %err, "durable task lifecycle checkpoint remains pending");
    }
    let session_started_at = app.session_started_at;
    let mut entries: Vec<TaskPanelEntry> =
        select_work_sidebar_tasks(tasks, session_started_at, app.current_session_id.as_deref())
            .into_iter()
            .map(task_summary_to_panel_entry)
            .collect();

    entries.extend(active_rlm_task_entries(app));

    // #3804: this is a render-only read of shell jobs and must not block the
    // async UI loop on the shell manager's std::sync Mutex. Use try_lock; on
    // contention, retain the previous frame's background shell entries so
    // running shells don't flicker out of the Work panel. Shell ownership,
    // cancellation, approval state, and output capture never depend on this
    // refresh succeeding.
    let prev_shell_entries: Vec<TaskPanelEntry> = app
        .task_panel
        .iter()
        .filter(|entry| matches!(entry.kind, TaskPanelEntryKind::Background))
        .cloned()
        .collect();
    let prev_shell_ids = prev_shell_entries
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<HashSet<_>>();
    let (shell_entries, shell_background_completed): (Vec<TaskPanelEntry>, bool) =
        match app.runtime_services.shell_manager.as_ref() {
            Some(shell_mgr) => match shell_mgr.try_lock() {
                Ok(mut mgr) => {
                    let jobs = mgr.list_jobs();
                    let completed = newly_completed_id(
                        prev_shell_ids.iter().map(String::as_str).collect(),
                        jobs.iter()
                            .filter(|job| {
                                matches!(job.status, crate::tools::shell::ShellStatus::Completed)
                            })
                            .map(|job| job.id.as_str()),
                    );
                    let entries = jobs
                        .into_iter()
                        .filter(|job| {
                            matches!(job.status, crate::tools::shell::ShellStatus::Running)
                        })
                        .map(|job| TaskPanelEntry {
                            id: job.id,
                            status: "running".to_string(),
                            prompt_summary: format!("shell: {}", job.command),
                            duration_ms: Some(job.elapsed_ms),
                            kind: TaskPanelEntryKind::Background,
                            stale: job.stale,
                            elapsed_since_output_ms: job.elapsed_since_output_ms,
                            owner_agent_id: job.owner_agent_id,
                            owner_agent_name: job.owner_agent_name,
                            current_tool: None,
                            role: None,
                            files_touched: 0,
                        })
                        .collect();
                    (entries, completed)
                }
                // Contended: keep the last known snapshot rather than blocking.
                Err(_) => (prev_shell_entries, false),
            },
            None => (Vec::new(), false),
        };
    entries.extend(shell_entries);

    // Report whether anything visible changed so the idle tick can skip the
    // redraw: an unconditional 2.5 s repaint kept the app from ever going
    // quiescent (#3757).
    let changed = lifecycle_changed || app.task_panel != entries;
    app.task_panel = entries;
    let tip_shown = (durable_background_completed || shell_background_completed)
        && app.maybe_show_behavioral_tip(
            crate::tui::behavioral_tips::BehavioralTip::BackgroundJobReceipt,
        );
    changed || tip_shown
}

fn newly_completed_id<'a>(
    previously_active_ids: HashSet<&'a str>,
    completed_ids: impl IntoIterator<Item = &'a str>,
) -> bool {
    completed_ids
        .into_iter()
        .any(|id| previously_active_ids.contains(id))
}

fn refresh_shell_exec_live_output(app: &mut App) -> bool {
    let Some(shell_mgr) = app.runtime_services.shell_manager.as_ref().cloned() else {
        return false;
    };
    // #3804: render-only read — try_lock so a contended shell Mutex can never
    // block the async UI loop; skip this frame's live-output update on
    // contention (the next refresh picks it up).
    let jobs = {
        let Ok(mut mgr) = shell_mgr.try_lock() else {
            return false;
        };
        mgr.list_jobs()
            .into_iter()
            .map(|job| (job.id.clone(), job))
            .collect::<std::collections::HashMap<_, _>>()
    };
    let mut changed = false;
    for index in 0..app.virtual_cell_count() {
        let Some(ShellExecLiveUpdate {
            task_id,
            status: next_status,
            output: next_live,
            duration_ms: next_duration,
            finalized,
            stale_elapsed_since_output_ms,
        }) = shell_exec_live_update(app, index, &jobs)
        else {
            continue;
        };
        let Some(HistoryCell::Tool(ToolCell::Exec(exec))) = app.cell_at_virtual_index_mut(index)
        else {
            continue;
        };
        if exec.output.is_some() || exec.shell_task_id.as_deref() != Some(task_id.as_str()) {
            continue;
        }
        exec.status = next_status;
        exec.duration_ms = Some(next_duration);
        exec.stale_elapsed_since_output_ms = stale_elapsed_since_output_ms;
        if finalized {
            exec.output = next_live;
            exec.output_summary = exec
                .output
                .as_deref()
                .map(super::history::summarize_tool_output);
            exec.live_output = None;
            exec.stale_elapsed_since_output_ms = None;
        } else {
            exec.live_output = next_live;
        }
        changed = true;
    }
    changed
}

struct ShellExecLiveUpdate {
    task_id: String,
    status: ToolStatus,
    output: Option<String>,
    duration_ms: u64,
    finalized: bool,
    stale_elapsed_since_output_ms: Option<u64>,
}

fn shell_exec_live_update(
    app: &App,
    index: usize,
    jobs: &std::collections::HashMap<String, ShellJobSnapshot>,
) -> Option<ShellExecLiveUpdate> {
    let HistoryCell::Tool(ToolCell::Exec(exec)) = app.cell_at_virtual_index(index)? else {
        return None;
    };
    if exec.output.is_some() {
        return None;
    }
    let task_id = exec.shell_task_id.as_deref()?;
    let Some(job) = jobs.get(task_id) else {
        return Some(ShellExecLiveUpdate {
            task_id: task_id.to_string(),
            status: ToolStatus::Failed,
            output: detached_shell_job_output(task_id, exec),
            duration_ms: exec.duration_ms.unwrap_or_default(),
            finalized: true,
            stale_elapsed_since_output_ms: None,
        });
    };
    let next_status = shell_job_tool_status(&job.status);
    let next_live = shell_job_live_output(job).or_else(|| exec.live_output.clone());
    let finalized = !matches!(job.status, ShellStatus::Running);
    let stale_elapsed_since_output_ms = if matches!(job.status, ShellStatus::Running) && job.stale {
        Some(job.elapsed_since_output_ms.unwrap_or(0))
    } else {
        None
    };
    if exec.status == next_status
        && exec.live_output == next_live
        && exec.duration_ms == Some(job.elapsed_ms)
        && exec.stale_elapsed_since_output_ms == stale_elapsed_since_output_ms
    {
        return None;
    }
    Some(ShellExecLiveUpdate {
        task_id: task_id.to_string(),
        status: next_status,
        output: next_live,
        duration_ms: job.elapsed_ms,
        finalized,
        stale_elapsed_since_output_ms,
    })
}

fn detached_shell_job_output(task_id: &str, exec: &ExecCell) -> Option<String> {
    let mut output = exec.live_output.clone().unwrap_or_default();
    if !output.trim().is_empty() {
        output.push_str("\n\n");
    }
    output.push_str(&format!(
        "Shell job `{task_id}` is no longer attached to this TUI session."
    ));
    Some(output)
}

fn shell_job_tool_status(status: &ShellStatus) -> ToolStatus {
    match status {
        ShellStatus::Running => ToolStatus::Running,
        ShellStatus::Completed => ToolStatus::Success,
        ShellStatus::Failed | ShellStatus::Killed | ShellStatus::TimedOut => ToolStatus::Failed,
    }
}

fn shell_job_live_output(job: &ShellJobSnapshot) -> Option<String> {
    match (job.stdout_tail.is_empty(), job.stderr_tail.is_empty()) {
        (true, true) => None,
        (false, true) => Some(job.stdout_tail.clone()),
        (true, false) => Some(format!("STDERR:\n{}", job.stderr_tail)),
        (false, false) => Some(format!(
            "{}\n\nSTDERR:\n{}",
            job.stdout_tail, job.stderr_tail
        )),
    }
}

fn active_rlm_task_entries(app: &App) -> Vec<TaskPanelEntry> {
    let Some(active) = app.active_cell.as_ref() else {
        return Vec::new();
    };
    let duration_ms = app
        .turn_started_at
        .map(|started| u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
    active
        .entries()
        .iter()
        .enumerate()
        .filter_map(|(idx, entry)| {
            let HistoryCell::Tool(ToolCell::Generic(generic)) = entry else {
                return None;
            };
            if !matches!(
                generic.name.as_str(),
                "rlm_open" | "rlm_eval" | "rlm_configure" | "rlm_close" | "rlm"
            ) || generic.status != ToolStatus::Running
            {
                return None;
            }
            let summary = generic
                .input_summary
                .as_deref()
                .filter(|summary| !summary.trim().is_empty())
                .unwrap_or("running chunked analysis");
            Some(TaskPanelEntry {
                id: format!("rlm-{}", idx + 1),
                status: "running".to_string(),
                prompt_summary: format!("RLM: {summary}"),
                duration_ms,
                kind: TaskPanelEntryKind::Background,
                stale: false,
                elapsed_since_output_ms: None,
                owner_agent_id: None,
                owner_agent_name: None,
                current_tool: None,
                role: None,
                files_touched: 0,
            })
        })
        .collect()
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
            HistoryCell::Tool(tool) => tool_cell_is_running(tool),
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

const INITIAL_PROMPT_DEFERRED_STATUS: &str = "Initial prompt ready; complete setup to send it";

fn paused_quarry_title(quarry: &str) -> &str {
    quarry
        .split(['\n', '\r'])
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .unwrap_or("the paused command")
}

fn is_resume_message(message: &str) -> bool {
    let words: Vec<String> = message
        .to_ascii_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect();
    if words.is_empty() {
        return false;
    }
    let text = words.join(" ");
    let has_resume_verb = words
        .iter()
        .any(|word| matches!(word.as_str(), "continue" | "resume"));
    if !has_resume_verb {
        return false;
    }

    let blockers = [
        "do not continue",
        "do not resume",
        "don t continue",
        "don t resume",
        "dont continue",
        "dont resume",
        "not continue",
        "not resume",
        "continue yet",
        "resume yet",
        "will continue",
        "will resume",
        "continue tomorrow",
        "resume tomorrow",
        "continue later",
        "resume later",
    ];
    if blockers.iter().any(|blocker| text.contains(blocker)) {
        return false;
    }
    if matches!(
        words.first().map(String::as_str),
        Some("how" | "what" | "when" | "where" | "why")
    ) {
        return false;
    }

    if words.len() == 1 {
        return true;
    }

    let context_words = [
        "please", "now", "paused", "pause", "command", "task", "work", "request", "goal",
        "previous", "last", "same", "it", "that", "this", "go", "ahead",
    ];
    if words
        .iter()
        .any(|word| context_words.contains(&word.as_str()))
    {
        return true;
    }

    text.starts_with("can you continue")
        || text.starts_with("can you resume")
        || text.starts_with("could you continue")
        || text.starts_with("could you resume")
}

fn paused_command_note(title: &str, resume: bool) -> String {
    let instruction = if resume {
        "The user is resuming that paused command. Continue the paused command."
    } else {
        "The user is not resuming that paused command. Answer only the new message and do not continue the paused command."
    };
    format!(
        "\n\nCodewhale paused custom slash command context:\n\
Paused custom slash command: {title}\n\
Paused command: {title}\n\
{instruction}"
    )
}

#[derive(Debug, Clone)]
enum PausedCommandDispatch {
    None,
    ClearWithoutQuarry,
    Resume { quarry: String, note: String },
    Detach { note: String },
}

impl PausedCommandDispatch {
    fn note(&self) -> Option<&str> {
        match self {
            Self::Resume { note, .. } | Self::Detach { note } => Some(note),
            Self::None | Self::ClearWithoutQuarry => None,
        }
    }

    fn goal_objective(&self, app: &App) -> Option<String> {
        match self {
            Self::Resume { quarry, .. } => Some(quarry.clone()),
            Self::Detach { .. } | Self::ClearWithoutQuarry => None,
            Self::None => app.hunt.quarry.clone(),
        }
    }

    fn apply(self, app: &mut App, engine_handle: &EngineHandle) {
        engine_handle.set_paused(false);
        match self {
            Self::None => {}
            Self::ClearWithoutQuarry => {
                app.paused = false;
                app.pausable = false;
            }
            Self::Resume { quarry, .. } => {
                app.paused = false;
                app.paused_quarry = None;
                app.hunt.quarry = Some(quarry);
                app.pausable = true;
            }
            Self::Detach { .. } => {
                app.paused = false;
                app.hunt.quarry = None;
                app.hunt.tokens_used = 0;
                app.hunt.time_used_seconds = 0;
                app.hunt.continuation_count = 0;
            }
        }
    }
}

fn plan_paused_command_message(app: &App, user_message: &str) -> PausedCommandDispatch {
    if !app.paused && app.paused_quarry.is_none() {
        return PausedCommandDispatch::None;
    }

    let Some(quarry) = app
        .paused_quarry
        .clone()
        .or_else(|| app.hunt.quarry.clone())
    else {
        return PausedCommandDispatch::ClearWithoutQuarry;
    };
    let title = paused_quarry_title(&quarry).to_string();
    if is_resume_message(user_message) {
        PausedCommandDispatch::Resume {
            quarry,
            note: paused_command_note(&title, true),
        }
    } else {
        PausedCommandDispatch::Detach {
            note: paused_command_note(&title, false),
        }
    }
}

fn pause_pausable_command(app: &mut App, engine_handle: &EngineHandle) {
    app.paused_quarry = app
        .paused_quarry
        .clone()
        .or_else(|| app.hunt.quarry.clone());
    app.hunt.quarry = None;
    app.hunt.tokens_used = 0;
    app.hunt.time_used_seconds = 0;
    app.hunt.continuation_count = 0;
    app.paused = true;
    app.pausable = true;
    engine_handle.set_paused(true);
    app.status_message = Some(
        "Request paused. Send `continue` or `resume` to continue, or Esc to cancel.".to_string(),
    );
}

fn clear_paused_command_state(app: &mut App, engine_handle: &EngineHandle) {
    app.pausable = false;
    app.paused = false;
    app.paused_quarry = None;
    engine_handle.set_paused(false);
}

fn app_scoped_runtime_config(app: &App, config: &Config) -> (ProviderIdentity, Config) {
    let identity = ProviderIdentity {
        provider: app.api_provider,
        key: app.provider_identity_for_persistence().to_string(),
        exact_id: app.provider_id_for_persistence().map(str::to_string),
    };
    let mut scoped = config.clone();
    scoped.scope_to_provider_identity(&identity);
    (identity, scoped)
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum DispatchRecovery {
    /// Normal immediate composer submit: restore the composer on failure.
    Immediate,
    /// A queued follow-up that was being edited in the composer.
    Draft,
    /// A queued follow-up pulled from the queue; re-insert at the prior index.
    Queued { restore_index: Option<usize> },
    /// Initial `--prompt` / startup input.
    Initial,
}

/// Snapshot of App state taken before the sync prepare phase so a failed
/// dispatch can roll back the optimistic history/api_messages changes.
#[derive(Debug, Clone)]
struct UserDispatchSnapshot {
    is_loading: bool,
    runtime_turn_status: Option<String>,
    receipt_text: Option<String>,
    receipt_started_at: Option<Instant>,
    tool_evidence: Vec<ToolEvidence>,
    history_len: usize,
    history_revisions_len: usize,
    history_version: u64,
    api_messages_len: usize,
    last_send_at: Option<Instant>,
}

/// Data captured synchronously before the async dispatch phase. All values are
/// Send so the spawned task can resolve routes and send without holding `&mut App`.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone)]
pub(crate) struct UserDispatchPrepare {
    message: QueuedMessage,
    content: String,
    references: Vec<ContextReference>,
    paused_dispatch: PausedCommandDispatch,
    app_route_identity: ProviderIdentity,
    route_config: Config,
    goal_objective: Option<String>,
    goal_status: GoalStatus,
    goal_token_budget: Option<u32>,
    mode: AppMode,
    api_provider: ApiProvider,
    app_model: String,
    auto_model: bool,
    reasoning_effort: ReasoningEffort,
    allow_shell: bool,
    trust_mode: bool,
    auto_approve: bool,
    approval_mode: ApprovalMode,
    translation_enabled: bool,
    allowed_tools: Option<Vec<String>>,
    hook_executor: Option<Arc<HookExecutor>>,
    verbosity: Option<String>,
    provenance: UserInputProvenance,
    auto_router_context: String,
    should_auto_resolve: bool,
    auto_compact_user_configured: bool,
    auto_compact: bool,
    auto_compact_threshold_percent: f64,
    snapshot: UserDispatchSnapshot,
    message_index: usize,
    history_cell: usize,
}

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

fn goal_status_from_snapshot(snapshot: &GoalSnapshot) -> Option<GoalStatus> {
    match snapshot.status.trim() {
        "active" => Some(GoalStatus::Active),
        "paused" => Some(GoalStatus::Paused),
        "complete" => Some(GoalStatus::Complete),
        "blocked" => Some(GoalStatus::Blocked),
        _ => None,
    }
}

fn is_model_visible_tool_call(id: &str) -> bool {
    !id.starts_with(USER_SHELL_TOOL_ID_PREFIX)
}

/// Queue a live compaction update without waiting on the engine mailbox.
///
/// Config edits are valid while a turn is streaming, but awaiting a bounded
/// engine mailbox from the UI event loop can make the whole TUI appear frozen
/// when the turn is busy. A dropped refresh is safe: the next turn rebuilds
/// its compaction config from `App`, and the status message tells the user
/// whether the update was queued or deferred.
fn try_apply_model_and_compaction_update(
    engine_handle: &EngineHandle,
    compaction: crate::compaction::CompactionConfig,
    mode: AppMode,
    route_limits: Option<codewhale_config::route::RouteLimits>,
) -> bool {
    if engine_handle
        .try_send(Op::SetModel {
            model: compaction.model.clone(),
            mode,
            route_limits,
        })
        .is_err()
    {
        return false;
    }
    engine_handle
        .try_send(Op::SetCompaction { config: compaction })
        .is_ok()
}

fn set_explicit_compaction_status(
    app: &mut App,
    text: String,
    level: StatusToastLevel,
    sticky: bool,
) {
    app.status_message = Some(text.clone());
    // This lifecycle reducer assigns the semantic level explicitly. Mark the
    // legacy status bridge as synchronized so it cannot add a second,
    // keyword-classified toast with a different level on the next frame.
    app.last_status_message_seen = Some(text.clone());
    if sticky {
        app.set_sticky_status(text, level, Some(App::STICKY_ERROR_TTL_MS));
    } else {
        app.push_status_toast(text, level, Some(5_000));
    }
}

/// Queue manual compaction without ever awaiting the bounded engine mailbox
/// from the terminal event loop.
///
/// During an active turn, a successful send is intentionally deferred until
/// the engine returns to its outer operation loop. Full and closed mailboxes
/// are rejected immediately with an actionable receipt, so `/compact` cannot
/// freeze keyboard input or rendering.
pub(crate) fn try_queue_manual_compaction(
    app: &mut App,
    config: &Config,
    engine_handle: &EngineHandle,
    focus: Option<String>,
) {
    if app.is_compacting || app.manual_compaction_queued {
        let text = app
            .tr(MessageId::ContextCompactionAlreadyRunning)
            .into_owned();
        add_compaction_receipt(app, &text);
        set_explicit_compaction_status(app, text, StatusToastLevel::Warning, false);
        return;
    }

    let route = match validated_app_runtime_route(app, config) {
        Ok(route) => route,
        Err(error) => {
            let text = app
                .tr(MessageId::ContextCompactionRouteInvalid)
                .replace("{error}", &error.to_string());
            add_compaction_receipt(app, &text);
            set_explicit_compaction_status(app, text, StatusToastLevel::Error, true);
            return;
        }
    };
    let mut compaction = compaction_for_validated_route(app, &route);
    compaction.focus = focus.clone();
    let op = Op::CompactContext {
        route: Box::new(route.into_resolved()),
        compaction: Box::new(compaction),
    };

    match engine_handle.try_send(op) {
        Ok(()) => {
            app.manual_compaction_queued = true;
            let id = if app.is_loading {
                MessageId::ContextCompactionQueued
            } else {
                MessageId::ContextManualCompacting
            };
            let text = app.tr(id).into_owned();
            // Queued-behind-a-turn is a state the user must be able to find
            // again after the 5s toast: leave it in the transcript too.
            if app.is_loading {
                add_compaction_receipt(app, &text);
            }
            set_explicit_compaction_status(app, text, StatusToastLevel::Info, false);
        }
        Err(error) => {
            let full = error
                .downcast_ref::<tokio::sync::mpsc::error::TrySendError<Op>>()
                .is_some_and(|send_error| {
                    matches!(send_error, tokio::sync::mpsc::error::TrySendError::Full(_))
                });
            if full {
                // A saturated mailbox is a timing accident of the active turn,
                // not a user error. Queue client-side and let the event loop
                // retry once the engine drains a slot; the user sees the same
                // queued receipt as the ordinary behind-a-turn path.
                app.manual_compaction_queued = true;
                app.deferred_manual_compaction = Some(focus);
                let text = app.tr(MessageId::ContextCompactionQueued).into_owned();
                add_compaction_receipt(app, &text);
                set_explicit_compaction_status(app, text, StatusToastLevel::Info, false);
            } else {
                let text = app.tr(MessageId::ContextCompactionQueueClosed).into_owned();
                add_compaction_receipt(app, &text);
                set_explicit_compaction_status(app, text, StatusToastLevel::Error, true);
            }
        }
    }
}

/// Retry a manual compaction that was deferred by a full engine mailbox.
///
/// Called once per event-loop iteration. Silent by design: the queued receipt
/// was already written when the request was deferred, a still-full mailbox
/// just waits for the next iteration, and a compaction that started or
/// settled in the meantime supersedes the request entirely (handled by
/// `apply_compaction_started`/`settle_compaction`).
pub(crate) fn flush_deferred_manual_compaction(
    app: &mut App,
    config: &Config,
    engine_handle: &EngineHandle,
) {
    if app.deferred_manual_compaction.is_none() || app.is_compacting {
        return;
    }
    let route = match validated_app_runtime_route(app, config) {
        Ok(route) => route,
        Err(error) => {
            app.deferred_manual_compaction = None;
            app.manual_compaction_queued = false;
            let text = app
                .tr(MessageId::ContextCompactionRouteInvalid)
                .replace("{error}", &error.to_string());
            add_compaction_receipt(app, &text);
            set_explicit_compaction_status(app, text, StatusToastLevel::Error, true);
            return;
        }
    };
    let focus = app.deferred_manual_compaction.clone().unwrap_or_default();
    let mut compaction = compaction_for_validated_route(app, &route);
    compaction.focus = focus;
    let op = Op::CompactContext {
        route: Box::new(route.into_resolved()),
        compaction: Box::new(compaction),
    };
    match engine_handle.try_send(op) {
        Ok(()) => {
            app.deferred_manual_compaction = None;
        }
        Err(error) => {
            let full = error
                .downcast_ref::<tokio::sync::mpsc::error::TrySendError<Op>>()
                .is_some_and(|send_error| {
                    matches!(send_error, tokio::sync::mpsc::error::TrySendError::Full(_))
                });
            if !full {
                app.deferred_manual_compaction = None;
                app.manual_compaction_queued = false;
                let text = app.tr(MessageId::ContextCompactionQueueClosed).into_owned();
                add_compaction_receipt(app, &text);
                set_explicit_compaction_status(app, text, StatusToastLevel::Error, true);
            }
        }
    }
}

pub(crate) fn apply_compaction_started(app: &mut App, id: String, auto: bool) {
    if !auto {
        app.manual_compaction_queued = false;
    }
    // A compaction is running; a deferred manual request is now redundant.
    // Dropping it must also release the queued flag when the running pass is
    // automatic, or `/compact` would report "already in progress" forever.
    if app.deferred_manual_compaction.take().is_some() && auto {
        app.manual_compaction_queued = false;
    }
    app.active_compaction = Some(ActiveCompaction { id, auto });
    app.is_compacting = true;
    let message_id = if auto {
        MessageId::ContextAutoCompacting
    } else {
        MessageId::ContextManualCompacting
    };
    let text = app.tr(message_id).into_owned();
    set_explicit_compaction_status(app, text, StatusToastLevel::Info, false);
}

/// Clear the compaction-in-flight state for a terminal lifecycle event.
///
/// An exact id match clears normally. A terminal event with NO tracked
/// compaction is still authoritative (the started event can be lost to a
/// dropped drain or session switch): without this, `is_compacting`/
/// `manual_compaction_queued` stayed latched and every later `/compact` was
/// silently rejected as "already in progress". A stale event while a NEWER
/// compaction is live must not clear it (or report anything) — that live
/// pass gets its own terminal event. Returns whether the event settled.
fn settle_compaction(app: &mut App, id: &str, auto: bool) -> bool {
    if app
        .active_compaction
        .as_ref()
        .is_some_and(|active| active.id != id || active.auto != auto)
    {
        return false;
    }
    app.active_compaction = None;
    app.is_compacting = false;
    if !auto {
        app.manual_compaction_queued = false;
    }
    // A settled pass makes a still-deferred manual request redundant (the
    // context was just compacted). Dropping it releases the queued flag so a
    // later `/compact` is not rejected as "already in progress".
    if app.deferred_manual_compaction.take().is_some() {
        app.manual_compaction_queued = false;
    }
    true
}

/// Durable transcript receipt for a compaction outcome.
///
/// Outcome feedback used to be toast-only, and the engine emits
/// `TurnComplete` immediately after the compaction event — both land in the
/// same UI drain batch, so the turn's "done" status replaced the completion
/// toast before a single frame was drawn. `/compact` looked like a no-op
/// even when the summary committed (the v0.9.6 release blocker).
fn add_compaction_receipt(app: &mut App, message: &str) {
    app.add_message(HistoryCell::System {
        content: message.to_string(),
    });
}

pub(crate) fn apply_compaction_completed(app: &mut App, id: &str, auto: bool, message: String) {
    if settle_compaction(app, id, auto) {
        add_compaction_receipt(app, &message);
        set_explicit_compaction_status(app, message, StatusToastLevel::Success, false);
    }
}

pub(crate) fn apply_compaction_failed(app: &mut App, id: &str, auto: bool, message: String) {
    if settle_compaction(app, id, auto) {
        add_compaction_receipt(app, &message);
        set_explicit_compaction_status(app, message, StatusToastLevel::Error, true);
    }
}

#[cfg(test)]
mod config_update_tests {
    use super::*;
    use crate::core::engine::mock_engine_handle;
    use crate::core::ops::Op;

    #[tokio::test]
    async fn live_compaction_update_queues_without_waiting_on_engine() {
        let mut mock = mock_engine_handle();
        let compaction = crate::compaction::CompactionConfig {
            enabled: false,
            token_threshold: 123,
            model: "deepseek-v4-flash".to_string(),
            effective_context_window: Some(128_000),
            cache_summary: true,
            focus: None,
            runtime_cost_owner: None,
            workspace: None,
            image_input: crate::model_profile::SupportState::Unknown,
        };

        assert!(try_apply_model_and_compaction_update(
            &mock.handle,
            compaction.clone(),
            AppMode::Agent,
            None,
        ));

        assert!(matches!(
            mock.rx_op.recv().await,
            Some(Op::SetModel {
                model,
                mode: AppMode::Agent,
                route_limits: None,
            }) if model == compaction.model
        ));
        assert!(matches!(
            mock.rx_op.recv().await,
            Some(Op::SetCompaction { config }) if config == compaction
        ));
    }
}

async fn drain_web_config_events(
    web_config_session: &mut Option<WebConfigSession>,
    app: &mut App,
    config: &mut Config,
    engine_handle: &EngineHandle,
) -> bool {
    let Some(session) = web_config_session.as_mut() else {
        return true;
    };

    let mut keep_session = true;
    while let Ok(event) = session.receiver.try_recv() {
        match event {
            WebConfigSessionEvent::Draft(doc) => {
                match config_ui::apply_document(doc, app, config, false) {
                    Ok(outcome) if outcome.changed => {
                        if outcome.requires_engine_sync {
                            apply_model_and_compaction_update(
                                engine_handle,
                                app.compaction_config(),
                                app.mode,
                                app.active_route_limits,
                            )
                            .await;
                        }
                        app.status_message = Some(format!(
                            "Web config draft applied: {}",
                            outcome.final_message
                        ));
                    }
                    Ok(_) => {}
                    Err(err) => {
                        app.add_message(HistoryCell::System {
                            content: format!("Web config draft apply failed: {err}"),
                        });
                    }
                }
            }
            WebConfigSessionEvent::Committed(doc) => {
                keep_session = false;
                match config_ui::apply_document(doc, app, config, true) {
                    Ok(outcome) => {
                        if outcome.requires_engine_sync {
                            apply_model_and_compaction_update(
                                engine_handle,
                                app.compaction_config(),
                                app.mode,
                                app.active_route_limits,
                            )
                            .await;
                        }
                        app.add_message(HistoryCell::System {
                            content: outcome.final_message.clone(),
                        });
                        app.status_message = Some(outcome.final_message);
                    }
                    Err(err) => {
                        app.add_message(HistoryCell::System {
                            content: format!("Web config commit failed: {err}"),
                        });
                    }
                }
            }
            WebConfigSessionEvent::Failed(err) => {
                keep_session = false;
                app.add_message(HistoryCell::System {
                    content: format!("Web config session failed: {err}"),
                });
            }
        }
    }

    keep_session
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
mod event_loop;
mod handlers;

pub(crate) use apply::*;
pub(crate) use event_loop::*;
pub(crate) use handlers::*;
// The crate-wide glob would otherwise narrow this to `pub(crate)`; `tui/mod.rs`
// re-exports it as the binary's entry point.
pub use event_loop::run_tui;

mod dispatch;
mod motion;
mod release_check;
mod terminal;

pub(crate) use dispatch::*;
pub(crate) use motion::*;
pub(crate) use release_check::*;
pub(crate) use terminal::*;

mod frame;
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
    paused_quarry: Option<String>,
    quarry: Option<String>,
    tokens_used: u64,
    time_used_seconds: u64,
    continuation_count: u32,
}

fn reject_local_input_while_remote(app: &mut App, input: &str) -> bool {
    if !app.remote_control.blocks_local_input() || is_remote_control_command(input) {
        return false;
    }
    app.input = input.to_string();
    app.cursor_position = app.input.chars().count();
    let status = "Web remote control owns prompts. Use /rc stop to return input to this terminal."
        .to_string();
    app.status_message = Some(status.clone());
    app.push_status_toast(status, StatusToastLevel::Warning, Some(6_000));
    true
}

fn is_remote_control_command(input: &str) -> bool {
    input.split_whitespace().next().is_some_and(|value| {
        value.eq_ignore_ascii_case("/rc") || value.eq_ignore_ascii_case("/remote-control")
    })
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

struct RuntimePresetFileSnapshot {
    path: PathBuf,
    contents: Option<Vec<u8>>,
}

impl RuntimePresetFileSnapshot {
    fn capture(path: PathBuf) -> Result<Self> {
        let contents = match std::fs::read(&path) {
            Ok(contents) => Some(contents),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to snapshot {}", path.display()));
            }
        };
        Ok(Self { path, contents })
    }

    fn restore(&self) -> Result<()> {
        match &self.contents {
            Some(contents) => crate::utils::write_atomic(&self.path, contents)
                .with_context(|| format!("failed to restore {}", self.path.display())),
            None => match std::fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => {
                    Err(error).with_context(|| format!("failed to remove {}", self.path.display()))
                }
            },
        }
    }
}

fn runtime_preset_error_with_rollback(
    error: anyhow::Error,
    snapshots: &[&RuntimePresetFileSnapshot],
) -> anyhow::Error {
    let rollback_errors = snapshots
        .iter()
        .filter_map(|snapshot| snapshot.restore().err())
        .map(|error| format!("{error:#}"))
        .collect::<Vec<_>>();
    if rollback_errors.is_empty() {
        error
    } else {
        anyhow::anyhow!(
            "{error:#}; runtime preset rollback also failed: {}",
            rollback_errors.join("; ")
        )
    }
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

pub(crate) trait ProviderKeyVerifier {
    fn verify<'a>(
        &'a self,
        provider: ApiProvider,
        api_key: &'a str,
        base_url: &'a str,
    ) -> ProviderKeyVerification<'a>;
}

struct LiveProviderKeyVerifier;

impl ProviderKeyVerifier for LiveProviderKeyVerifier {
    fn verify<'a>(
        &'a self,
        provider: ApiProvider,
        api_key: &'a str,
        base_url: &'a str,
    ) -> ProviderKeyVerification<'a> {
        Box::pin(crate::client::verify_provider_api_key(
            provider, api_key, base_url,
        ))
    }
}

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

#[cfg(test)]
fn maybe_warn_context_pressure(app: &mut App) {
    let config = app.compaction_config();
    maybe_warn_context_pressure_for_config(app, &config);
}

fn maybe_warn_context_pressure_for_config(
    app: &mut App,
    config: &crate::compaction::CompactionConfig,
) {
    let max = config.effective_context_window.unwrap_or_else(|| {
        crate::route_budget::route_context_window_tokens(
            app.api_provider,
            app.effective_model_for_budget(),
            app.active_route_limits,
        )
    });
    let Some((used, max, percent)) = context_usage_snapshot_for_window(app, max) else {
        return;
    };

    let configured_threshold = app.auto_compact_threshold_percent.clamp(10.0, 100.0);
    let warning_threshold = CONTEXT_SUGGEST_COMPACT_THRESHOLD_PERCENT.min(configured_threshold);
    let will_auto_compact = config.enabled && used.max(0) as usize >= config.token_threshold;
    if percent < warning_threshold && !will_auto_compact {
        return;
    }

    let recommendation = if !config.enabled {
        "Consider enabling auto_compact or use /compact."
    } else if will_auto_compact {
        "Auto-compaction will run before the next send."
    } else {
        "Auto-compaction is enabled."
    };

    if percent >= CONTEXT_CRITICAL_THRESHOLD_PERCENT {
        app.status_message = Some(format!(
            "Context critical: {percent:.0}% ({used}/{max} tokens). {recommendation}"
        ));
        return;
    }

    if app.status_message.is_none() {
        let status_prefix = if percent >= CONTEXT_WARNING_THRESHOLD_PERCENT {
            "Context high"
        } else {
            "Context building"
        };
        app.status_message = Some(format!(
            "{status_prefix}: {percent:.0}% ({used}/{max} tokens). {recommendation}"
        ));
    }
}

#[cfg(test)]
fn should_auto_compact_before_send(app: &App) -> bool {
    let config = app.compaction_config();
    should_auto_compact_before_send_with_config(app, &config)
}

#[cfg(test)]
fn should_auto_compact_before_send_with_config(
    app: &App,
    config: &crate::compaction::CompactionConfig,
) -> bool {
    if !config.enabled {
        return false;
    }
    // Use the same ceiling-anchored token threshold as the engine. Comparing
    // against a raw percentage of the input-plus-output window can delay this
    // gate until after the spendable input budget has already been exhausted.
    let max = config.effective_context_window.unwrap_or_else(|| {
        crate::route_budget::route_context_window_tokens(
            app.api_provider,
            app.effective_model_for_budget(),
            app.active_route_limits,
        )
    });
    context_usage_snapshot_for_window(app, max)
        .map(|(used, _, _)| used.max(0) as usize >= config.token_threshold)
        .unwrap_or(false)
}

fn clamp_event_poll_timeout(timeout: Duration) -> Duration {
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
            "Run `/update install` here (preview it with a bare `/update`), or `codewhale update` in a shell, then restart CodeWhale."
                .to_string()
        } else {
            format!(
                "Installed via {label}. Run `{command}`, then restart CodeWhale.\n\
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
    use std::ffi::OsString;
    use tempfile::TempDir;

    struct ConfigPathEnvGuard {
        _tmp: TempDir,
        previous: Option<OsString>,
        _lock: crate::test_support::TestEnvLock,
    }

    impl ConfigPathEnvGuard {
        fn new() -> Self {
            let lock = crate::test_support::lock_test_env();
            let tmp = TempDir::new().expect("config tempdir");
            let config_path = tmp.path().join(".codewhale").join("config.toml");
            std::fs::create_dir_all(config_path.parent().expect("config parent"))
                .expect("config dir");
            let previous = std::env::var_os("DEEPSEEK_CONFIG_PATH");
            // Safety: test-only environment mutation guarded by a global mutex.
            unsafe {
                std::env::set_var("DEEPSEEK_CONFIG_PATH", &config_path);
            }
            Self {
                _tmp: tmp,
                previous,
                _lock: lock,
            }
        }

        fn config_path(&self) -> PathBuf {
            std::env::var_os("DEEPSEEK_CONFIG_PATH")
                .map(PathBuf::from)
                .expect("config path set")
        }
    }

    impl Drop for ConfigPathEnvGuard {
        fn drop(&mut self) {
            // Safety: test-only environment mutation guarded by a global mutex.
            unsafe {
                if let Some(previous) = self.previous.take() {
                    std::env::set_var("DEEPSEEK_CONFIG_PATH", previous);
                } else {
                    std::env::remove_var("DEEPSEEK_CONFIG_PATH");
                }
            }
        }
    }

    fn create_test_app() -> App {
        let options = TuiOptions {
            start_in_agent_mode: true,
            skip_onboarding: false,
            ..crate::test_support::test_tui_options(PathBuf::from("."))
        };
        let mut app = App::new(options, &Config::default());
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
            app.status_message
                .as_deref()
                .is_some_and(|status| status.contains("API key verified")),
            "status names verification success: {:?}",
            app.status_message
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
            rendered.contains("Default model") || rendered.contains("Pick a default model"),
            "expected model-pick stage UI, got:\n{rendered}"
        );
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
